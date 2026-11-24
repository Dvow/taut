//! TrueType hinting: a font's own instructions fit its small glyphs to the pixel grid. A
//! TrueType font may carry programs: `fpgm` defines functions, `prep` adjusts the control values
//! (`cvt `, heights and stem widths in font units) for a size, and each glyph has instructions
//! that move its points to them. They run on a stack machine of about 200 instructions, with a
//! graphics state (vectors to move and measure along, reference points, rounding), a storage
//! area, and a twilight zone of points not in the glyph.
//!
//! This runs them as FreeType's interpreter does by default (version 40, what Linux desktops
//! use): vertically only, like Windows' ClearType "natural" mode. Moves along x are ignored, and
//! so is anything after the glyph is interpolated (`IUP` on both axes); advances and kerning
//! stay as unhinted. A font that turns these off (`INSTCTRL` 3) is run as written. Arithmetic is
//! FreeType's, in 26.6 fixed point, rounding where it rounds, so the points come out the same
//! (see the tests). Composite glyphs hint each part, then run their own instructions; a variable
//! font varies its control values (`cvar`) and points (`gvar`, phantom points included) first.
//!
//! A hostile font is bounded as in FreeType: a million instructions per program, a call depth
//! of 32, loops and backward jumps counted against the size of the glyph and `cvt `, a stack
//! 50% larger than declared. Also bounded here: the points all instructions together may move
//! (`SHZ` on a large zone, a million times, is not). A program that fails leaves its glyph as
//! far as it got; one failing in `fpgm` or `prep` leaves the font to `hinting.rs`.
//!
//! Only with the feature `hinting`. Glyphs at [`LARGE`] pixels per em or more aren't hinted.
use crate::font_file::{Bytes, Dot, FontFile, Sketch, listed, outline};
use crate::geom::pt;
use crate::hinting::LARGE;
use crate::path::Path;

/// Instructions one program may run (FreeType's limit).
const MOST_STEPS: u32 = 1_000_000;
/// Points all instructions of one program together may move or measure.
const MOST_WORK: u64 = 10_000_000;
/// Nested calls.
const DEEPEST_CALLS: usize = 32;
/// Nested composite glyphs.
const DEEPEST_PARTS: usize = 16;
/// Points in one glyph, with its parts.
const MOST_POINTS: usize = 1 << 16;

/// A point or vector: x and y, in 26.6 fixed point (or font units, or F2Dot14 for a unit
/// vector).
type Pos = [i32; 2];

/// Touched along x, along y: tag bits, beside bit 0, on-curve.
const TOUCH_X: u8 = 8;
const TOUCH_Y: u8 = 16;
/// The backward-compatibility state (FreeType's `backward_compatibility`): bit 2 while on, and
/// bits 0 and 1 once `IUP` ran along x and y. All three set, nothing moves any more.
const COMPATIBLE: u8 = 4;
const SETTLED: u8 = 7;

/// Component flags of a composite glyph.
const WORDS: u16 = 0x1;
const OFFSET: u16 = 0x2;
const ROUND_OFFSET: u16 = 0x4;
const SCALE: u16 = 0x8;
const MORE: u16 = 0x20;
const SCALES: u16 = 0x40;
const MATRIX: u16 = 0x80;
const INSTRUCTED: u16 = 0x100;
const OWN_METRICS: u16 = 0x200;
const SCALED_OFFSET: u16 = 0x800;

/// FreeType's `FT_MulFix`: a × b / 65536, rounded half away from zero.
fn mul_fix(a: i32, b: i32) -> i32 {
    let ab = a as i64 * b as i64;
    ((ab + 0x8000 + (ab >> 63)) >> 16) as i32
}

/// FreeType's `FT_MulDiv` (`round`) and `FT_MulDiv_No_Round`: a × b / c on magnitudes, the sign
/// after. Dividing by 0 gives the largest number.
fn mul_div(a: i32, b: i32, c: i32, round: bool) -> i32 {
    let negative = (a < 0) ^ (b < 0) ^ (c < 0);
    let (a, b, c) = (a.unsigned_abs() as u64, b.unsigned_abs() as u64, c.unsigned_abs() as u64);
    let d = a.wrapping_mul(b).wrapping_add(if round { c >> 1 } else { 0 }).checked_div(c).unwrap_or(0x7fff_ffff) as i32;
    if negative { d.wrapping_neg() } else { d }
}

/// FreeType's `FT_DivFix`: a × 65536 / b, rounded.
fn div_fix(a: i32, b: i32) -> i32 {
    let negative = (a < 0) ^ (b < 0);
    let (a, b) = (a.unsigned_abs() as u64, b.unsigned_abs() as u64);
    let q = ((a << 16) + (b >> 1)).checked_div(b).unwrap_or(0x7fff_ffff) as i32;
    if negative { q.wrapping_neg() } else { q }
}

/// a × b / 16384 (b an F2Dot14), rounded half away from zero.
fn mul_14(a: i32, b: i32) -> i32 {
    let ab = a as i64 * b as i64;
    ((ab + 0x2000 + (ab >> 63)) >> 14) as i32
}

/// A vector projected on a unit vector (F2Dot14), rounded as `mul_14`.
fn dot_14(d: Pos, unit: Pos) -> i32 {
    let c = d[0] as i64 * unit[0] as i64 + d[1] as i64 * unit[1] as i64;
    ((c + 0x2000 + (c >> 63)) >> 14) as i32
}

/// The difference of two points.
fn minus(a: Pos, b: Pos) -> Pos {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1])]
}

/// A vector as a unit vector, F2Dot14, by FreeType's `FT_Vector_NormLen`: Newton's iterations on
/// an estimate, exactly as it counts. `None` for (0, 0), which leaves the vector as it was.
fn unit(x: i32, y: i32) -> Option<Pos> {
    let (sx, sy) = (if x < 0 { -1 } else { 1 }, if y < 0 { -1 } else { 1 });
    let (mut ux, mut uy) = (x.unsigned_abs(), y.unsigned_abs());
    match (ux, uy) {
        (0, 0) => return None,
        (0, _) => return Some([0, sy * 0x4000]),
        (_, 0) => return Some([sx * 0x4000, 0]),
        _ => {}
    }
    let estimate = |x: u32, y: u32| if x > y { x.wrapping_add(y >> 1) } else { y.wrapping_add(x >> 1) };
    let mut l = estimate(ux, uy);
    let mut shift = 31 - (31 - l.leading_zeros() as i32);
    shift -= 15 + (l >= (0xaaaa_aaaa_u32 >> shift)) as i32;
    if shift > 0 {
        (ux, uy) = (ux << shift, uy << shift);
        l = estimate(ux, uy);
    } else {
        (ux, uy, l) = (ux >> -shift, uy >> -shift, l >> -shift);
    }
    let (mut b, x, y) = (0x10000 - l as i32, ux as i32, uy as i32);
    let (mut u, mut v);
    loop {
        u = x.wrapping_add(x.wrapping_mul(b) >> 16) as u32;
        v = y.wrapping_add(y.wrapping_mul(b) >> 16) as u32;
        let z = (u.wrapping_mul(u).wrapping_add(v.wrapping_mul(v)) as i32).wrapping_neg() / 0x200;
        let z = z.wrapping_mul((0x10000 + b) >> 8) / 0x10000;
        b += z;
        if z <= 0 {
            break;
        }
    }
    Some([sx * u as i32 / 4, sy * v as i32 / 4])
}

/// How many values an instruction takes from the stack and gives back (FreeType's table).
/// Instructions that loop take theirs as they go.
fn counts(op: u8) -> (usize, usize) {
    match op {
        0x06..=0x0b | 0x27 | 0x2a | 0x3a | 0x3b | 0x3e | 0x3f | 0x42 | 0x44 | 0x48 | 0x70 | 0x78 | 0x79 | 0x81 | 0x82 | 0x86 | 0x87 | 0x8e | 0xe0..=0xff => (2, 0),
        0x0c | 0x0d => (0, 2),
        0x0f => (5, 0),
        0x10..=0x17 | 0x1a | 0x1c..=0x1f | 0x21 | 0x26 | 0x29 | 0x2b | 0x2c | 0x2e | 0x2f | 0x34..=0x38 | 0x4f | 0x58 => (1, 0),
        0x5d..=0x5f | 0x71..=0x77 | 0x7e | 0x7f | 0x85 | 0x89 | 0x8d | 0xc0..=0xdf => (1, 0),
        0x20 => (1, 2),
        0x23 => (2, 2),
        0x24 | 0x4b | 0x4c | 0x92 => (0, 1),
        0x25 | 0x43 | 0x45..=0x47 | 0x56 | 0x57 | 0x5c | 0x64..=0x6f | 0x88 => (1, 1),
        0x49 | 0x4a | 0x50..=0x55 | 0x5a | 0x5b | 0x60..=0x63 | 0x8b | 0x8c => (2, 1),
        0x8a => (3, 3),
        0xb0..=0xbf => (0, (op & 7) as usize + 1),
        _ => (0, 0),
    }
}

/// How rounding instructions round (`RTG`, `RTHG` and the rest).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Rounding {
    HalfGrid,
    Grid,
    DoubleGrid,
    DownToGrid,
    UpToGrid,
    Off,
    Super,
    Super45,
}

/// The graphics state: what `prep` may set for every glyph, and what a glyph program sets as it
/// goes.
#[derive(Clone, Copy, Debug)]
struct State {
    /// The three reference points, and which zone each of the three zone pointers points at
    /// (0: twilight, 1: the glyph).
    rp: [u16; 3],
    zones: [u8; 3],
    /// The dual projection, projection and freedom vectors (F2Dot14).
    dual: Pos,
    proj: Pos,
    free: Pos,
    looping: i32,
    min_distance: i32,
    cutin: i32,
    width_cutin: i32,
    width: i32,
    delta_base: u16,
    delta_shift: u16,
    auto_flip: bool,
    control: u8,
    scan_control: bool,
    scan_type: i32,
}

/// The graphics state every program starts from.
const DEFAULT: State = State {
    rp: [0; 3],
    zones: [1; 3],
    dual: [0x4000, 0],
    proj: [0x4000, 0],
    free: [0x4000, 0],
    looping: 1,
    min_distance: 64,
    cutin: 68,
    width_cutin: 0,
    width: 0,
    delta_base: 9,
    delta_shift: 3,
    auto_flip: true,
    control: 0,
    scan_control: false,
    scan_type: 0,
};

impl State {
    /// After `prep`: the default, with what `prep` may change.
    fn after_prep(&self) -> State {
        let State { min_distance, cutin, width_cutin, width, delta_base, delta_shift, auto_flip, control, scan_control, scan_type, .. } = *self;
        State { min_distance, cutin, width_cutin, width, delta_base, delta_shift, auto_flip, control, scan_control, scan_type, ..DEFAULT }
    }
}

/// Points of the twilight zone or of the glyph: as stored (font units), scaled, and current;
/// each one's tags; and where each contour ends.
#[derive(Clone, Debug, Default)]
struct Zone {
    orus: Vec<Pos>,
    org: Vec<Pos>,
    cur: Vec<Pos>,
    tags: Vec<u8>,
    ends: Vec<u16>,
}

impl Zone {
    /// A zone of `count` points at (0, 0), and no contours: the twilight zone.
    fn empty(count: usize) -> Zone {
        Zone { orus: vec![[0; 2]; count], org: vec![[0; 2]; count], cur: vec![[0; 2]; count], tags: vec![0; count], ends: Vec::new() }
    }
}

/// A function (`FDEF`) or instruction (`IDEF`) definition: its number and where its code is.
#[derive(Clone, Copy, Debug, Default)]
struct Def {
    number: u32,
    range: u8,
    start: usize,
    end: usize,
    active: bool,
}

/// A call in progress: where to return to, how many more times to run, and the function's
/// start and end.
#[derive(Clone, Copy, Debug)]
struct Call {
    range: u8,
    back: usize,
    count: i32,
    start: usize,
    end: usize,
}

/// A `hmtx` or `vmtx` table, and its count of long metrics (0: none).
#[derive(Clone, Copy)]
struct Metrics<'f> {
    table: Bytes<'f>,
    long: usize,
}

/// What a font's tables give the programs and the glyph loader.
struct Font<'f> {
    /// `fpgm`, `prep`, `cvt ` and `cvar`.
    functions: &'f [u8],
    control: &'f [u8],
    values: &'f [u8],
    cvar: Bytes<'f>,
    /// From `maxp`: the twilight points (with 4 phantom points), storage, function and
    /// instruction definitions, and stack size it declares; and its glyph count.
    twilight: usize,
    storage: usize,
    most_functions: usize,
    most_instructions: usize,
    stack: usize,
    glyphs: usize,
    /// Its axis count, if variable, and whether `HVAR` and `VVAR` vary its metrics.
    axes: usize,
    hvar: bool,
    vvar: bool,
    /// Horizontal and vertical metrics; without `vmtx`, the column's top and bottom (`OS/2`'s
    /// typographic ascender and descender, else `hhea`'s).
    widths: Metrics<'f>,
    heights: Metrics<'f>,
    column: [i32; 2],
}

impl<'f> Font<'f> {
    /// A TrueType font's programs and tables. `None` for a font without any program: nothing
    /// to run (FreeType autohints those too).
    fn of(file: &'f FontFile) -> Option<Font<'f>> {
        let (all, start) = (file.bytes(), file.start);
        let table = |name: &[u8; 4]| listed(all, start, name).map_or(Bytes(&[]), |range| all.within(&range));
        let (maxp, functions, control) = (table(b"maxp"), table(b"fpgm").0, table(b"prep").0);
        let instructed = maxp.u16(26).unwrap_or(0) > 0 || !functions.is_empty() || !control.is_empty();
        (instructed && maxp.u32(0) == Some(0x0001_0000) && listed(all, start, b"glyf").is_some()).then_some(())?;
        let count = |at: usize| maxp.u16(at).map_or(0, usize::from);
        let (hhea, vhea, os2) = (table(b"hhea"), table(b"vhea"), table(b"OS/2"));
        let heights = Metrics { table: table(b"vmtx"), long: vhea.u16(34).map_or(0, usize::from) };
        let column = [[68, 70], [4, 6]][os2.0.is_empty() as usize].map(|at| [os2, hhea][os2.0.is_empty() as usize].i16(at).unwrap_or(0) as i32);
        let stack = count(24) + (count(24) / 2).max(128);
        Some(Font {
            functions,
            control,
            values: table(b"cvt ").0,
            cvar: table(b"cvar"),
            twilight: count(16) + 4,
            storage: count(18),
            most_functions: count(20).max(64),
            most_instructions: count(22),
            stack,
            glyphs: count(4),
            axes: table(b"fvar").u16(8).map_or(0, usize::from),
            hvar: !table(b"HVAR").0.is_empty(),
            vvar: !table(b"VVAR").0.is_empty(),
            widths: Metrics { table: table(b"hmtx"), long: hhea.u16(34).map_or(0, usize::from) },
            heights: if heights.table.0.is_empty() { Metrics { long: 0, ..heights } } else { heights },
            column,
        })
    }

    /// `cvt `'s values in 26.6 font units, varied by `cvar` in a variable font instance.
    fn values(&self, file: &FontFile) -> Vec<i32> {
        let mut values: Vec<i32> = self.values.as_chunks::<2>().0.iter().map(|pair| i16::from_be_bytes(*pair) as i32 * 64).collect();
        if let Some(varied) = &file.varied {
            let moved = varied.controls(self.cvar, values.len());
            values.iter_mut().zip(moved).for_each(|(value, by)| *value += (by * 64.0 + 0.5).floor() as i32);
        }
        values
    }

    /// A glyph's advance and side bearing: long metrics first, then side bearings alone (the
    /// last advance repeating).
    fn metric(metrics: Metrics, glyph: u16) -> [i32; 2] {
        let (glyph, long, table) = (glyph as usize, metrics.long, metrics.table);
        if long == 0 {
            return [0, 0];
        }
        let advance = table.u16(4 * glyph.min(long - 1)).unwrap_or(0) as i32;
        let side = if glyph < long { table.i16(4 * glyph + 2) } else { table.i16(4 * long + 2 * (glyph - long)) };
        [advance, side.unwrap_or(0) as i32]
    }
}

/// A font's programs run for one size: what hinting its glyphs at that size takes.
pub(crate) struct Sized {
    /// Pixels per em, rounded, and the scale from font units to 26.6 pixels (16.16).
    ppem: i32,
    scale: i32,
    /// The control values, scaled and adjusted by `prep`; storage as `prep` left it.
    values: Vec<i32>,
    storage: Vec<i32>,
    /// The twilight zone: what `prep` and glyphs left there, as in FreeType.
    twilight: Zone,
    functions: Vec<Def>,
    instructions: Vec<Def>,
    most_function: u32,
    most_instruction: u32,
    /// The graphics state glyphs start from.
    state: State,
    /// From the font: limits, glyph and axis counts, normalized position.
    most_functions: usize,
    most_instructions: usize,
    stack: usize,
    glyphs: usize,
    axes: usize,
    place: Vec<i32>,
    /// Whether to hint glyphs at all (`prep` may say not to), and with backward compatibility.
    hinted: bool,
    compatible: u8,
}

impl Sized {
    /// Runs a font's `fpgm` and `prep` for `pixels` per em. `None` for a font with no programs
    /// (or one whose programs fail), or a size too large to hint.
    pub(crate) fn of(file: &FontFile, pixels: f32) -> Option<Sized> {
        (pixels > 0.0 && pixels < LARGE).then_some(())?;
        let font = Font::of(file)?;
        let units = (file.units() as i32).max(16);
        let (ppem, scale) = ((pixels).round() as i32, div_fix((pixels * 64.0).round() as i32, units));
        let mut sized = Sized {
            ppem,
            scale,
            values: Vec::new(),
            storage: vec![0; font.storage],
            twilight: Zone::empty(font.twilight),
            functions: Vec::new(),
            instructions: Vec::new(),
            most_function: 0,
            most_instruction: 0,
            state: DEFAULT,
            most_functions: font.most_functions,
            most_instructions: font.most_instructions,
            stack: font.stack,
            glyphs: font.glyphs,
            axes: font.axes,
            place: file.varied.as_ref().map_or(Vec::new(), |varied| varied.place.clone()),
            hinted: true,
            compatible: 0,
        };
        // `fpgm` defines functions; `prep` then starts from zeroed twilight and storage, and the
        // control values scaled (as FreeType: the 26.6 value truncated to whole units first).
        sized.values = font.values(file).iter().map(|value| mul_fix(value / 64, scale)).collect();
        // (`fpgm` sees control values and storage still zero, as in FreeType, and keeps
        // nothing of them.)
        if !font.functions.is_empty() {
            let zeros = (vec![0; sized.values.len()], vec![0; sized.storage.len()]);
            Machine::with(&mut sized, [font.functions, &[], &[]], Zone::default(), zeros, false).run(1)?;
        }
        let code = [font.functions, font.control, &[][..]];
        if !font.control.is_empty() {
            let (values, storage) = (std::mem::take(&mut sized.values), std::mem::take(&mut sized.storage));
            let mut machine = Machine::with(&mut sized, code, Zone::default(), (values, storage), false);
            let ran = machine.run(2);
            let (values, storage, state) = (machine.values, machine.storage, machine.state.after_prep());
            (sized.values, sized.storage, sized.state) = (values, storage, state);
            ran?;
        }
        // `prep` may turn hinting off at this size, ask for the default state, or for native
        // ClearType (no backward compatibility).
        sized.hinted = sized.state.control & 1 == 0;
        if sized.state.control & 2 != 0 {
            sized.state = DEFAULT;
        }
        sized.compatible = (sized.state.control & 4) ^ COMPATIBLE;
        Some(sized)
    }

    /// A glyph hinted: its outline in pixels, x right from the pen and y down from the
    /// baseline, and its bounds (left, top, right, bottom). `None` where it isn't hinted (`prep`
    /// said not to, or the glyph is damaged): then it is drawn unhinted.
    pub(crate) fn glyph(&mut self, file: &FontFile, glyph: u16) -> Option<(Path, [f32; 4])> {
        self.hinted.then_some(())?;
        let font = Font::of(file)?;
        let mut loader = Loader::new(self, file, &font);
        let shift = loader.load(glyph)?;
        let mut sketch = Sketch::new();
        let mut start = 0;
        for end in loader.outline.ends.iter().copied() {
            let dot = |(at, tags): (&Pos, &u8)| Dot { flags: tags & 1, at: pt((at[0] - shift) as f32 / 64.0, -at[1] as f32 / 64.0) };
            let dots: Vec<Dot> = loader.outline.points.get(start..end)?.iter().zip(&loader.outline.tags[start..end]).map(dot).collect();
            outline(&dots, &mut sketch);
            start = end;
        }
        let reach = if sketch.reach[0] > sketch.reach[2] { [0.0; 4] } else { sketch.reach };
        Some((sketch.path, reach))
    }
}

/// An instruction stopped its program: a damaged font, or one taking too long.
type Ran = Option<()>;

/// The interpreter, running one program for a size: on its twilight zone and definitions, on
/// a glyph's points, and on control values and storage of its own (a glyph's copies).
struct Machine<'a> {
    size: &'a mut Sized,
    /// `fpgm`, `prep` and the glyph's instructions; which runs, which ran first, where, and the
    /// length of the instruction there.
    code: [&'a [u8]; 3],
    range: u8,
    first: u8,
    ip: usize,
    length: usize,
    op: u8,
    glyph: Zone,
    values: Vec<i32>,
    storage: Vec<i32>,
    /// The stack: its top, and where the current instruction's arguments start and end.
    stack: Vec<i32>,
    top: usize,
    args: usize,
    new_top: usize,
    calls: Vec<Call>,
    state: State,
    /// How the rounding instructions round, and `SROUND`'s period, phase and threshold.
    round: Rounding,
    period: i32,
    phase: i32,
    threshold: i32,
    /// The freedom vector as moved along when measured on the projection vector (16.16).
    moves: Pos,
    compatible: u8,
    composite: bool,
    /// The scale of distances in font units: the size's, or 1 for a composite's own program.
    scale: i32,
    /// The twilight points usable, and the loop and backward jump budgets.
    twilight: usize,
    loops: u64,
    jumps: u64,
    most_loops: u64,
    work: u64,
}

impl<'a> Machine<'a> {
    /// A machine on these control values and storage.
    fn with(size: &'a mut Sized, code: [&'a [u8]; 3], glyph: Zone, (values, storage): (Vec<i32>, Vec<i32>), composite: bool) -> Machine<'a> {
        let (stack, state, compatible, scale) = (vec![0; size.stack.max(8)], size.state, size.compatible, if composite { 0x10000 } else { size.scale });
        Machine {
            size,
            code,
            range: 0,
            first: 0,
            ip: 0,
            length: 1,
            op: 0,
            glyph,
            values,
            storage,
            stack,
            top: 0,
            args: 0,
            new_top: 0,
            calls: Vec::new(),
            state,
            round: Rounding::Grid,
            period: 64,
            phase: 0,
            threshold: 0,
            moves: [0x10000, 0],
            compatible,
            composite,
            scale,
            twilight: 0,
            loops: 0,
            jumps: 0,
            most_loops: 0,
            work: 0,
        }
    }

    /// Runs the program in `range` (1: `fpgm`, 2: `prep`, 3: the glyph's) from its start.
    /// `None` if an instruction stopped it.
    fn run(&mut self, range: u8) -> Ran {
        let (points, values) = (self.glyph.cur.len() as u64, self.values.len() as u64);
        self.twilight = (2 * (points + values)).clamp(30, 0xffff) as usize;
        let loops = if points > 0 { (10 * points).max(50) + (values / 10).max(50) } else { 300 + 22 * values };
        (self.most_loops, self.loops, self.jumps) = (loops.min(100 * self.size.glyphs as u64), 0, 0);
        (self.round, self.compatible) = (Rounding::Grid, self.compatible & !3);
        self.vectors_set();
        (self.range, self.first, self.ip, self.top) = (range, range, 0, 0);
        self.calls.clear();
        for _ in 0..MOST_STEPS {
            (self.work <= MOST_WORK).then_some(())?;
            self.op = *self.code().get(self.ip)?;
            self.length = 1;
            let (pops, pushes) = counts(self.op);
            self.args = self.top.checked_sub(pops).unwrap_or_else(|| {
                self.stack[..pops].fill(0);
                0
            });
            self.new_top = self.args + pushes;
            (self.new_top <= self.stack.len()).then_some(())?;
            self.step()?;
            self.top = self.new_top;
            self.ip += self.length;
            if self.ip >= self.code().len() {
                return self.calls.is_empty().then_some(());
            }
        }
        None
    }

    /// The code being run.
    fn code(&self) -> &'a [u8] {
        self.code[(self.range as usize).clamp(1, 3) - 1]
    }

    /// The `n`-th argument of the instruction (0: the deepest).
    fn arg(&self, n: usize) -> i32 {
        self.stack[self.args + n]
    }

    /// The value `n` places below the arguments.
    fn below(&self, n: usize) -> i32 {
        self.stack[self.args - n]
    }

    /// Sets the `n`-th result.
    fn give(&mut self, n: usize, value: i32) {
        self.stack[self.args + n] = value;
    }

    /// Runs the instruction at the instruction pointer.
    fn step(&mut self) -> Ran {
        let (op, a, b) = (self.op, self.arg(0), self.arg(1));
        match op {
            0x00..=0x05 => self.axis(op),
            0x06..=0x09 => self.along_line(op, b as u16 as usize, a as u16 as usize),
            0x0a | 0x0b => self.vector_given(op, a as i16 as i32, b as i16 as i32),
            0x0c => self.give_two(self.state.proj),
            0x0d => self.give_two(self.state.free),
            0x0e => {
                self.state.free = self.state.proj;
                self.vectors_set();
            }
            0x0f => self.intersect(),
            0x10..=0x12 => self.state.rp[op as usize - 0x10] = a as u16,
            0x13..=0x16 => self.zones(op, a),
            0x17 => return self.looping(a),
            0x18 => self.round = Rounding::Grid,
            0x19 => self.round = Rounding::HalfGrid,
            0x1a => self.state.min_distance = a,
            0x1b => return self.skip_else(),
            0x1c => return self.jump(a),
            0x1d => self.state.cutin = a,
            0x1e => self.state.width_cutin = a,
            0x1f => self.state.width = mul_fix(a, self.size.scale),
            0x20 => self.give(1, a),
            0x21 | 0x28 | 0x59 | 0x7e | 0x7f => {}
            0x22 => self.new_top = 0,
            0x23 => self.give_two([b, a]),
            0x24 => self.give(0, self.top as i32),
            0x25 => self.copy_index(a),
            0x26 => self.move_index(a),
            0x27 => self.align_points(a as u16 as usize, b as u16 as usize),
            0x29 => self.untouch(a as u16 as usize),
            0x2a => return self.call(b, a),
            0x2b => return self.call(a, 1),
            0x2c => return self.define(a, false),
            0x2d => return self.end_function(),
            0x2e | 0x2f => self.anchor(op, a as u16 as usize),
            0x30 | 0x31 => self.interpolate_untouched(op),
            0x32 | 0x33 => self.shift_points(op),
            0x34 | 0x35 => self.shift_contour(op, a as u16 as usize),
            0x36 | 0x37 => self.shift_zone(op, a),
            0x38 => self.shift_pixels(a),
            0x39 => self.interpolate(),
            0x3a | 0x3b => self.relative_stretch(op, a as u16 as usize, b),
            0x3c => self.align_to_reference(),
            0x3d => self.round = Rounding::DoubleGrid,
            0x3e | 0x3f => self.anchor_to_value(op, a as u16 as usize, b as u32 as usize),
            0x40 | 0x41 | 0xb0..=0xbf => return self.push(op),
            0x42 => self.write_storage(a as u32 as usize, b),
            0x43 => self.give(0, self.storage.get(a as u32 as usize).copied().unwrap_or(0)),
            0x44 => self.write_value(a as u32 as usize, b, false),
            0x45 => self.give(0, self.values.get(a as u32 as usize).copied().unwrap_or(0)),
            0x46 | 0x47 => self.coordinate(op, a as u32 as usize),
            0x48 => self.coordinate_set(a as u16 as usize, b),
            0x49 | 0x4a => self.measure(op, a as u16 as usize, b as u16 as usize),
            0x4b => self.give(0, self.size.ppem),
            0x4c => self.give(0, self.size.ppem * 64),
            0x4d | 0x4e => self.state.auto_flip = op == 0x4d,
            0x4f => return None,
            0x50 => self.give(0, (a < b) as i32),
            0x51 => self.give(0, (a <= b) as i32),
            0x52 => self.give(0, (a > b) as i32),
            0x53 => self.give(0, (a >= b) as i32),
            0x54 => self.give(0, (a == b) as i32),
            0x55 => self.give(0, (a != b) as i32),
            0x56 => self.give(0, (self.rounded(a, 0) & 64 == 64) as i32),
            0x57 => self.give(0, (self.rounded(a, 0) & 64 == 0) as i32),
            0x58 => return self.skip_if(a),
            0x5a => self.give(0, (a != 0 && b != 0) as i32),
            0x5b => self.give(0, (a != 0 || b != 0) as i32),
            0x5c => self.give(0, (a == 0) as i32),
            0x5d | 0x71 | 0x72 => self.delta_points(op, a),
            0x5e => self.state.delta_base = a as u16,
            0x5f if a as u32 > 6 => return None,
            0x5f => self.state.delta_shift = a as u16,
            0x60 => self.give(0, a.wrapping_add(b)),
            0x61 => self.give(0, a.wrapping_sub(b)),
            0x62 if b == 0 => return None,
            0x62 => self.give(0, mul_div(a, 64, b, false)),
            0x63 => self.give(0, mul_div(a, b, 64, true)),
            0x64 => self.give(0, a.wrapping_abs()),
            0x65 => self.give(0, a.wrapping_neg()),
            0x66 => self.give(0, a & -64),
            0x67 => self.give(0, a.wrapping_add(63) & -64),
            0x68..=0x6b => self.give(0, self.rounded(a, 0)),
            0x6c..=0x6f => self.give(0, rounded(Rounding::Off, a, 0, [0; 3])),
            0x70 => self.write_value(a as u32 as usize, b, true),
            0x73..=0x75 => self.delta_values(op, a),
            0x76 | 0x77 => self.super_round(op, a),
            0x78 if b != 0 => return self.jump(a),
            0x79 if b == 0 => return self.jump(a),
            0x78 | 0x79 => {}
            0x7a => self.round = Rounding::Off,
            0x7c => self.round = Rounding::UpToGrid,
            0x7d => self.round = Rounding::DownToGrid,
            0x80 => self.flip_points(),
            0x81 | 0x82 => self.flip_range(op, b as u16 as usize, a as u16 as usize),
            0x85 => self.scan_control(a),
            0x86 | 0x87 => self.dual_line(op, b as u16 as usize, a as u16 as usize),
            0x88 => self.give(0, self.info(a)),
            0x89 => return self.define(a, true),
            0x8a => {
                self.give_two([b, self.arg(2)]);
                self.give(2, a);
            }
            0x8b => self.give(0, a.max(b)),
            0x8c => self.give(0, a.min(b)),
            0x8d if a >= 0 => self.state.scan_type = a & 0xffff,
            0x8d => {}
            0x8e => self.control(a as u32, b as u32),
            0x91 if self.size.axes > 0 => return self.variation(),
            0x92 if self.size.axes > 0 => self.give(0, 17),
            0xc0..=0xdf => self.direct_relative(op, a as u16 as usize),
            0xe0..=0xff => self.indirect_relative(op, a as u16 as usize, b),
            _ => return self.unknown(),
        }
        Some(())
    }
}

/// A distance rounded as `mode` says, after adding `compensation` (FreeType's rounding
/// functions). `period`, `phase` and `threshold` are `SROUND`'s. The sign is kept: a distance
/// rounding past zero becomes 0 (or the least of its grid, for half grids and phases).
fn rounded(mode: Rounding, distance: i32, compensation: i32, [period, phase, threshold]: [i32; 3]) -> i32 {
    let (d, c, cut) = (distance, compensation, threshold.wrapping_sub(phase));
    let size = if d >= 0 { d.wrapping_add(c) } else { c.wrapping_sub(d) };
    let whole = match mode {
        Rounding::Off => size,
        Rounding::Grid => size.wrapping_add(32) & -64,
        Rounding::HalfGrid => (size & -64).wrapping_add(32),
        Rounding::DownToGrid => size & -64,
        Rounding::UpToGrid => size.wrapping_add(63) & -64,
        Rounding::DoubleGrid => size.wrapping_add(16) & -32,
        Rounding::Super => (size.wrapping_add(cut) & -period).wrapping_add(phase),
        Rounding::Super45 => (size.wrapping_add(cut) / period * period).wrapping_add(phase),
    };
    let least = match mode {
        Rounding::HalfGrid => 32,
        Rounding::Super | Rounding::Super45 => phase,
        _ => 0,
    };
    match d >= 0 {
        true if whole < 0 => least,
        true => whole,
        false if whole.wrapping_neg() > 0 => least.wrapping_neg(),
        false => whole.wrapping_neg(),
    }
}

/// A unit vector along `d`, or across it (turned left) with `op & 1`. Along x if `d` is
/// (0, 0), and `op` is then cleared.
fn line(d: Pos, op: &mut u8) -> Pos {
    if d == [0, 0] {
        *op = 0;
    }
    let d = if d == [0, 0] { [0x4000, 0] } else { d };
    let d = if *op & 1 != 0 { [d[1].wrapping_neg(), d[0]] } else { d };
    unit(d[0], d[1]).unwrap_or([0x4000, 0])
}

/// A distance kept at least `least` long, on the side `original` is.
fn at_least(original: i32, distance: i32, least: i32) -> i32 {
    if original >= 0 { distance.max(least) } else { distance.min(least.wrapping_neg()) }
}

impl Machine<'_> {
    /// Sets the first two results.
    fn give_two(&mut self, [a, b]: [i32; 2]) {
        self.give(0, a);
        self.give(1, b);
    }

    /// The zone a zone pointer (0, 1 or 2) points at.
    fn zone(&self, pointer: usize) -> &Zone {
        if self.state.zones[pointer] == 0 { &self.size.twilight } else { &self.glyph }
    }

    fn zone_mut(&mut self, pointer: usize) -> &mut Zone {
        if self.state.zones[pointer] == 0 { &mut self.size.twilight } else { &mut self.glyph }
    }

    /// Whether that zone has this point.
    fn has(&self, pointer: usize, point: usize) -> bool {
        let count = if self.state.zones[pointer] == 0 { self.size.twilight.cur.len().min(self.twilight) } else { self.glyph.cur.len() };
        point < count
    }

    /// A vector's length along the projection vector, and along the dual projection vector.
    fn project(&self, d: Pos) -> i32 {
        dot_14(d, self.state.proj)
    }

    fn dual(&self, d: Pos) -> i32 {
        dot_14(d, self.state.dual)
    }

    /// The original distance from one point to another along the dual projection vector: from
    /// their scaled originals with a twilight point involved, else from their font units.
    fn original(&self, (one, a): (usize, usize), (other, b): (usize, usize), twilight: bool) -> i32 {
        match twilight {
            true => self.dual(minus(self.zone(one).org[a], self.zone(other).org[b])),
            false => mul_fix(self.dual(minus(self.zone(one).orus[a], self.zone(other).orus[b])), self.scale),
        }
    }

    /// A distance rounded the current way.
    fn rounded(&self, distance: i32, compensation: i32) -> i32 {
        rounded(self.round, distance, compensation, [self.period, self.phase, self.threshold])
    }

    /// After the vectors change: how far a point moves along the freedom vector to move
    /// `distance` along the projection vector (FreeType's `Compute_Funcs`). Nearly
    /// perpendicular vectors move nothing.
    fn vectors_set(&mut self) {
        let (p, f) = (self.state.proj, self.state.free);
        let dot = (p[0] as i64 * f[0] as i64 + p[1] as i64 * f[1] as i64 + 0x2000) >> 14;
        self.moves = match dot {
            0x3ffe.. => [f[0] * 4, f[1] * 4],
            -0x3ff..=0x3ff => [0, 0],
            _ => [(f[0] as i64 * 0x10000 / dot) as i32, (f[1] as i64 * 0x10000 / dot) as i32],
        };
    }

    /// Moves a point `distance` along the projection vector, by the freedom vector, and marks
    /// it touched (FreeType's `Direct_Move`). With backward compatibility, never along x, and
    /// not at all once the glyph was interpolated on both axes.
    fn shift(&mut self, pointer: usize, point: usize, distance: i32) {
        let (moves, compatible) = (self.moves, self.compatible);
        let zone = self.zone_mut(pointer);
        if moves[0] != 0 && compatible == 0 {
            zone.cur[point][0] = zone.cur[point][0].wrapping_add(mul_fix(distance, moves[0]));
        }
        if moves[1] != 0 && compatible != SETTLED {
            zone.cur[point][1] = zone.cur[point][1].wrapping_add(mul_fix(distance, moves[1]));
        }
        zone.tags[point] |= if moves[0] != 0 { TOUCH_X } else { 0 } | if moves[1] != 0 { TOUCH_Y } else { 0 };
    }

    /// Moves a point's original position the same way, untouched.
    fn shift_original(&mut self, pointer: usize, point: usize, distance: i32) {
        let moves = self.moves;
        let zone = self.zone_mut(pointer);
        for axis in [0, 1].into_iter().filter(|axis| moves[*axis] != 0) {
            zone.org[point][axis] = zone.org[point][axis].wrapping_add(mul_fix(distance, moves[axis]));
        }
    }

    /// Moves a point of zone 2 by `[dx, dy]`, on the axes the freedom vector has (FreeType's
    /// `Move_Zp2_Point`).
    fn move_by(&mut self, point: usize, [dx, dy]: Pos, touch: bool) {
        let (free, compatible) = (self.state.free, self.compatible);
        let zone = self.zone_mut(2);
        if free[0] != 0 && compatible == 0 {
            zone.cur[point][0] = zone.cur[point][0].wrapping_add(dx);
        }
        if free[1] != 0 && compatible != SETTLED {
            zone.cur[point][1] = zone.cur[point][1].wrapping_add(dy);
        }
        if touch {
            zone.tags[point] |= if free[0] != 0 { TOUCH_X } else { 0 } | if free[1] != 0 { TOUCH_Y } else { 0 };
        }
    }

    /// `SVTCA`, `SPVTCA`, `SFVTCA`: vectors along an axis (x with `op & 1`).
    fn axis(&mut self, op: u8) {
        let along = if op & 1 != 0 { [0x4000, 0] } else { [0, 0x4000] };
        if op < 4 {
            (self.state.proj, self.state.dual) = (along, along);
        }
        if op & 2 == 0 {
            self.state.free = along;
        }
        self.vectors_set();
    }

    /// `SPVTL`, `SFVTL`: a vector along the line from point `one` (zone 2) to `two` (zone 1),
    /// or across it with `op & 1`.
    fn along_line(&mut self, op: u8, one: usize, two: usize) {
        if !self.has(2, one) || !self.has(1, two) {
            return;
        }
        let along = line(minus(self.zone(1).cur[two], self.zone(2).cur[one]), &mut op.clone());
        match op < 8 {
            true => (self.state.proj, self.state.dual) = (along, along),
            false => self.state.free = along,
        }
        self.vectors_set();
    }

    /// `SPVFS`, `SFVFS`: a vector from the stack.
    fn vector_given(&mut self, op: u8, x: i32, y: i32) {
        let along = unit(x, y);
        match op {
            0x0a => {
                self.state.proj = along.unwrap_or(self.state.proj);
                self.state.dual = self.state.proj;
            }
            _ => self.state.free = along.unwrap_or(self.state.free),
        }
        self.vectors_set();
    }

    /// `SDPVTL`: the projection vector along the line between two points now, and the dual one
    /// along it originally (across it, with `op & 1`).
    fn dual_line(&mut self, op: u8, one: usize, two: usize) {
        if !self.has(1, two) || !self.has(2, one) {
            return;
        }
        // (A line of two coinciding points is x, for the current points too.)
        let mut op = op;
        self.state.dual = line(minus(self.zone(1).org[two], self.zone(2).org[one]), &mut op);
        self.state.proj = line(minus(self.zone(1).cur[two], self.zone(2).cur[one]), &mut op);
        self.vectors_set();
    }

    /// `ISECT`: a point (zone 2) at the intersection of two lines, `a` (zone 1) and `b` (zone
    /// 0); halfway between their middles if they are nearly parallel.
    fn intersect(&mut self) {
        let [point, a0, a1, b0, b1] = [0, 1, 2, 3, 4].map(|n| self.arg(n) as u16 as usize);
        if !(self.has(0, b0) && self.has(0, b1) && self.has(1, a0) && self.has(1, a1) && self.has(2, point)) {
            return;
        }
        let ([pa0, pa1], [pb0, pb1]) = ([a0, a1].map(|n| self.zone(1).cur[n]), [b0, b1].map(|n| self.zone(0).cur[n]));
        let (db, da, d) = (minus(pb1, pb0), minus(pa1, pa0), minus(pb0, pa0));
        let cross = |u: Pos, v: Pos| mul_div(u[0], v[1].wrapping_neg(), 0x40, true).wrapping_add(mul_div(u[1], v[0], 0x40, true));
        let (discriminant, dot) = (cross(da, db), mul_div(da[0], db[0], 0x40, true).wrapping_add(mul_div(da[1], db[1], 0x40, true)));
        let at = if 19 * (discriminant as i64).abs() > (dot as i64).abs() {
            let along = cross(d, db);
            [0, 1].map(|k| pa0[k].wrapping_add(mul_div(along, da[k], discriminant, true)))
        } else {
            [0, 1].map(|k| pa0[k].wrapping_add(pa1[k]).wrapping_add(pb0[k].wrapping_add(pb1[k])) / 4)
        };
        let zone = self.zone_mut(2);
        zone.cur[point] = at;
        zone.tags[point] |= TOUCH_X | TOUCH_Y;
    }

    /// `SZP0`, `SZP1`, `SZP2`, `SZPS`: the zone pointers.
    fn zones(&mut self, op: u8, zone: i32) {
        match (zone, op) {
            (0 | 1, 0x16) => self.state.zones = [zone as u8; 3],
            (0 | 1, _) => self.state.zones[op as usize - 0x13] = zone as u8,
            _ => {}
        }
    }

    /// `SLOOP`.
    fn looping(&mut self, count: i32) -> Ran {
        (count >= 0).then_some(())?;
        self.state.looping = count.min(0xffff);
        Some(())
    }

    /// The point numbers a looping instruction takes, from the top of the stack down (none if
    /// the stack hasn't that many). The loop count goes back to 1.
    fn looped(&mut self) -> Vec<i32> {
        let count = std::mem::replace(&mut self.state.looping, 1) as usize;
        if self.new_top < count {
            return Vec::new();
        }
        self.new_top -= count;
        self.work += count as u64;
        (1..=count).map(|n| self.below(n)).collect()
    }

    /// The instruction after the current one, skipped over: its opcode and length.
    fn skip(&mut self) -> Ran {
        self.ip += self.length;
        let code = self.code();
        self.op = *code.get(self.ip)?;
        self.length = match self.op {
            0x40 => 2 + *code.get(self.ip + 1)? as usize,
            0x41 => 2 + 2 * *code.get(self.ip + 1)? as usize,
            0xb0..=0xb7 => (self.op - 0xb0) as usize + 2,
            0xb8..=0xbf => 2 * (self.op - 0xb8) as usize + 3,
            _ => 1,
        };
        Some(())
    }

    /// `IF`: on false, on to the matching `ELSE` or `EIF`.
    fn skip_if(&mut self, condition: i32) -> Ran {
        let mut depth = (condition == 0) as i32;
        while depth > 0 {
            self.skip()?;
            depth += (self.op == 0x58) as i32 - (self.op == 0x59) as i32;
            if self.op == 0x1b && depth == 1 {
                break;
            }
        }
        Some(())
    }

    /// `ELSE`, reached after a true branch: on to the matching `EIF`.
    fn skip_else(&mut self) -> Ran {
        let mut depth = 1;
        while depth > 0 {
            self.skip()?;
            depth += (self.op == 0x58) as i32 - (self.op == 0x59) as i32;
        }
        Some(())
    }

    /// `JMPR` (and `JROT`, `JROF` when they jump): `by` bytes on from the instruction. A jump
    /// back counts against the loop budget; none may leave the function it is in.
    fn jump(&mut self, by: i32) -> Ran {
        (by != 0 || self.args != 0).then_some(())?;
        let (to, end) = (self.ip as i64 + by as i64, self.calls.last().map_or(i64::MAX, |call| call.end as i64));
        (0..=end).contains(&to).then_some(())?;
        (self.ip, self.length) = (to as usize, 0);
        self.jumps += (by < 0) as u64;
        (self.jumps <= self.most_loops).then_some(())
    }

    /// `CINDEX`: a copy of the value `n` down.
    fn copy_index(&mut self, n: i32) {
        let value = if n <= 0 || n as usize > self.args { 0 } else { self.below(n as usize) };
        self.give(0, value);
    }

    /// `MINDEX`: the value `n` down, moved to the top.
    fn move_index(&mut self, n: i32) {
        if n <= 0 || n as usize > self.args {
            return;
        }
        let (n, at) = (n as usize, self.args);
        let moved = self.stack[at - n];
        self.stack.copy_within(at - n + 1..at, at - n);
        self.stack[at - 1] = moved;
    }

    /// `ALIGNPTS`: two points moved to meet halfway.
    fn align_points(&mut self, one: usize, two: usize) {
        if !self.has(1, one) || !self.has(0, two) {
            return;
        }
        let distance = self.project(minus(self.zone(0).cur[two], self.zone(1).cur[one])) / 2;
        self.shift(1, one, distance);
        self.shift(0, two, distance.wrapping_neg());
    }

    /// `UTP`: a point untouched on the axes the freedom vector has.
    fn untouch(&mut self, point: usize) {
        if !self.has(0, point) {
            return;
        }
        let free = self.state.free;
        let mask = if free[0] != 0 { TOUCH_X } else { 0 } | if free[1] != 0 { TOUCH_Y } else { 0 };
        self.zone_mut(0).tags[point] &= !mask;
    }

    /// A function's definition, if defined and active.
    fn function(&self, n: u32) -> Option<Def> {
        (n <= self.size.most_function).then_some(())?;
        let defs = &self.size.functions;
        let direct = defs.get(n as usize).filter(|def| self.size.most_function as usize + 1 == defs.len() && def.number == n);
        direct.or_else(|| defs.iter().find(|def| def.number == n)).filter(|def| def.active).copied()
    }

    /// `CALL` and `LOOPCALL`: runs a function `count` times.
    fn call(&mut self, function: i32, count: i32) -> Ran {
        let def = self.function(function as u32)?;
        (self.calls.len() < DEEPEST_CALLS).then_some(())?;
        if count <= 0 {
            return Some(());
        }
        self.calls.push(Call { range: self.range, back: self.ip + 1, count, start: def.start, end: def.end });
        self.goto(def.range, def.start)?;
        self.loops += if self.op == 0x2a { count as u64 } else { 0 };
        (self.loops <= self.most_loops).then_some(())
    }

    /// Continues in another code range.
    fn goto(&mut self, range: u8, ip: usize) -> Ran {
        let code = *self.code.get((range as usize).checked_sub(1)?)?;
        (!code.is_empty() && ip <= code.len()).then_some(())?;
        (self.range, self.ip, self.length) = (range, ip, 0);
        Some(())
    }

    /// `ENDF`: back to the caller, or the function again for a `LOOPCALL`.
    fn end_function(&mut self) -> Ran {
        let call = self.calls.last_mut()?;
        call.count -= 1;
        if call.count > 0 {
            (self.ip, self.length) = (call.start, 0);
            return Some(());
        }
        let call = self.calls.pop()?;
        self.goto(call.range, call.back)
    }

    /// `FDEF`, `IDEF`: defines function (or instruction) `n` as the code up to `ENDF`. Only in
    /// `fpgm` and `prep`; a redefinition replaces.
    fn define(&mut self, n: i32, instruction: bool) -> Ran {
        (self.first != 3).then_some(())?;
        let (n, most, highest) = (n as u32, [self.size.most_functions, self.size.most_instructions][instruction as usize], [0xffff, 0xff][instruction as usize]);
        let defs = if instruction { &mut self.size.instructions } else { &mut self.size.functions };
        let at = defs.iter().position(|def| def.number == n).unwrap_or(defs.len());
        if at == defs.len() {
            (at < most).then_some(())?;
            defs.push(Def::default());
        }
        (n <= highest).then_some(())?;
        defs[at] = Def { number: n, range: self.range, start: self.ip + 1, end: 0, active: true };
        let top = if instruction { &mut self.size.most_instruction } else { &mut self.size.most_function };
        *top = (*top).max(n);
        loop {
            self.skip()?;
            match self.op {
                0x89 | 0x2c => return None,
                0x2d => break,
                _ => {}
            }
        }
        let defs = if instruction { &mut self.size.instructions } else { &mut self.size.functions };
        defs[at].end = self.ip;
        Some(())
    }

    /// An instruction not in the set: one a font defined (`IDEF`), else the program stops.
    fn unknown(&mut self) -> Ran {
        let def = *self.size.instructions.iter().find(|def| def.number as u8 == self.op && def.active)?;
        (self.calls.len() < DEEPEST_CALLS).then_some(())?;
        self.calls.push(Call { range: self.range, back: self.ip + 1, count: 1, start: def.start, end: def.end });
        self.goto(def.range, def.start)
    }

    /// `NPUSHB`, `NPUSHW`, `PUSHB`, `PUSHW`: values from the code onto the stack.
    fn push(&mut self, op: u8) -> Ran {
        let (code, wide, counted) = (self.code(), op == 0x41 || op >= 0xb8, op < 0x42);
        let at = self.ip + counted as usize;
        let count = if counted { *code.get(at)? as usize } else { (op & 7) as usize + 1 };
        let long = if wide { 2 * count } else { count };
        (at + long < code.len() && count < self.stack.len() + 1 - self.top).then_some(())?;
        for n in 0..count {
            let value = if wide { i16::from_be_bytes([code[at + 1 + 2 * n], code[at + 2 + 2 * n]]) as i32 } else { code[at + 1 + n] as i32 };
            self.give(n, value);
        }
        self.new_top += if counted { count } else { 0 };
        self.ip = at + long;
        Some(())
    }

    /// `WS`.
    fn write_storage(&mut self, at: usize, value: i32) {
        if let Some(slot) = self.storage.get_mut(at) {
            *slot = value;
        }
    }

    /// `WCVTP`, and `WCVTF` (`units`: a value in font units, scaled).
    fn write_value(&mut self, at: usize, value: i32, units: bool) {
        let value = if units { mul_fix(value, self.size.scale) } else { value };
        if let Some(slot) = self.values.get_mut(at) {
            *slot = value;
        }
    }

    /// `GC`: a point's coordinate along the projection vector, now (or originally, along the
    /// dual one, with `op & 1`).
    fn coordinate(&mut self, op: u8, point: usize) {
        let at = match (self.has(2, point), op & 1) {
            (false, _) => 0,
            (true, 0) => self.project(self.zone(2).cur[point]),
            (true, _) => self.dual(self.zone(2).org[point]),
        };
        self.give(0, at);
    }

    /// `SCFS`: a point moved to a coordinate along the projection vector.
    fn coordinate_set(&mut self, point: usize, to: i32) {
        if !self.has(2, point) {
            return;
        }
        let now = self.project(self.zone(2).cur[point]);
        self.shift(2, point, to.wrapping_sub(now));
        if self.state.zones[2] == 0 {
            let zone = self.zone_mut(2);
            zone.org[point] = zone.cur[point];
        }
    }

    /// `MD`: the distance between two points, now (`op & 1`) or originally.
    fn measure(&mut self, op: u8, one: usize, two: usize) {
        let twilight = self.state.zones[0] == 0 || self.state.zones[1] == 0;
        let distance = match (self.has(0, one) && self.has(1, two), op & 1) {
            (false, _) => 0,
            (true, 1) => self.project(minus(self.zone(0).cur[one], self.zone(1).cur[two])),
            (true, _) => self.original((0, one), (1, two), twilight),
        };
        self.give(0, distance);
    }

    /// `MDAP`: a point made a reference, rounded with `op & 1`.
    fn anchor(&mut self, op: u8, point: usize) {
        if !self.has(0, point) {
            return;
        }
        let now = self.project(self.zone(0).cur[point]);
        let distance = if op & 1 != 0 { self.rounded(now, 0).wrapping_sub(now) } else { 0 };
        self.shift(0, point, distance);
        self.state.rp[..2].fill(point as u16);
    }

    /// `MIAP`: a point moved to a control value; rounded with `op & 1`, unless the value is
    /// further than the cut-in from where the point is. A twilight point is first put there.
    fn anchor_to_value(&mut self, op: u8, point: usize, entry: usize) {
        if !self.has(0, point) || entry >= self.values.len() {
            return self.state.rp[..2].fill(point as u16);
        }
        let (value, free) = (self.values[entry], self.state.free);
        if self.state.zones[0] == 0 {
            let zone = self.zone_mut(0);
            zone.org[point] = [mul_14(value, free[0]), mul_14(value, free[1])];
            zone.cur[point] = zone.org[point];
        }
        let now = self.project(self.zone(0).cur[point]);
        let far = value.wrapping_sub(now).wrapping_abs() > self.state.cutin;
        let distance = if op & 1 == 0 { value } else { self.rounded(if far { now } else { value }, 0) };
        self.shift(0, point, distance.wrapping_sub(now));
        self.state.rp[..2].fill(point as u16);
    }

    /// `MSIRP`: a point moved `distance` from rp0 (a twilight point is first put there).
    fn relative_stretch(&mut self, op: u8, point: usize, distance: i32) {
        let rp0 = self.state.rp[0] as usize;
        if !self.has(1, point) || !self.has(0, rp0) {
            return;
        }
        if self.state.zones[1] == 0 {
            let origin = self.zone(0).org[rp0];
            self.zone_mut(1).org[point] = origin;
            self.shift_original(1, point, distance);
            let zone = self.zone_mut(1);
            zone.cur[point] = zone.org[point];
        }
        let now = self.project(minus(self.zone(1).cur[point], self.zone(0).cur[rp0]));
        self.shift(1, point, distance.wrapping_sub(now));
        self.state.rp[1..].copy_from_slice(&[rp0 as u16, point as u16]);
        if op & 1 != 0 {
            self.state.rp[0] = point as u16;
        }
    }

    /// `MDRP`: a point moved to its original distance from rp0: rounded (`op & 4`), at least
    /// the minimum (`op & 8`), and making it rp0 (`op & 16`).
    fn direct_relative(&mut self, op: u8, point: usize) {
        let rp0 = self.state.rp[0] as usize;
        if self.has(1, point) && self.has(0, rp0) {
            let twilight = self.state.zones[0] == 0 || self.state.zones[1] == 0;
            let (mut original, width, cutin) = (self.original((1, point), (0, rp0), twilight), self.state.width, self.state.width_cutin);
            if cutin > 0 && original < width.wrapping_add(cutin) && original > width.wrapping_sub(cutin) {
                original = if original >= 0 { width } else { width.wrapping_neg() };
            }
            let distance = if op & 4 != 0 { self.rounded(original, 0) } else { rounded(Rounding::Off, original, 0, [0; 3]) };
            let distance = if op & 8 != 0 { at_least(original, distance, self.state.min_distance) } else { distance };
            let now = self.project(minus(self.zone(1).cur[point], self.zone(0).cur[rp0]));
            self.shift(1, point, distance.wrapping_sub(now));
        }
        self.state.rp[1..].copy_from_slice(&[rp0 as u16, point as u16]);
        if op & 16 != 0 {
            self.state.rp[0] = point as u16;
        }
    }

    /// `MIRP`: a point moved to a control value's distance from rp0 (entry -1: 0), flipped to
    /// the original's side (auto flip), the original distance where the value is beyond the
    /// cut-in; then as `MDRP`.
    fn indirect_relative(&mut self, op: u8, point: usize, entry: i32) {
        let (rp0, entry) = (self.state.rp[0] as usize, entry.wrapping_add(1) as u32 as usize);
        if self.has(1, point) && entry <= self.values.len() && self.has(0, rp0) {
            let value = if entry == 0 { 0 } else { self.values[entry - 1] };
            self.indirect_move(op, point, rp0, value);
        }
        self.state.rp[1..].copy_from_slice(&[rp0 as u16, point as u16]);
        if op & 16 != 0 {
            self.state.rp[0] = point as u16;
        }
    }

    /// The move of `MIRP`, by a control value.
    fn indirect_move(&mut self, op: u8, point: usize, rp0: usize, value: i32) {
        let (width, free) = (self.state.width, self.state.free);
        let mut value = if value.wrapping_sub(width).wrapping_abs() < self.state.width_cutin { if value >= 0 { width } else { width.wrapping_neg() } } else { value };
        if self.state.zones[1] == 0 {
            let origin = self.zone(0).org[rp0];
            let zone = self.zone_mut(1);
            zone.org[point] = [origin[0].wrapping_add(mul_14(value, free[0])), origin[1].wrapping_add(mul_14(value, free[1]))];
            zone.cur[point] = zone.org[point];
        }
        let original = self.dual(minus(self.zone(1).org[point], self.zone(0).org[rp0]));
        let now = self.project(minus(self.zone(1).cur[point], self.zone(0).cur[rp0]));
        if self.state.auto_flip && (original ^ value) < 0 {
            value = value.wrapping_neg();
        }
        let far = self.state.zones[0] == self.state.zones[1] && value.wrapping_sub(original).wrapping_abs() > self.state.cutin;
        let distance = if op & 4 != 0 { self.rounded(if far { original } else { value }, 0) } else { rounded(Rounding::Off, value, 0, [0; 3]) };
        let distance = if op & 8 != 0 { at_least(original, distance, self.state.min_distance) } else { distance };
        self.shift(1, point, distance.wrapping_sub(now));
    }

    /// `ALIGNRP`: points moved onto rp0 along the projection vector.
    fn align_to_reference(&mut self) {
        let (points, rp0) = (self.looped(), self.state.rp[0] as usize);
        if !self.has(0, rp0) {
            return;
        }
        for point in points.into_iter().map(|point| point as u16 as usize) {
            if !self.has(1, point) {
                continue;
            }
            let distance = self.project(minus(self.zone(1).cur[point], self.zone(0).cur[rp0]));
            self.shift(1, point, distance.wrapping_neg());
        }
    }

    /// `IP`: points placed between rp1 and rp2 as they were originally.
    fn interpolate(&mut self) {
        let (points, rp1, rp2) = (self.looped(), self.state.rp[1] as usize, self.state.rp[2] as usize);
        if !self.has(0, rp1) {
            return;
        }
        let twilight = self.state.zones.contains(&0);
        let first = |zone: &Zone, point: usize| if twilight { zone.org[point] } else { zone.orus[point] };
        let (base, now) = (first(self.zone(0), rp1), self.zone(0).cur[rp1]);
        let (before, after) = match self.has(1, rp2) {
            true => (self.dual(minus(first(self.zone(1), rp2), base)), self.project(minus(self.zone(1).cur[rp2], now))),
            false => (0, 0),
        };
        for point in points.into_iter().map(|point| point as u32 as usize) {
            if !self.has(2, point) {
                continue;
            }
            let original = self.dual(minus(first(self.zone(2), point), base));
            let current = self.project(minus(self.zone(2).cur[point], now));
            let wanted = match (original, before) {
                (0, _) => 0,
                (_, 0) => original,
                _ => mul_div(original, after, before, true),
            };
            self.shift(2, point, wanted.wrapping_sub(current));
        }
    }

    /// How far the reference point (rp1 in zone 0 with `op & 1`, else rp2 in zone 1) has moved
    /// along the projection vector, as a move by the freedom vector; with its zone and number.
    fn displacement(&self, op: u8) -> Option<(Pos, usize, usize)> {
        let (pointer, point) = if op & 1 != 0 { (0, self.state.rp[1] as usize) } else { (1, self.state.rp[2] as usize) };
        self.has(pointer, point).then_some(())?;
        let zone = self.zone(pointer);
        let d = self.project(minus(zone.cur[point], zone.org[point]));
        Some(([mul_fix(d, self.moves[0]), mul_fix(d, self.moves[1])], pointer, point))
    }

    /// `SHP`: points shifted as far as the reference point moved.
    fn shift_points(&mut self, op: u8) {
        let looping = self.state.looping;
        if self.new_top < looping as usize {
            return self.state.looping = 1;
        }
        let Some((d, _, _)) = self.displacement(op) else { return self.new_top -= looping as usize };
        for point in self.looped().into_iter().map(|point| point as u16 as usize) {
            if self.has(2, point) {
                self.move_by(point, d, true);
            }
        }
    }

    /// `SHC`: a contour shifted as far as the reference point moved (but for that point).
    fn shift_contour(&mut self, op: u8, contour: usize) {
        let twilight = self.state.zones[2] == 0;
        let ends = &self.zone(2).ends;
        if contour >= if twilight { 1 } else { ends.len() } {
            return;
        }
        let start = if contour == 0 { 0 } else { ends[contour - 1] as usize + 1 };
        let limit = if twilight { self.zone(2).cur.len().min(self.twilight) } else { ends[contour] as usize + 1 };
        let Some((d, pointer, reference)) = self.displacement(op) else { return };
        let same = self.state.zones[pointer] == self.state.zones[2];
        self.work += limit.saturating_sub(start) as u64;
        for point in start..limit {
            if self.has(2, point) && !(same && point == reference) {
                self.move_by(point, d, true);
            }
        }
    }

    /// `SHZ`: zone 2 shifted as far as the reference point moved, phantom points and the
    /// reference point aside, untouched.
    fn shift_zone(&mut self, op: u8, zone: i32) {
        if zone as u32 >= 2 {
            return;
        }
        let Some((d, pointer, reference)) = self.displacement(op) else { return };
        let limit = match self.state.zones[2] {
            0 => self.size.twilight.cur.len().min(self.twilight),
            _ => self.glyph.ends.last().map_or(0, |end| *end as usize + 1),
        };
        let same = self.state.zones[pointer] == self.state.zones[2];
        self.work += limit as u64;
        for point in 0..limit {
            if self.has(2, point) && !(same && point == reference) {
                self.move_by(point, d, false);
            }
        }
    }

    /// `SHPIX`: points shifted by a distance along the freedom vector. With backward
    /// compatibility, vertically only, and only twilight points, parts of a composite or
    /// points already touched vertically.
    fn shift_pixels(&mut self, amount: i32) {
        let twilight = self.state.zones.contains(&0);
        let (free, compatible) = (self.state.free, self.compatible);
        let d = [mul_14(amount, free[0]), mul_14(amount, free[1])];
        for point in self.looped().into_iter().map(|point| point as u16 as usize) {
            if !self.has(2, point) {
                continue;
            }
            let touched = self.zone(2).tags[point] & TOUCH_Y != 0;
            let allowed = twilight || (compatible != SETTLED && ((self.composite && free[1] != 0) || touched));
            match compatible {
                0 => self.move_by(point, d, true),
                _ if allowed => self.move_by(point, [0, d[1]], true),
                _ => {}
            }
        }
    }

    /// `IUP`: the points no instruction touched along an axis (x with `op & 1`) follow the
    /// touched ones around them on their contour: shifted beyond them, interpolated between.
    fn interpolate_untouched(&mut self, op: u8) {
        if self.compatible == SETTLED {
            return;
        }
        if self.compatible != 0 {
            self.compatible |= 1 << (op & 1);
        }
        let (axis, mask, count) = if op & 1 != 0 { (0, TOUCH_X, self.glyph.cur.len()) } else { (1, TOUCH_Y, self.glyph.cur.len()) };
        let mut first = 0;
        for contour in 0..self.glyph.ends.len() {
            let end = (self.glyph.ends[contour] as usize).min(count.saturating_sub(1));
            let touched: Vec<usize> = (first..=end).filter(|point| self.glyph.tags[*point] & mask != 0).collect();
            self.follow(axis, [first, end], &touched);
            first = first.max(end + 1);
        }
        self.work += count as u64;
    }

    /// Interpolates one contour's untouched points, from `first` to `end`, between the touched
    /// ones; with one touched, shifts them all with it.
    fn follow(&mut self, axis: usize, [first, end]: [usize; 2], touched: &[usize]) {
        let (Some(&head), Some(&last)) = (touched.first(), touched.last()) else { return };
        for pair in touched.windows(2) {
            self.between(axis, [pair[0] + 1, pair[1] - 1], [pair[0], pair[1]]);
        }
        if head == last {
            let zone = &mut self.glyph;
            let by = zone.cur[head][axis].wrapping_sub(zone.org[head][axis]);
            let moved = (first..=end).filter(|point| *point != head);
            return moved.for_each(|point| zone.cur[point][axis] = zone.cur[point][axis].wrapping_add(by));
        }
        self.between(axis, [last + 1, end], [last, head]);
        if head > 0 {
            self.between(axis, [first, head - 1], [last, head]);
        }
    }

    /// Points `from` to `to` interpolated along an axis between two touched ones: shifted as
    /// the nearer one where outside them, scaled between them (FreeType's
    /// `iup_worker_interpolate_`, which scales by the font units).
    fn between(&mut self, axis: usize, [from, to]: [usize; 2], [one, two]: [usize; 2]) {
        let zone = &mut self.glyph;
        if from > to || one >= zone.cur.len() || two >= zone.cur.len() {
            return;
        }
        let (one, two) = if zone.orus[one][axis] > zone.orus[two][axis] { (two, one) } else { (one, two) };
        let ([orus1, orus2], [org1, org2], [cur1, cur2]) = ([zone.orus[one][axis], zone.orus[two][axis]], [zone.org[one][axis], zone.org[two][axis]], [zone.cur[one][axis], zone.cur[two][axis]]);
        let flat = cur1 == cur2 || orus1 == orus2;
        let scale = if flat { 0 } else { div_fix(cur2.wrapping_sub(cur1), orus2.wrapping_sub(orus1)) };
        for point in from..=to.min(zone.cur.len() - 1) {
            let x = zone.org[point][axis];
            zone.cur[point][axis] = match () {
                _ if x <= org1 => x.wrapping_add(cur1.wrapping_sub(org1)),
                _ if x >= org2 => x.wrapping_add(cur2.wrapping_sub(org2)),
                _ if flat => cur1,
                _ => cur1.wrapping_add(mul_fix(zone.orus[point][axis].wrapping_sub(orus1), scale)),
            };
        }
    }

    /// `DELTAP1` to `DELTAP3`: points moved by small amounts at one size each.
    fn delta_points(&mut self, op: u8, count: i32) {
        let pairs = self.deltas(op, count);
        let (compatible, composite, free) = (self.compatible, self.composite, self.state.free);
        for (point, by) in pairs.into_iter().map(|(point, by)| (point as u16 as usize, by)) {
            let touched = self.has(0, point) && self.zone(0).tags[point] & TOUCH_Y != 0;
            if !self.has(0, point) {
                continue;
            }
            if compatible == 0 || (compatible != SETTLED && ((composite && free[1] != 0) || touched)) {
                self.shift(0, point, by);
            }
        }
    }

    /// `DELTAC1` to `DELTAC3`: control values changed by small amounts at one size each.
    fn delta_values(&mut self, op: u8, count: i32) {
        for (entry, by) in self.deltas(op, count) {
            if let Some(value) = self.values.get_mut(entry as u32 as usize) {
                *value = value.wrapping_add(by);
            }
        }
    }

    /// The pairs a delta instruction takes: each a point or control value and an argument naming
    /// a size and a step, kept where the size is this one: with the step's amount. (The second
    /// and third of each kind count sizes from 16 and 32 past the delta base.)
    fn deltas(&mut self, op: u8, count: i32) -> Vec<(i32, i32)> {
        let half = self.new_top / 2;
        let count = if count < 0 || count as usize > half { half } else { count as usize };
        self.new_top -= 2 * count;
        let size = self.size.ppem - self.state.delta_base as i32 - [0, 16, 32][matches!(op, 0x71 | 0x74) as usize + 2 * matches!(op, 0x72 | 0x75) as usize];
        if size & !0xf != 0 {
            return Vec::new();
        }
        let step = 1 << (6 - self.state.delta_shift.min(6));
        let pairs = (0..count).map(|n| (self.below(1 + 2 * n), self.below(2 + 2 * n)));
        pairs.filter(|(_, arg)| arg & 0xf0 == size << 4).map(|(at, arg)| (at, ((arg & 0xf) - 8 + (arg & 0xf >= 8) as i32) * step)).collect()
    }

    /// `SROUND`, `S45ROUND`: rounding to a period, phase and threshold.
    fn super_round(&mut self, op: u8, selector: i32) {
        let grid = if op == 0x76 { 0x4000 } else { 0x2d41 };
        let period = match selector & 0xc0 {
            0 => grid / 2,
            0x80 => grid * 2,
            _ => grid,
        };
        let phase = [0, period / 4, period / 2, period * 3 / 4][(selector as usize & 0x30) >> 4];
        let threshold = if selector & 0x0f == 0 { period - 1 } else { ((selector & 0x0f) - 4) * period / 8 };
        (self.period, self.phase, self.threshold) = (period >> 8, phase >> 8, threshold >> 8);
        self.round = if op == 0x76 { Rounding::Super } else { Rounding::Super45 };
    }

    /// `FLIPPT`: points' on-curve flags flipped (not once settled).
    fn flip_points(&mut self) {
        let points = self.looped();
        if self.compatible == SETTLED {
            return;
        }
        for point in points.into_iter().map(|point| point as u16 as usize) {
            if let Some(tags) = self.glyph.tags.get_mut(point) {
                *tags ^= 1;
            }
        }
    }

    /// `FLIPRGON`, `FLIPRGOFF`: a range of points made on- or off-curve.
    fn flip_range(&mut self, op: u8, last: usize, first: usize) {
        if self.compatible == SETTLED || last >= self.glyph.cur.len() || first >= self.glyph.cur.len() {
            return;
        }
        for tags in self.glyph.tags.get_mut(first..=last).unwrap_or_default() {
            *tags = if op == 0x81 { *tags | 1 } else { *tags & !1 };
        }
    }

    /// `SCANCTRL`: dropout control, by size.
    fn scan_control(&mut self, value: i32) {
        let (threshold, ppem) = (value & 0xff, self.size.ppem);
        self.state.scan_control = match threshold {
            0xff => true,
            0 => false,
            _ if value & 0x800 != 0 && ppem > threshold => false,
            _ if value & 0x100 != 0 && ppem <= threshold => true,
            _ => self.state.scan_control,
        };
    }

    /// `GETINFO`: as FreeType's version 40 answers for grey, light hinting: the version, a
    /// variable font, subpixel hinting, subpixel positioning, symmetrical smoothing, and
    /// ClearType's greyscale.
    fn info(&self, asked: i32) -> i32 {
        let answers = [(1, 40), (8, if self.size.axes > 0 { 1 << 10 } else { 0 }), (64, 1 << 13), (1024, 1 << 17), (2048, 1 << 18), (4096, 1 << 19)];
        answers.iter().filter(|(bit, _)| asked & bit != 0).fold(0, |info, (_, answer)| info | answer)
    }

    /// `INSTCTRL`: in `prep`, the instruction control flags (1: don't hint, 2: glyphs start from
    /// the default state, 4: native ClearType); in a glyph, only native ClearType for it.
    fn control(&mut self, value: u32, selector: u32) {
        let flag = [0, 1, 2, 4].get(selector as usize).copied().unwrap_or(0);
        if flag == 0 || (value != 0 && value != flag) {
            return;
        }
        match (self.first, selector) {
            (2, _) => self.state.control = self.state.control & !(flag as u8) | value as u8,
            (3, 3) => self.compatible = (value as u8 & 4) ^ COMPATIBLE,
            _ => {}
        }
    }

    /// `GETVARIATION`: the normalized position on each axis.
    fn variation(&mut self) -> Ran {
        let axes = self.size.axes;
        (axes < self.stack.len() + 1 - self.top).then_some(())?;
        for axis in 0..axes {
            let at = self.size.place.get(axis).copied().unwrap_or(0);
            self.give(axis, at);
        }
        self.new_top += axes;
        Some(())
    }
}

/// A glyph's outline as it is loaded: its points (26.6 pixels, y up), their tags, and where
/// each contour ends (after its last point).
#[derive(Default)]
struct Outline {
    points: Vec<Pos>,
    tags: Vec<u8>,
    ends: Vec<usize>,
}

/// A component of a composite glyph: its flags, glyph, the offset or the two points to match,
/// and its 2×2 transform (16.16: xx, yx, xy, yy).
#[derive(Clone, Copy, Debug)]
struct Part {
    flags: u16,
    glyph: u16,
    args: Pos,
    matrix: [i32; 4],
}

impl Part {
    /// The component at `at`, and the next one's offset.
    fn read(data: Bytes, at: usize) -> Option<(Part, usize)> {
        let (flags, glyph) = (data.u16(at)?, data.u16(at + 2)?);
        let args = match (flags & WORDS != 0, flags & OFFSET != 0) {
            (true, true) => [data.i16(at + 4)? as i32, data.i16(at + 6)? as i32],
            (true, false) => [data.u16(at + 4)? as i32, data.u16(at + 6)? as i32],
            (false, true) => [data.u8(at + 4)? as i8 as i32, data.u8(at + 5)? as i8 as i32],
            (false, false) => [data.u8(at + 4)? as i32, data.u8(at + 5)? as i32],
        };
        let at = at + if flags & WORDS != 0 { 8 } else { 6 };
        let number = |n: usize| data.i16(at + 2 * n).map(|number| number as i32 * 4);
        let (matrix, long) = match () {
            _ if flags & SCALE != 0 => ([number(0)?, 0, 0, number(0)?], 2),
            _ if flags & SCALES != 0 => ([number(0)?, 0, 0, number(1)?], 4),
            _ if flags & MATRIX != 0 => ([number(0)?, number(1)?, number(2)?, number(3)?], 8),
            _ => ([0x10000, 0, 0, 0x10000], 0),
        };
        Some((Part { flags, glyph, args, matrix }, at + long))
    }
}

/// A composite glyph being loaded: its components, how many were started, where its points
/// and contours start, its bytes and where its instructions are; and the component loading:
/// where its points start, and the phantom points before it.
struct Frame<'f> {
    glyph: u16,
    parts: Vec<Part>,
    next: usize,
    first: usize,
    contours: usize,
    data: Bytes<'f>,
    after: usize,
    loading: Option<(usize, [Pos; 4])>,
}

/// Loads a glyph as FreeType does: each simple glyph scaled and hinted, composites built of
/// them and then hinted as a whole; with copies of the control values and storage that live as
/// long as the glyph.
struct Loader<'s, 'f> {
    size: &'s mut Sized,
    file: &'f FontFile,
    font: &'f Font<'f>,
    outline: Outline,
    /// The phantom points: the advance's start and end, the top and bottom (26.6).
    phantom: [Pos; 4],
    values: Vec<i32>,
    storage: Vec<i32>,
    compatible: u8,
}

impl<'s, 'f> Loader<'s, 'f> {
    fn new(size: &'s mut Sized, file: &'f FontFile, font: &'f Font<'f>) -> Loader<'s, 'f> {
        let (values, storage, compatible) = (size.values.clone(), size.storage.clone(), size.compatible);
        Loader { size, file, font, outline: Outline::default(), phantom: [[0; 2]; 4], values, storage, compatible }
    }

    /// Loads and hints a glyph with its parts (nested composites wait on a stack). Gives the
    /// advance's start, which the outline is then moved left by.
    fn load(&mut self, glyph: u16) -> Option<i32> {
        let mut frames: Vec<Frame> = self.start(glyph)?.into_iter().collect();
        while let Some(frame) = frames.last_mut() {
            self.placed(frame)?;
            let Some(next) = frame.parts.get(frame.next).map(|part| part.glyph) else {
                let frame = frames.pop()?;
                self.finish(frame)?;
                continue;
            };
            frame.next += 1;
            frame.loading = Some((self.outline.points.len(), self.phantom));
            (frames.len() < DEEPEST_PARTS && !frames.iter().any(|frame| frame.glyph == next)).then_some(())?;
            frames.extend(self.start(next)?);
        }
        Some(self.phantom[0][0])
    }

    /// Starts on a glyph: its metrics give its phantom points; a simple glyph is hinted and
    /// added at once; a composite gives its components, to load one by one.
    fn start(&mut self, glyph: u16) -> Option<Option<Frame<'f>>> {
        ((glyph as usize) < self.font.glyphs).then_some(())?;
        let data = self.file.points(glyph).unwrap_or(Bytes(&[]));
        let contours = if data.0.is_empty() { 0 } else { data.i16(0)? };
        let [low, high] = if contours == 0 { [0, 0] } else { [data.i16(2)? as i32, data.i16(8)? as i32] };
        let [advance, left] = Font::metric(self.font.widths, glyph);
        let advance = if self.font.hvar { self.file.width(glyph).round() as i32 } else { advance };
        let [tall, top] = if self.font.heights.long > 0 { Font::metric(self.font.heights, glyph) } else { [(self.font.column[0] - self.font.column[1]).abs(), self.font.column[0] - high] };
        let phantom = [[low - left, 0], [low - left + advance, 0], [advance / 2, high + top], [advance / 2, high + top - tall]];
        match contours {
            0 => self.empty(glyph, phantom).map(|_| None),
            1.. => self.simple(glyph, data, contours as usize, phantom).map(|_| None),
            _ => self.composite(glyph, data, phantom).map(Some),
        }
    }

    /// The deltas a variable font instance moves `points` and the four phantom points after
    /// them by (in font units; none at the default): as FreeType, the phantom points stay
    /// where `HVAR` and `VVAR` give the metrics instead.
    fn varied(&self, glyph: u16, points: &[Pos], ends: &[u16]) -> Option<Option<Vec<[f32; 2]>>> {
        let Some(varied) = &self.file.varied else { return Some(None) };
        let original = points.iter().map(|at| pt(at[0] as f32, at[1] as f32));
        let mut moved: Vec<[f32; 2]> = varied.moved(self.file.bytes(), glyph, original, ends.iter().map(|end| *end as usize))?.iter().map(|by| [by.x, by.y]).collect();
        let n = points.len();
        (moved.len() == n + 4).then_some(())?;
        for (phantom, kept) in [(n, self.font.hvar), (n + 1, self.font.hvar), (n + 2, self.font.vvar), (n + 3, self.font.vvar)] {
            moved[phantom] = if kept { [0.0; 2] } else { moved[phantom] };
        }
        Some(Some(moved))
    }

    /// A glyph with no outline (a space): only its phantom points.
    fn empty(&mut self, glyph: u16, phantom: [Pos; 4]) -> Option<()> {
        let moved = self.varied(glyph, &[], &[])?.unwrap_or(vec![[0.0; 2]; 4]);
        let units = std::array::from_fn::<Pos, 4, _>(|n| [0, 1].map(|axis| phantom[n][axis] + (moved[n][axis] + 0.5).floor() as i32));
        self.phantom = units.map(|at| at.map(|value| mul_fix(value, self.size.scale)));
        Some(())
    }

    /// A simple glyph: its points read, varied, scaled and hinted, and added to the outline.
    /// It lists each contour's last point, its instructions, a flag byte per point (repeated
    /// by a flag), then all x deltas and all y deltas, each one or two bytes or none by flags.
    fn simple(&mut self, glyph: u16, data: Bytes<'f>, contours: usize, phantom: [Pos; 4]) -> Option<()> {
        (contours < 0xfff).then_some(())?;
        let ends: Vec<u16> = (0..contours).map(|n| data.u16(10 + 2 * n)).collect::<Option<_>>()?;
        (ends.windows(2).all(|pair| pair[0] < pair[1])).then_some(())?;
        let count = ends.last().map_or(0, |end| *end as usize + 1);
        (self.outline.points.len() + count <= MOST_POINTS).then_some(())?;
        let long = data.u16(10 + 2 * contours)? as usize;
        let code = data.part(12 + 2 * contours, long)?.0;
        let (mut tags, mut at) = (Vec::with_capacity(count + 4), 12 + 2 * contours + long);
        while tags.len() < count {
            let (flags, again) = (data.u8(at)?, data.u8(at + 1));
            let times = if flags & 8 != 0 { again? as usize + 1 } else { 1 };
            (tags.len() + times <= count).then_some(())?;
            tags.extend(std::iter::repeat_n(flags, times));
            at += if flags & 8 != 0 { 2 } else { 1 };
        }
        let mut points = vec![[0; 2]; count];
        for axis in 0..2 {
            let mut value = 0i32;
            for (point, flags) in points.iter_mut().zip(&tags) {
                let (step, long) = step(data, at, flags >> (1 + axis) & 1 != 0, flags >> (4 + axis) & 1 != 0)?;
                (value, at) = (value.wrapping_add(step), at + long);
                point[axis] = value;
            }
        }
        let moved = self.varied(glyph, &points, &ends)?;
        points.extend(phantom);
        tags.iter_mut().for_each(|tags| *tags &= 1);
        tags.extend([0; 4]);
        let scale = self.size.scale;
        // At the default each point is scaled; an instance's points are kept to 1/64 unit
        // before scaling, and rounded to whole units as the originals instructions measure.
        let (orus, cur): (Vec<Pos>, Vec<Pos>) = match moved {
            None => points.iter().map(|at| (*at, at.map(|value| mul_fix(value, scale)))).unzip(),
            Some(moved) => points.iter().zip(moved).map(|(at, by)| varied_point(*at, by, scale)).unzip(),
        };
        self.phantom = [cur[count], cur[count + 1], cur[count + 2], cur[count + 3]];
        let mut zone = Zone { orus, org: cur.clone(), cur, tags, ends };
        self.hint(&mut zone, code, false);
        let first = self.outline.points.len();
        self.outline.points.extend_from_slice(&zone.cur[..count]);
        self.outline.tags.extend_from_slice(&zone.tags[..count]);
        self.outline.ends.extend(zone.ends.iter().map(|end| first + *end as usize + 1));
        Some(())
    }

    /// A composite glyph: its components read, their offsets varied, its phantom points
    /// scaled.
    fn composite(&mut self, glyph: u16, data: Bytes<'f>, phantom: [Pos; 4]) -> Option<Frame<'f>> {
        let (mut parts, mut at) = (Vec::new(), 10);
        loop {
            let (part, next) = Part::read(data, at)?;
            ((part.glyph as usize) < self.font.glyphs && parts.len() < MOST_POINTS).then_some(())?;
            parts.push(part);
            at = next;
            if part.flags & MORE == 0 {
                break;
            }
        }
        let offsets: Vec<Pos> = parts.iter().map(|part| part.args).collect();
        let moved = self.varied(glyph, &offsets, &[])?.unwrap_or(vec![[0.0; 2]; parts.len() + 4]);
        let round = |at: Pos, by: [f32; 2]| [0, 1].map(|axis| at[axis] + (by[axis] + 0.5).floor() as i32);
        for (part, by) in parts.iter_mut().zip(&moved).filter(|(part, _)| part.flags & OFFSET != 0) {
            part.args = round(part.args, *by);
        }
        let units = std::array::from_fn::<Pos, 4, _>(|n| round(phantom[n], moved[parts.len() + n]));
        self.phantom = units.map(|at| at.map(|value| mul_fix(value, self.size.scale)));
        Some(Frame { glyph, parts, next: 0, first: self.outline.points.len(), contours: self.outline.ends.len(), data, after: at, loading: None })
    }

    /// After a component is loaded: the phantom points back (unless it gives the metrics),
    /// and its points transformed and offset (by its offset, scaled; or so that its point
    /// matches one of the components before).
    fn placed(&mut self, frame: &mut Frame) -> Option<()> {
        let Some((base, phantom)) = frame.loading.take() else { return Some(()) };
        let part = frame.parts[frame.next - 1];
        if part.flags & OWN_METRICS == 0 {
            self.phantom = phantom;
        }
        if self.outline.points.len() == base {
            return Some(());
        }
        let ([xx, yx, xy, yy], scaled) = (part.matrix, part.flags & (SCALE | SCALES | MATRIX) != 0);
        if scaled {
            let turn = |at: Pos| [mul_fix(at[0], xx).wrapping_add(mul_fix(at[1], xy)), mul_fix(at[0], yx).wrapping_add(mul_fix(at[1], yy))];
            self.outline.points[base..].iter_mut().for_each(|at| *at = turn(*at));
        }
        let by = match part.flags & OFFSET != 0 {
            false => {
                let (onto, own) = (frame.first + part.args[0] as usize, base + part.args[1] as usize);
                (onto < base && own < self.outline.points.len()).then_some(())?;
                minus(self.outline.points[onto], self.outline.points[own])
            }
            true => self.offset(part, scaled),
        };
        self.outline.points[base..].iter_mut().for_each(|at| *at = [at[0].wrapping_add(by[0]), at[1].wrapping_add(by[1])]);
        Some(())
    }

    /// A component's offset in 26.6: scaled by the component's own scale where flagged, then
    /// by the size, and rounded to whole pixels where flagged (vertically only, with backward
    /// compatibility).
    fn offset(&self, part: Part, scaled: bool) -> Pos {
        let [xx, yx, xy, yy] = part.matrix;
        let length = |a: i32, b: i32| (a as f64).hypot(b as f64).round() as i32;
        let [x, y] = match scaled && part.flags & SCALED_OFFSET != 0 {
            true => [mul_fix(part.args[0], length(xx, xy)), mul_fix(part.args[1], length(yy, yx))],
            false => part.args,
        };
        let [x, y] = [mul_fix(x, self.size.scale), mul_fix(y, self.size.scale)];
        match part.flags & ROUND_OFFSET != 0 {
            true => [if self.compatible == 0 { x.wrapping_add(32) & -64 } else { x }, y.wrapping_add(32) & -64],
            false => [x, y],
        }
    }

    /// After a composite's components: its own instructions, if it has any, run on all their
    /// points, measured as they are now.
    fn finish(&mut self, frame: Frame<'f>) -> Option<()> {
        let first = frame.first;
        if frame.parts.last()?.flags & INSTRUCTED == 0 || self.outline.points.len() <= first {
            return Some(());
        }
        let long = frame.data.u16(frame.after)? as usize;
        (long <= frame.data.0.len()).then_some(())?;
        let code = frame.data.part(frame.after + 2, long)?.0;
        if code.is_empty() {
            return Some(());
        }
        let cur = [&self.outline.points[first..], &self.phantom[..]].concat();
        let tags = self.outline.tags[first..].iter().map(|tags| tags & !(TOUCH_X | TOUCH_Y)).chain([0; 4]).collect();
        let ends = self.outline.ends[frame.contours..].iter().map(|end| (end - 1 - first) as u16).collect();
        let mut zone = Zone { orus: cur.clone(), org: cur.clone(), cur, tags, ends };
        self.hint(&mut zone, code, true);
        let count = zone.cur.len() - 4;
        self.outline.points[first..].copy_from_slice(&zone.cur[..count]);
        self.outline.tags[first..].copy_from_slice(&zone.tags[..count]);
        Some(())
    }

    /// Hints a glyph's points (`zone`, its four phantom points last): the phantom points
    /// rounded, then its instructions run. Without backward compatibility the phantom points
    /// it moved give the metrics.
    fn hint(&mut self, zone: &mut Zone, code: &'f [u8], composite: bool) {
        let n = zone.cur.len();
        for (back, axis) in [(4, 0), (3, 0), (2, 1), (1, 1)] {
            zone.cur[n - back][axis] = zone.cur[n - back][axis].wrapping_add(32) & -64;
        }
        if !code.is_empty() {
            let (values, storage, glyph) = (std::mem::take(&mut self.values), std::mem::take(&mut self.storage), std::mem::take(zone));
            let mut machine = Machine::with(self.size, [self.font.functions, self.font.control, code], glyph, (values, storage), composite);
            machine.compatible = self.compatible;
            _ = machine.run(3);
            (*zone, self.values, self.storage, self.compatible) = (machine.glyph, machine.values, machine.storage, machine.compatible);
        }
        if self.compatible == 0 {
            self.phantom = [zone.cur[n - 4], zone.cur[n - 3], zone.cur[n - 2], zone.cur[n - 1]];
        }
    }
}

/// One coordinate delta of a `glyf` point at `at`, and its length: one byte (`short`, `same`
/// its sign), none (`same`: zero), or two.
fn step(data: Bytes, at: usize, short: bool, same: bool) -> Option<(i32, usize)> {
    Some(match (short, same) {
        (true, true) => (data.u8(at)? as i32, 1),
        (true, false) => (-(data.u8(at)? as i32), 1),
        (false, true) => (0, 0),
        (false, false) => (data.i16(at)? as i32, 2),
    })
}

/// A point of a variable font instance: its font units moved by `by`, rounded to whole units
/// (as instructions measure them), and scaled from 1/64 units (as FreeType keeps them).
fn varied_point(at: Pos, by: [f32; 2], scale: i32) -> (Pos, Pos) {
    let units = [0, 1].map(|axis| at[axis] + (by[axis] + 0.5).floor() as i32);
    let fine = [0, 1].map(|axis| at[axis] * 64 + (by[axis] * 64.0 + 0.5).floor() as i32);
    (units, fine.map(|value| mul_fix(value, scale).wrapping_add(32) >> 6))
}

/// A font's programs run for each size its glyphs are drawn at: for the cache of one size,
/// made when its first glyph is.
#[derive(Default)]
pub(crate) struct Hints {
    sized: Option<Option<Box<Sized>>>,
}

impl Hints {
    /// A glyph of `file` at `pixels` per em hinted by the font's instructions. `None` with
    /// none to run (no programs, a size too large, programs that fail): `hinting.rs` hints it
    /// then. `Some(None)` where they say to leave it unhinted.
    pub(crate) fn glyph(&mut self, file: &FontFile, pixels: f32, glyph: u16) -> Option<Option<(Path, [f32; 4])>> {
        let sized = self.sized.get_or_insert_with(|| Sized::of(file, pixels).map(Box::new)).as_mut()?;
        Some(sized.glyph(file, glyph))
    }

    /// Whether the font's instructions hint at this size.
    pub(crate) fn applies(&mut self, file: &FontFile, pixels: f32) -> bool {
        self.sized.get_or_insert_with(|| Sized::of(file, pixels).map(Box::new)).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font_file::tests::{Tables, file, glyph, longs, tables_of, with, words};

    /// A `maxp` for `glyphs` glyphs, declaring room for programs: 16 twilight points, 16
    /// storage slots, 16 functions, 256 stack values, and glyph programs of `long` bytes.
    fn maxp(glyphs: i32, long: i32) -> Vec<u8> {
        words(&[1, 0, glyphs, 64, 8, 64, 8, 2, 16, 16, 16, 16, 256, long, 4, 4])
    }

    /// A font of these glyphs (1024 units per em) with `fpgm`, `prep` and `cvt `.
    fn font(glyphs: Vec<Vec<u8>>, functions: &[u8], control: &[u8], values: &[i32]) -> FontFile {
        let count = glyphs.len() as i32;
        let mut tables: Tables = with(tables_of(glyphs), b"maxp", maxp(count, 1000));
        tables.extend([(*b"fpgm", functions.to_vec()), (*b"prep", control.to_vec()), (*b"cvt ", words(values))]);
        FontFile::read(file(&[0, 1, 0, 0], &tables)).expect("the font is read")
    }

    /// A simple glyph with these instructions.
    fn instructed(glyph: Vec<u8>, code: &[u8]) -> Vec<u8> {
        let at = 10 + 2 * i16::from_be_bytes([glyph[0], glyph[1]]) as usize;
        [&glyph[..at], &(code.len() as u16).to_be_bytes()[..], code, &glyph[at + 2..]].concat()
    }

    /// A box 500 wide, 700 tall: points 0 to 3 from its bottom left, clockwise.
    fn tall() -> Vec<u8> {
        glyph(&[&[(0, 0, true), (0, 700, true), (500, 700, true), (500, 0, true)]])
    }

    /// A glyph's hinted points (26.6, y up, x from the pen) at `pixels`.
    fn hinted(font: &FontFile, pixels: f32, glyph: u16) -> Option<Vec<Pos>> {
        let mut sized = Sized::of(font, pixels)?;
        let tables = Font::of(font)?;
        let mut loader = Loader::new(&mut sized, font, &tables);
        let shift = loader.load(glyph)?;
        Some(loader.outline.points.iter().map(|at| [at[0] - shift, at[1]]).collect())
    }

    #[test]
    fn fixed_point_rounds_as_freetype_does() {
        assert_eq!([mul_fix(3, 0x8000), mul_fix(-3, 0x8000), mul_fix(1, 0x7fff), mul_fix(-1, 0x8000)], [2, -2, 0, -1]);
        assert_eq!([mul_div(7, 1, 2, true), mul_div(-7, 1, 2, true), mul_div(7, 1, 2, false), mul_div(5, 5, 0, true)], [4, -4, 3, 0x7fff_ffff]);
        assert_eq!([div_fix(1, 3), div_fix(-1, 3), div_fix(1, 0)], [21845, -21845, 0x7fff_ffff]);
        assert_eq!([mul_14(64, 0x2000), mul_14(-3, 0x2000)], [32, -2]);
        // A unit vector is 1 in F2Dot14 to within a unit, in any direction.
        assert_eq!([unit(0, 5), unit(-7, 0), unit(0, 0)], [Some([0, 0x4000]), Some([-0x4000, 0]), None]);
        assert_eq!(unit(1, 1), Some([11585, 11585]));
        for (x, y) in [(3, 4), (-1000, 1), (12345, -54321), (1, 1 << 30), (i32::MAX, i32::MIN + 1)] {
            let [ux, uy] = unit(x, y).unwrap();
            let length = ((ux as f64).hypot(uy as f64) - 16384.0).abs();
            assert!(length < 2.0 && (ux < 0) == (x < 0) && (uy < 0) == (y < 0), "({x}, {y}) gave ({ux}, {uy})");
        }
    }

    #[test]
    fn rounding_keeps_the_sign_and_the_least_distance() {
        let none = [0; 3];
        let cases = [(Rounding::Grid, 96, 128), (Rounding::Grid, 31, 0), (Rounding::Grid, -95, -64), (Rounding::HalfGrid, 10, 32), (Rounding::HalfGrid, -70, -96)];
        let more = [(Rounding::DoubleGrid, 47, 32), (Rounding::DownToGrid, 127, 64), (Rounding::UpToGrid, 65, 128), (Rounding::Off, -5, -5), (Rounding::UpToGrid, -1, -64)];
        for (mode, distance, wanted) in cases.into_iter().chain(more) {
            assert_eq!(rounded(mode, distance, 0, none), wanted, "{mode:?} of {distance}");
        }
        // SROUND 0x48: period of a pixel, no phase, threshold at half: rounds to the grid.
        let size = Sized {
            ppem: 12,
            scale: 0,
            values: vec![],
            storage: vec![],
            twilight: Zone::default(),
            functions: vec![],
            instructions: vec![],
            most_function: 0,
            most_instruction: 0,
            state: DEFAULT,
            most_functions: 0,
            most_instructions: 0,
            stack: 8,
            glyphs: 1,
            axes: 0,
            place: vec![],
            hinted: true,
            compatible: 0,
        };
        let mut size = size;
        let mut machine = Machine::with(&mut size, [&[], &[], &[]], Zone::default(), (vec![], vec![]), false);
        machine.super_round(0x76, 0x48);
        assert_eq!([machine.rounded(96, 0), machine.rounded(31, 0), machine.rounded(-96, 0)], [128, 0, -128]);
    }

    #[test]
    fn a_glyph_program_puts_a_height_on_a_whole_pixel() {
        // SVTCA[y]; MDAP[round] points 0 and 1 (700 units: 8.89 px at 13 px per em); IUP[y].
        let code = [0x00, 0xb0, 0, 0x2f, 0xb0, 1, 0x2f, 0x30];
        let font = font(vec![glyph(&[&[(0, 0, true), (0, 10, true), (10, 0, true)]]), instructed(tall(), &code)], &[], &[0xb0, 0, 0x21], &[]);
        let points = hinted(&font, 13.0, 1).unwrap();
        assert_eq!(points.iter().map(|at| at[1]).collect::<Vec<_>>(), [0, 9 * 64, 9 * 64, 0], "the top on 9 px, the bottom kept");
        assert_eq!(points[2][0], mul_fix(500, div_fix(13 * 64, 1024)), "x as scaled");
        // As drawn: y down from the baseline.
        let (_, reach) = Sized::of(&font, 13.0).unwrap().glyph(&font, 1).unwrap();
        assert_eq!(reach[1], -9.0);
    }

    #[test]
    fn moves_along_x_wait_for_a_font_that_asks_for_them() {
        // SVTCA[x]; MDAP[round] point 2 (500 units: 6.35 px at 13 px per em).
        let code = [0x01, 0xb0, 2, 0x2f];
        let scaled = mul_fix(500, div_fix(13 * 64, 1024));
        let plain = font(vec![tall(), instructed(tall(), &code)], &[], &[0xb0, 0, 0x21], &[]);
        assert_eq!(hinted(&plain, 13.0, 1).unwrap()[2][0], scaled, "backward compatibility: x stays");
        // `prep`: PUSHB 4, 3; INSTCTRL: native ClearType.
        let native = font(vec![tall(), instructed(tall(), &code)], &[], &[0xb1, 4, 3, 0x8e], &[]);
        assert_eq!(hinted(&native, 13.0, 1).unwrap()[2][0], 6 * 64, "native: x on 6 px");
    }

    #[test]
    fn control_values_set_by_prep_and_functions_from_fpgm_are_used() {
        // fpgm: function 0 does MIRP[rp0, min] from rp0 to the point on the stack by cvt 0.
        // prep: cvt 0 set to 2 px. Glyph: SVTCA[y]; MDAP point 0; call function 0 on point 1.
        let functions = [0xb0, 0, 0x2c, 0xb0, 0, 0xf8, 0x2d];
        let control = [0xb1, 0, 128, 0x44];
        let code = [0x00, 0xb0, 0, 0x2e, 0xb1, 1, 0, 0x2b];
        let font = font(vec![tall(), instructed(tall(), &code)], &functions, &control, &[100]);
        let points = hinted(&font, 13.0, 1).unwrap();
        assert_eq!(points[1][1], 128, "point 1 two pixels above point 0, as cvt 0 says after prep");
    }

    #[test]
    fn prep_may_turn_hinting_off_at_a_size() {
        // prep: MPPEM; PUSHB 12; GT; IF; PUSHB 1, 1; INSTCTRL; EIF: no hinting above 12 px.
        let control = [0x4b, 0xb0, 12, 0x52, 0x58, 0xb1, 1, 1, 0x8e, 0x59];
        let font = font(vec![tall(), instructed(tall(), &[0x00, 0xb0, 1, 0x2f, 0x30])], &[], &control, &[]);
        assert!(Sized::of(&font, 12.0).unwrap().glyph(&font, 1).is_some(), "hinted at 12 px");
        assert!(Sized::of(&font, 13.0).unwrap().glyph(&font, 1).is_none(), "left unhinted at 13 px");
    }

    #[test]
    fn a_composite_hints_its_parts_then_runs_its_own_program() {
        // Glyph 2: glyph 1 twice, the second 200 units right, then SVTCA[y]; PUSHB 1 (point
        // 1); PUSHW -64; SHPIX: point 1 down a pixel (its part touched it vertically).
        let part = instructed(tall(), &[0x00, 0xb0, 0, 0x2f, 0xb0, 1, 0x2f, 0x30]);
        let code = [0x00, 0xb0, 1, 0xb8, 0xff, 0xc0, 0x38];
        let mut composite = words(&[-1, 0, 0, 900, 700, 0x22 | 1, 1, 0, 0, 2 | 0x100 | 1, 1, 200, 0]);
        composite.extend([&(code.len() as u16).to_be_bytes()[..], &code].concat());
        let font = font(vec![tall(), part, composite], &[], &[0xb0, 0, 0x21], &[]);
        let points = hinted(&font, 13.0, 2).unwrap();
        assert_eq!(points.len(), 8);
        assert_eq!(points.iter().map(|at| at[1]).collect::<Vec<_>>(), [0, 8 * 64, 9 * 64, 0, 0, 9 * 64, 9 * 64, 0]);
        assert_eq!(points[4][0] - points[0][0], mul_fix(200, div_fix(13 * 64, 1024)), "the second part offset as scaled");
    }

    #[test]
    fn a_program_that_never_ends_is_stopped() {
        let base = font(vec![tall(), tall()], &[], &[0xb0, 0, 0x21], &[]);
        let unhinted = hinted(&base, 13.0, 1).unwrap();
        // A glyph jumping back forever (PUSHW -3; JMPR), one calling a function that calls
        // itself, and `prep` looping forever.
        let looping = font(vec![tall(), instructed(tall(), &[0xb8, 0xff, 0xfd, 0x1c])], &[], &[0xb0, 0, 0x21], &[]);
        let since = std::time::Instant::now();
        assert_eq!(hinted(&looping, 13.0, 1).unwrap(), unhinted, "stopped before it moved anything");
        let calling = font(vec![tall(), instructed(tall(), &[0xb0, 0, 0x2b])], &[0xb0, 0, 0x2c, 0xb0, 0, 0x2b, 0x2d], &[0xb0, 0, 0x21], &[]);
        assert_eq!(hinted(&calling, 13.0, 1).unwrap(), unhinted);
        let forever = font(vec![tall(), tall()], &[], &[0xb8, 0xff, 0xfd, 0x1c], &[]);
        assert!(Sized::of(&forever, 13.0).is_none(), "a font whose prep never ends goes to the autohinter");
        assert!(since.elapsed() < std::time::Duration::from_secs(5), "took {:?}", since.elapsed());
    }

    #[test]
    fn programs_of_random_bytes_end_without_harm() {
        // Each program random; instructions that loop over points given large loop counts.
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut random = move |n: usize| {
            (0..n)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    (seed >> 24) as u8
                })
                .collect::<Vec<u8>>()
        };
        let heavy = [0xb8, 0x7f, 0xff, 0x17, 0xb0, 1, 0x36, 0xb8, 0x7f, 0xff, 0x17, 0x32, 0x30, 0x31];
        let since = std::time::Instant::now();
        for round in 0..300 {
            let (functions, control, code) = (random(64 + round % 200), random(32 + round % 100), random(16 + round % 300));
            let code = if round % 7 == 0 { [&heavy[..], &code].concat() } else { code };
            let font = font(vec![tall(), instructed(tall(), &code)], &functions, &control, &[100, -40, 3000]);
            for pixels in [9.0, 12.0, 17.0] {
                _ = Sized::of(&font, pixels).map(|mut sized| sized.glyph(&font, 1));
            }
        }
        assert!(since.elapsed() < std::time::Duration::from_secs(60), "took {:?}", since.elapsed());
    }

    #[test]
    fn a_font_without_programs_is_left_to_the_autohinter() {
        let tables: Tables = with(tables_of(vec![tall(), tall()]), b"maxp", maxp(2, 0));
        let font = FontFile::read(file(&[0, 1, 0, 0], &tables)).unwrap();
        assert!(Sized::of(&font, 12.0).is_none());
        let large = font_with_prep();
        assert!(Sized::of(&large, 12.0).is_some() && Sized::of(&large, LARGE).is_none(), "nor are large sizes hinted");
    }

    /// A font with a `prep` doing nothing.
    fn font_with_prep() -> FontFile {
        font(vec![tall(), tall()], &[], &[0xb0, 0, 0x21], &[])
    }

    #[test]
    fn a_variable_font_instance_varies_its_control_values() {
        // `cvar`: one tuple at the heaviest weight, embedded, its own point (cvt 0): +64 units.
        let cvar = [longs(&[0x0001_0000]), words(&[1, 16, 5, 0xa000, 16384, 0]), vec![1, 0, 0, 0, 64]].concat();
        let more = [(b"cvt ", words(&[100, 200])), (b"cvar", cvar), (b"prep", vec![0xb0, 0, 0x21])];
        let bytes = crate::variable_fonts::tests::variable(&more);
        let plain = FontFile::read(bytes.clone()).unwrap();
        assert_eq!(Font::of(&plain).unwrap().values(&plain), [100 * 64, 200 * 64]);
        let heavy = FontFile::read(bytes.clone()).unwrap().with_variation(b"wght", 900.0);
        assert_eq!(Font::of(&heavy).unwrap().values(&heavy), [164 * 64, 200 * 64]);
        let between = FontFile::read(bytes).unwrap().with_variation(b"wght", 650.0);
        assert_eq!(Font::of(&between).unwrap().values(&between), [132 * 64, 200 * 64]);
    }

    #[test]
    fn app_fonts_draw_with_the_fonts_instructions_unless_asked_not_to() {
        use crate::font_file::AppFonts;
        use crate::text::{Font, TextSystem};
        let Some(file) = crate::pdf::tests::latin() else { return eprintln!("no font file here: not tested") };
        let bytes = file.bytes().0.to_vec();
        let draw = |fonts: AppFonts| {
            let (mut fonts, mut pixels) = (fonts, vec![0u32; 300 * 20]);
            let mut canvas = crate::canvas::Canvas::new(&mut pixels, 300, 20, (0, 0), 1.0);
            fonts.draw(&mut canvas, "Hamburgefonstiv xzeHEsa", 2.0, 2.0, Font::new(11.0), crate::color::Color::BLACK);
            (pixels, fonts.metrics(Font::new(11.0), 1.0))
        };
        let read = || FontFile::read(bytes.clone()).unwrap();
        let ((own, metrics), (auto, _), (plain, _)) = (draw(AppFonts::new(read())), draw(AppFonts::new(read()).autohinted()), draw(AppFonts::new(read()).unhinted()));
        assert!(own != auto && own != plain && auto != plain, "three ways of drawing");
        // The cap-height is where the font's own program put the top of "H".
        let mut sized = Sized::of(&file, 11.0).unwrap();
        let (_, reach) = sized.glyph(&file, file.glyph('H')).unwrap();
        assert_eq!(metrics.cap_height, -reach[1].floor());
        // Large text is as unhinted.
        let large = |fonts: AppFonts| {
            let (mut fonts, mut pixels) = (fonts, vec![0u32; 400 * 60]);
            let mut canvas = crate::canvas::Canvas::new(&mut pixels, 400, 60, (0, 0), 1.0);
            fonts.draw(&mut canvas, "Hamburgefonstiv", 2.0, 2.0, Font::new(40.0), crate::color::Color::BLACK);
            pixels
        };
        assert_eq!(large(AppFonts::new(read())), large(AppFonts::new(read()).unhinted()));
    }

    /// Fonts to check: `TAUT_HINT_FONTS` (paths separated by `;`), else the system's.
    fn fonts() -> Vec<std::path::PathBuf> {
        if let Ok(listed) = std::env::var("TAUT_HINT_FONTS") {
            return listed.split(';').filter(|path| !path.is_empty()).map(Into::into).collect();
        }
        let mut found = Vec::new();
        let mut folders: Vec<std::path::PathBuf> = ["C:/Windows/Fonts", "/usr/share/fonts"].iter().map(Into::into).collect();
        while let Some(folder) = folders.pop() {
            for path in std::fs::read_dir(&folder).into_iter().flatten().flatten().map(|entry| entry.path()) {
                let kind = path.extension().map(|kind| kind.to_ascii_lowercase());
                match kind.as_ref().and_then(|kind| kind.to_str()) {
                    _ if path.is_dir() => folders.push(path),
                    Some("ttf" | "ttc" | "otf") => found.push(path),
                    _ => {}
                }
            }
        }
        found.sort();
        found
    }

    /// Every glyph of every font (`TAUT_HINT_FONTS`, else the system's) hinted at 8 to 20 px:
    /// nothing panics, and no glyph takes long. Prints the slowest.
    #[test]
    #[ignore]
    fn every_glyph_of_every_font_is_hinted_in_bounded_time() {
        let (mut slowest, mut glyphs, since) = ((std::time::Duration::ZERO, String::new()), 0u64, std::time::Instant::now());
        for path in fonts() {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let Some(first) = FontFile::read(bytes) else { continue };
            let faces = (1..64).map_while(|n| FontFile::read(std::fs::read(&path).ok()?)?.nth(n));
            for font in std::iter::once(first).chain(faces) {
                for pixels in 8..=20 {
                    let Some(mut sized) = Sized::of(&font, pixels as f32) else { continue };
                    for glyph in 0..font.glyph_count() {
                        let start = std::time::Instant::now();
                        _ = sized.glyph(&font, glyph);
                        let took = start.elapsed();
                        glyphs += 1;
                        if took > slowest.0 {
                            slowest = (took, format!("{} glyph {glyph} at {pixels} px", path.display()));
                        }
                    }
                }
            }
        }
        println!("{glyphs} glyphs hinted in {:?}; slowest {:?}: {}", since.elapsed(), slowest.0, slowest.1);
        assert!(slowest.0 < std::time::Duration::from_secs(2), "{} took {:?}", slowest.1, slowest.0);
    }

    /// One glyph of FreeType's dump (`tools/ftpoints`): its load error, contour ends, points.
    struct Theirs {
        error: i64,
        ends: Vec<usize>,
        points: Vec<(Pos, u8)>,
    }

    /// Reads a dump: per size, per glyph in order.
    fn dump(text: &str) -> Vec<(u32, Vec<Theirs>)> {
        let mut sizes: Vec<(u32, Vec<Theirs>)> = Vec::new();
        let mut lines = text.lines();
        while let Some(line) = lines.next() {
            let words: Vec<&str> = line.split(' ').collect();
            match words[0] {
                "s" => sizes.push((words[1].parse().unwrap(), Vec::new())),
                "g" => {
                    let ends = lines.next().unwrap().split(' ').filter(|word| !word.is_empty()).map(|word| word.parse().unwrap()).collect();
                    let numbers: Vec<i32> = lines.next().unwrap().split(' ').filter(|word| !word.is_empty()).map(|word| word.parse().unwrap()).collect();
                    let points = numbers.chunks(3).map(|point| ([point[0], point[1]], point[2] as u8)).collect();
                    sizes.last_mut().unwrap().1.push(Theirs { error: words[2].parse().unwrap(), ends, points });
                }
                _ => {}
            }
        }
        sizes
    }

    /// Every glyph of a font hinted as FreeType hints it, point for point: the font
    /// `TAUT_HINT_FONT`, against FreeType's dump `TAUT_HINT_DUMP` (from `tools/ftpoints`),
    /// glyphs loaded in the same order. Prints how many match exactly, and the first
    /// differences.
    #[test]
    #[ignore]
    fn matches_freetype_point_for_point() {
        let (Ok(path), Ok(theirs)) = (std::env::var("TAUT_HINT_FONT"), std::env::var("TAUT_HINT_DUMP")) else { return eprintln!("set TAUT_HINT_FONT and TAUT_HINT_DUMP") };
        let font = FontFile::read(std::fs::read(&path).unwrap()).unwrap();
        // (`TAUT_HINT_AXIS`: "wght=700", a variable font's position.)
        let axis = std::env::var("TAUT_HINT_AXIS").ok().and_then(|axis| Some((axis.get(..4)?.as_bytes().try_into().ok()?, axis.get(5..)?.parse::<f32>().ok()?)));
        let font = match axis {
            Some((name, at)) => font.with_variation(&name, at),
            None => font,
        };
        let (mut same, mut near, mut other, mut skipped, mut shown) = (0, 0, 0, 0, 0);
        for (pixels, glyphs) in dump(&std::fs::read_to_string(&theirs).unwrap()) {
            let Some(mut sized) = Sized::of(&font, pixels as f32) else {
                println!("{pixels} px: not hinted here");
                skipped += glyphs.len();
                continue;
            };
            let tables = Font::of(&font).unwrap();
            if !sized.hinted {
                println!("{pixels} px: prep says not to hint");
                skipped += glyphs.len();
                continue;
            }
            for (glyph, theirs) in glyphs.iter().enumerate() {
                let mut loader = Loader::new(&mut sized, &font, &tables);
                let ours = loader
                    .load(glyph as u16)
                    .map(|shift| (loader.outline.points.iter().zip(&loader.outline.tags).map(|(at, tags)| ([at[0] - shift, at[1]], tags & 1)).collect::<Vec<_>>(), loader.outline.ends.clone()));
                let ours = match (theirs.error, ours) {
                    (0, Some(ours)) => ours,
                    (0, None) | (_, Some(_)) => {
                        other += 1;
                        if shown < 20 {
                            println!("{pixels} px glyph {glyph}: FreeType's error {}, ours {}", theirs.error, if theirs.error == 0 { "failed" } else { "loaded" });
                            shown += 1;
                        }
                        continue;
                    }
                    (_, None) => {
                        same += 1;
                        continue;
                    }
                };
                let ends: Vec<usize> = ours.1.iter().map(|end| end - 1).collect();
                let apart = if ours.0.len() != theirs.points.len() || ends != theirs.ends {
                    i32::MAX
                } else {
                    ours.0.iter().zip(&theirs.points).map(|((a, on_a), (b, on_b))| if on_a != on_b { i32::MAX } else { (a[0] - b[0]).abs().max((a[1] - b[1]).abs()) }).max().unwrap_or(0)
                };
                match apart {
                    0 => same += 1,
                    1 => near += 1,
                    _ => other += 1,
                }
                if apart > 0 && shown < 20 {
                    shown += 1;
                    let first = ours.0.iter().zip(&theirs.points).position(|(a, b)| a != b);
                    println!(
                        "{pixels} px glyph {glyph}: {} points vs {}; apart {apart}; first at {first:?}: ours {:?} theirs {:?}",
                        ours.0.len(),
                        theirs.points.len(),
                        first.map(|n| ours.0[n]),
                        first.map(|n| theirs.points[n])
                    );
                }
            }
        }
        println!("{path}: {same} glyphs the same, {near} within 1/64 px, {other} differ, {skipped} not compared");
        assert!(other == 0, "{other} glyphs differ");
    }
}
