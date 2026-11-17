//! OpenType shaping: characters to the glyphs a font puts in their place, and where each goes.
//!
//! `GSUB` substitutes glyphs: an Arabic letter's joining form; two letters as one (lam-alef, a
//! two-letter flag); a family emoji. `GPOS` positions them: a vowel sign on its letter, a mark
//! on a mark, cursive attachment, kerning. Each table has features (`init`, `liga`, `mark`,
//! `kern`...) naming lookups; a lookup is a list of subtables of one type, with flags (to skip
//! marks, say). The script decides which features run, in what order, on which glyphs: Arabic's
//! forms come one feature per form, each on the letters taking it.
//!
//! All of it follows HarfBuzz, which browsers and Linux desktops shape with, and is tested
//! against it glyph for glyph: joining forms, mark reordering, lookup stages and flags,
//! contextual and chaining rules calling lookups (as deep as HarfBuzz goes), ligatures with
//! their marks' components, and `GPOS` over all that. Right-to-left text is shaped in logical
//! order and reversed at the end.
//!
//! Indic and Southeast Asian scripts, which reorder and shape by syllable, have their own
//! module, `brahmic.rs` (feature `brahmic`): it does what comes between their feature stages,
//! and here those features run, each kept to its syllable where the script asks. So do the
//! scripts HarfBuzz shapes with its Universal Shaping Engine, including those that join like
//! Arabic (N'Ko, Adlam, Mongolian and the like).
//!
//! A font lists features by script, and under each by language (`scripts.rs`): text is shaped
//! by what the font has for its letters' script (Latin by `latn`, Cyrillic by `cyrl`) and its
//! language, where the font has language forms. A character followed by a variation selector is
//! the glyph the font names for the pair (`cmap` 14): emoji or text presentation, a CJK
//! variant. Figures around a fraction slash (U+2044) become a fraction where the font can.
//!
//! Before lookups, a letter and its following accents are composed into one character the font
//! has, and a character the font lacks decomposed into ones it has (`normalization.rs`, as
//! HarfBuzz does for every script; a script's plan hands that over, so a program shaping only
//! emoji does not carry Unicode's tables). Vertical text has its own plan (`Plan::downward`):
//! the direction-independent features, plus vertical forms (`vert`); each glyph advances as the
//! font's `vmtx` says, centred in the column (see `vertical_text.rs`).
//!
//! Not done: fallback mark positioning for fonts without `GPOS`; stretching Syriac's
//! abbreviation mark over its letters (`stch` places its pieces without repeating them); random
//! alternates (`rand`); and language-specific kerning in the window's text (`font_file.rs`
//! kerns by script only).
use crate::bidi::{Kind, kind, mirrored};
use crate::font_file::{Asked, Bytes, FontFile, class, covered, lists_at, lookup_at, lookups};
use crate::scripts::{ARABIC, HEBREW, Language, SYRIAC};

/// The script a run is shaped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Script {
    /// No script of its own: figures, signs, emoji, and marks used by any script.
    None,
    /// A script with no special shaping (Latin, Greek, Cyrillic, Han and the rest), by its
    /// index in `scripts.rs`'s list: which of a font's features apply.
    Plain(u8),
    /// Arabic, whose letters join, and the languages written with it (Persian, Urdu).
    Arabic,
    /// Syriac, joining as Arabic does, with extra forms after alaph, dalath and rish.
    Syriac,
    Hebrew,
    /// One of the scripts `brahmic.rs` shapes by syllable, by its index in its list. (Only with
    /// the feature `brahmic`, and a right-to-left one also with `right-to-left`: without them,
    /// their letters have no script here.)
    Brahmic(u8),
}

impl Script {
    /// A character's script, if it is a letter or mark of one.
    pub fn of(letter: char) -> Option<Script> {
        match crate::scripts::of(letter) {
            0 => None,
            n if n as usize <= crate::scripts::BRAHMIC => crate::brahmic::shaped(n - 1).then_some(Script::Brahmic(n - 1)),
            ARABIC => Some(Script::Arabic),
            SYRIAC => Some(Script::Syriac),
            HEBREW => Some(Script::Hebrew),
            n => Some(Script::Plain(n)),
        }
    }

    /// The script tags a font may use for it, in lookup order: its own, then the default ones
    /// (and, as in HarfBuzz, `latn` last: some old fonts keep script-independent features
    /// there).
    fn names(self) -> Vec<&'static [u8; 4]> {
        let own: &'static [u8; 4] = match self {
            Script::None => return vec![b"DFLT", b"dflt", b"latn"],
            Script::Brahmic(n) => return crate::brahmic::names(n),
            Script::Plain(n) => crate::scripts::tag(n),
            Script::Arabic => b"arab",
            Script::Syriac => b"syrc",
            Script::Hebrew => b"hebr",
        };
        vec![own, b"DFLT", b"dflt", b"latn"]
    }
}

/// The glyphs a feature applies to: all (`ALL`), or those taking one of the joining forms (and
/// Syriac's forms after alaph, dalath and rish: the second and third final forms, the second
/// medial), or a mirrored form the font has no glyph for; and a fraction's figures, its
/// numerator's, its denominator's.
pub(crate) const ALL: u32 = 1;
pub(crate) const ALONE: u32 = 2;
pub(crate) const LAST: u32 = 4;
pub(crate) const MIDDLE: u32 = 8;
pub(crate) const FIRST: u32 = 16;
pub(crate) const MIRROR: u32 = 32;
const LAST_TWO: u32 = 1 << 14;
const LAST_THREE: u32 = 1 << 15;
const MIDDLE_TWO: u32 = 1 << 16;
pub(crate) const FRACTION: u32 = 1 << 17;
pub(crate) const NUMERATOR: u32 = 1 << 18;
pub(crate) const DENOMINATOR: u32 = 1 << 19;

/// A feature: its tag, the glyphs it applies to, and whether a joiner between glyphs stops its
/// rules matching (Arabic's joining features ask that).
type Feature = (&'static [u8; 4], u32, bool);

/// The `GSUB` features per shaper, in the stages they run in: a stage's lookups run together in
/// index order, and a stage starts when the previous one ends. (The first is for a variable
/// font's `rvrn`.)
const ARABIC_STAGES: [&[Feature]; 13] = [
    &[(b"rvrn", ALL, false)],
    &[(b"rtla", ALL, false), (b"rtlm", MIRROR, false), (b"frac", FRACTION, false), (b"numr", NUMERATOR, false), (b"dnom", DENOMINATOR, false), (b"stch", ALL, false)],
    &[(b"ccmp", ALL, true), (b"locl", ALL, true)],
    &[(b"isol", ALONE, false)],
    &[(b"fina", LAST, false)],
    &[(b"fin2", LAST_TWO, false)],
    &[(b"fin3", LAST_THREE, false)],
    &[(b"medi", MIDDLE, false)],
    &[(b"med2", MIDDLE_TWO, false)],
    &[(b"init", FIRST, false)],
    &[(b"rlig", ALL, true)],
    &[(b"rclt", ALL, true), (b"calt", ALL, true)],
    &[(b"mset", ALL, false), (b"liga", ALL, false), (b"clig", ALL, false)],
];
/// Syriac's: Arabic's, except required ligatures go with the contextual ones (Arabic's run
/// between, as HarfBuzz's fallback for featureless Arabic fonts).
const SYRIAC_STAGES: [&[Feature]; 12] = [
    ARABIC_STAGES[0],
    ARABIC_STAGES[1],
    ARABIC_STAGES[2],
    ARABIC_STAGES[3],
    ARABIC_STAGES[4],
    ARABIC_STAGES[5],
    ARABIC_STAGES[6],
    ARABIC_STAGES[7],
    ARABIC_STAGES[8],
    ARABIC_STAGES[9],
    &[(b"rlig", ALL, true), (b"rclt", ALL, true), (b"calt", ALL, true)],
    ARABIC_STAGES[12],
];
const LEFTWARD: [&[Feature]; 2] = [
    &[(b"rvrn", ALL, false)],
    &[
        (b"rtla", ALL, false),
        (b"rtlm", MIRROR, false),
        (b"frac", FRACTION, false),
        (b"numr", NUMERATOR, false),
        (b"dnom", DENOMINATOR, false),
        (b"ccmp", ALL, false),
        (b"locl", ALL, false),
        (b"rlig", ALL, false),
        (b"calt", ALL, false),
        (b"clig", ALL, false),
        (b"liga", ALL, false),
        (b"rclt", ALL, false),
    ],
];
const RIGHTWARD: [&[Feature]; 2] = [
    &[(b"rvrn", ALL, false)],
    &[
        (b"ltra", ALL, false),
        (b"ltrm", ALL, false),
        (b"frac", FRACTION, false),
        (b"numr", NUMERATOR, false),
        (b"dnom", DENOMINATOR, false),
        (b"ccmp", ALL, false),
        (b"locl", ALL, false),
        (b"rlig", ALL, false),
        (b"calt", ALL, false),
        (b"clig", ALL, false),
        (b"liga", ALL, false),
        (b"rclt", ALL, false),
    ],
];
/// Vertical text's features, in two stages (HarfBuzz's: `vert` is looked up separately, see
/// `Plan::downward`).
const DOWNWARD: [&[Feature]; 2] =
    [&[(b"rvrn", ALL, false)], &[(b"frac", FRACTION, false), (b"numr", NUMERATOR, false), (b"dnom", DENOMINATOR, false), (b"ccmp", ALL, false), (b"locl", ALL, false), (b"rlig", ALL, false)]];
/// And its `GPOS` features: mark positioning.
const DOWNWARD_MOVES: [Feature; 7] =
    [(b"frac", FRACTION, false), (b"numr", NUMERATOR, false), (b"dnom", DENOMINATOR, false), (b"abvm", ALL, false), (b"blwm", ALL, false), (b"mark", ALL, true), (b"mkmk", ALL, true)];

/// The lookups of the first feature named `name` in a table, under any script.
fn anywhere(table: Bytes, name: &[u8; 4]) -> Option<Vec<u16>> {
    let features = table.u16(6).filter(|_| table.u16(0) == Some(1))? as usize;
    let count = table.u16(features)? as usize;
    let n = (0..count).find(|n| table.u32(features + 2 + n * 6) == Some(u32::from_be_bytes(*name)))?;
    let feature = features + table.u16(features + 6 + n * 6)? as usize;
    let lookups = table.u16(feature + 2)? as usize;
    let mut found: Vec<u16> = (0..lookups).filter_map(|k| table.u16(feature + 4 + k * 2)).collect();
    found.sort_unstable();
    found.dedup();
    Some(found)
}

/// The `GPOS` features, all in one stage.
const MOVES: [Feature; 14] = [
    (b"ltra", ALL, false),
    (b"ltrm", ALL, false),
    (b"rtla", ALL, false),
    (b"rtlm", MIRROR, false),
    (b"frac", FRACTION, false),
    (b"numr", NUMERATOR, false),
    (b"dnom", DENOMINATOR, false),
    (b"abvm", ALL, false),
    (b"blwm", ALL, false),
    (b"mark", ALL, true),
    (b"mkmk", ALL, true),
    (b"curs", ALL, false),
    (b"dist", ALL, false),
    (b"kern", ALL, false),
];

/// Glyphs past this in one run are not shaped: no line comes near it, and it stops a damaged
/// font growing glyphs without end.
pub(crate) const LONGEST: usize = 4096;
/// How many times its glyph count a lookup may grow a run to (32,768 in all: see `LONGEST`).
const GROWTH: usize = 8;
/// How many subtables may be tried in one run, over all its glyphs and lookups. A real font
/// tries a few hundred thousand at most; a damaged one whose lookups each claim thousands of
/// subtables would otherwise take seconds per line.
const WORK: usize = 1 << 22;

/// One lookup as a stage runs it: its index, the glyphs it applies to, whether it skips joiners
/// when matching (and non-joiners, around a rule's input), and whether it keeps to the syllable
/// it starts in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Step {
    pub lookup: u16,
    pub mask: u32,
    pub joiners: bool,
    pub non_joiners: bool,
    pub syllable: bool,
}

/// When mark glyphs are zeroed: after `GPOS` (most scripts), before it (the Universal Shaping
/// Engine and Myanmar, so `GPOS` may give them width), or never (Indic and Khmer, whose fonts
/// give each mark its width).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Zeroing {
    #[default]
    Late,
    Early,
    Never,
}

/// What a font does for text in a script, direction and language: the `GSUB` lookups in stages,
/// and the `GPOS` ones; whether `GPOS` kerns the script (else the older `kern` table does);
/// whether the text is right-to-left, and whether its letters join; when marks are zeroed; and
/// for scripts `brahmic.rs` shapes, what runs between stages.
#[derive(Default)]
pub(crate) struct Plan {
    pub swaps: Vec<Vec<Step>>,
    pub moves: Vec<Step>,
    pub kerned: bool,
    pub leftward: bool,
    pub joins: bool,
    pub zeroing: Zeroing,
    pub syllabic: Option<Box<crate::brahmic::Syllabic>>,
    /// How letters and accents are composed and decomposed (see `normalization.rs`): handed
    /// over by `Plan::of`, so a program shaping only emoji (whose plans come from
    /// `Plan::looked_up`) does not carry Unicode's tables.
    pub accents: Option<(Normalizer, crate::normalization::Way)>,
    /// Whether the text is vertical (`Plan::downward`): glyphs advance by height, centred in
    /// the column; and whether, for a font without vertical forms (`vert`), characters with
    /// Unicode vertical forms use those.
    pub downward: bool,
    pub upright_forms: bool,
}

/// Composes and decomposes for a font: each character with its text index, returned as the font
/// is to have them, with the indices of combining grapheme joiners the lookups skip (see
/// `normalization::normalized`).
pub(crate) type Normalizer = fn(&FontFile, &[(char, u32)], &mut Vec<(u32, u16)>, crate::normalization::Way) -> (Vec<(char, u32)>, Vec<u32>);

impl Plan {
    /// The lookups of a script, direction and language from a font's tables; and composition as
    /// HarfBuzz's shaper for that script does it. (Scripts `brahmic.rs` shapes do that
    /// themselves.)
    pub fn of(font: &FontFile, script: Script, leftward: bool, language: Language) -> Plan {
        if let (true, Script::Brahmic(n)) = (cfg!(feature = "brahmic"), script) {
            return crate::brahmic::plan(font, n, language);
        }
        let plan = Plan::looked_up(font, script, leftward, language);
        let marks = || {
            let languages: Vec<[u8; 4]> = language.systems().copied().collect();
            lookups(font.table(b"GPOS"), (&script.names(), &languages, font.place()), &[b"mark"]).is_some()
        };
        let way = match script {
            _ if plan.joins => crate::normalization::Way::Arabic,
            Script::Hebrew => crate::normalization::Way::Hebrew { forms: !marks() },
            Script::Plain(n) if crate::scripts::tag(n) == b"hang" => crate::normalization::Way::Hangul,
            _ => crate::normalization::Way::Usual,
        };
        Plan { accents: Some((crate::normalization::normalized, way)), ..plan }
    }

    /// The lookups for vertical text, as HarfBuzz finds them: the direction-independent ones
    /// (`ccmp`, `locl`, `rlig`, mark positioning) and vertical forms (`vert`, under whichever
    /// script the font lists it if not the letters' own); not horizontal ones (`liga`, `kern`
    /// and the rest).
    pub fn downward(font: &FontFile, script: Script, language: Language) -> Plan {
        let plan = Plan::of(font, script, false, language);
        let languages: Vec<[u8; 4]> = language.systems().copied().collect();
        let (swapping, moving, names) = (font.table(b"GSUB"), font.table(b"GPOS"), script.names());
        let asked: Asked = (&names, &languages, font.place());
        let mut swaps = vec![steps(swapping, asked, DOWNWARD[0]), steps(swapping, asked, DOWNWARD[1])];
        let forms = lookups(swapping, asked, &[b"vert"]).or_else(|| anywhere(swapping, b"vert"));
        let found = forms.iter().flatten().map(|lookup| Step { lookup: *lookup, mask: ALL, joiners: true, non_joiners: true, syllable: false });
        swaps[1] = merged(swaps[1].iter().copied().chain(found).collect());
        let moves = steps(moving, asked, &DOWNWARD_MOVES);
        Plan { swaps, moves, kerned: true, leftward: false, joins: false, downward: true, upright_forms: forms.is_none(), ..plan }
    }

    /// The lookups alone, without composition: for emoji, which need none.
    pub fn looked_up(font: &FontFile, script: Script, leftward: bool, language: Language) -> Plan {
        let languages: Vec<[u8; 4]> = language.systems().copied().collect();
        let (swapping, moving, names) = (font.table(b"GSUB"), font.table(b"GPOS"), script.names());
        // (Syriac shapes as Arabic unless the font lists no script, as HarfBuzz has it: then
        // its tables list only `DFLT`.)
        let (stages, joins): (&[&[Feature]], bool) = match (script, leftward) {
            (Script::Arabic, _) => (&ARABIC_STAGES, true),
            (Script::Syriac, _) if chosen(swapping, &names) != Some(*b"DFLT") => (&SYRIAC_STAGES, true),
            (_, true) => (&LEFTWARD, false),
            (_, false) => (&RIGHTWARD, false),
        };
        let asked: Asked = (&names, &languages, font.place());
        let swaps = stages.iter().map(|features| steps(swapping, asked, features)).collect();
        let kerned = lookups(moving, asked, &[b"kern"]).is_some();
        Plan { swaps, moves: steps(moving, asked, &MOVES), kerned, leftward, joins, ..Plan::default() }
    }
}

/// The glyph for a letter alone in a language: the font's localized form (by `locl`, under the
/// letter's script), else its own glyph.
pub(crate) fn local(font: &FontFile, letter: char, language: Language) -> u16 {
    let script = Script::of(letter).unwrap_or(Script::None);
    let languages: Vec<[u8; 4]> = language.systems().copied().collect();
    let found = steps(font.table(b"GSUB"), (&script.names(), &languages, font.place()), &[(b"locl", ALL, false)]);
    if found.is_empty() {
        return font.glyph(letter);
    }
    let (plan, mut glyphs, mut written) = (Plan { swaps: vec![found], kerned: true, ..Plan::default() }, Vec::new(), [0; 4]);
    font.shape(&plan, letter.encode_utf8(&mut written), &mut glyphs);
    match glyphs[..] {
        [laid] => laid.glyph,
        _ => font.glyph(letter),
    }
}

/// The script tag a table lists text of a script under: the first of `names` it has (with
/// features or not).
pub(crate) fn chosen(table: Bytes, names: &[&'static [u8; 4]]) -> Option<[u8; 4]> {
    let scripts = table.u16(4).filter(|_| table.u16(0) == Some(1))? as usize;
    let count = table.u16(scripts)? as usize;
    let listed = |name: &[u8; 4]| (0..count).any(|n| table.u32(scripts + 2 + n * 6) == Some(u32::from_be_bytes(*name)));
    names.iter().find(|name| listed(name)).map(|name| **name)
}

/// The lookups some features name for a script, language and variation position, in index
/// order, each once: applying to the glyphs of every feature naming it.
fn steps(table: Bytes, asked: Asked, features: &[Feature]) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for (name, mask, joining) in features {
        let found = lookups(table, asked, &[*name]).unwrap_or_default();
        steps.extend(found.into_iter().map(|lookup| Step { lookup, mask: *mask, joiners: !*joining, non_joiners: true, syllable: false }));
    }
    merged(steps)
}

/// Steps in lookup order, each lookup once: for the glyphs of all steps naming it, skipping
/// joiners only where all do.
pub(crate) fn merged(mut steps: Vec<Step>) -> Vec<Step> {
    steps.sort_by_key(|step| step.lookup);
    let mut merged: Vec<Step> = Vec::new();
    for step in steps {
        match merged.last_mut() {
            Some(last) if last.lookup == step.lookup => {
                (last.mask, last.joiners, last.non_joiners) = (last.mask | step.mask, last.joiners && step.joiners, last.non_joiners && step.non_joiners);
                last.syllable |= step.syllable;
            }
            _ => merged.push(step),
        }
    }
    merged
}

/// A glyph's `GDEF` class: base, ligature or mark (a bit each, as lookup flags name them), and
/// for a mark its mark attachment class (second byte).
pub(crate) const BASE: u16 = 2;
const LIGATURE: u16 = 4;
pub(crate) const MARK: u16 = 8;
/// What was done to a glyph: substituted, ligated, multiplied.
pub(crate) const SUBSTITUTED: u16 = 0x10;
pub(crate) const LIGATED: u16 = 0x20;
pub(crate) const MULTIPLIED: u16 = 0x40;
/// Character properties: default ignorable (and a joiner or non-joiner among those); hidden
/// even more.
const IGNORABLE: u8 = 1;
const JOINER: u8 = 2;
const NON_JOINER: u8 = 4;
pub(crate) const HIDDEN: u8 = 8;

/// One glyph being shaped, and finally its position.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Laid {
    pub glyph: u16,
    /// The (first) character it stands for, by text index.
    pub from: u32,
    /// The features it applies to.
    pub mask: u32,
    /// Its `GDEF` class, and what was done to it.
    pub props: u16,
    /// Its character's properties (see `IGNORABLE`); in the high four bits, for a space the
    /// font lacks and that is drawn with its own, which space it is (see
    /// `normalization::space`).
    pub unicode: u8,
    /// Of a ligature: its id, and how many characters it covers. Of a mark belonging to one, or
    /// a multiplied glyph: which id, and which component (from one; zero for none).
    lig_id: u8,
    lig_comp: u8,
    lig_base: bool,
    /// Its advance, and its offset from the pen (right and up), in font units.
    pub advance: i32,
    pub offset: [i32; 2],
    /// The glyph it attaches to, relative to it, and how: as a mark (1) or cursively (2).
    chain: i16,
    attach: u8,
    /// For syllabic text (`brahmic.rs`): its character, its category and position in the
    /// syllable, and the syllable (number times sixteen, plus type; zero once the per-syllable
    /// features are done). And which syllable of the run it is, from one, kept to the end.
    pub letter: char,
    pub category: u8,
    pub position: u8,
    pub syllable: u8,
    pub whole: u16,
}

impl Laid {
    fn mark(&self) -> bool {
        self.props & MARK != 0
    }
    fn ignorable(&self) -> bool {
        self.unicode & IGNORABLE != 0 && self.props & SUBSTITUTED == 0
    }
    /// Its ligature component (zero for none, or for a ligature itself).
    pub(crate) fn component(&self) -> u8 {
        if self.lig_base { 0 } else { self.lig_comp }
    }
    /// How many characters it covers: a ligature's components; else one.
    fn parts(&self) -> u8 {
        if self.lig_base && self.props & LIGATURE != 0 { self.lig_comp } else { 1 }
    }
}

// ---- the characters, before anything is looked up ----

/// How the joining scripts' characters join, as ranges: first character (times 2048), count
/// more (times 8), and type: 0 non-joining, 1 left-joining (the next letter only), 2
/// right-joining (the one before only), 3 dual, 4 Syriac alaph, 5 dalath or rish, 6
/// transparent. (Unicode 17's `ArabicShaping.txt`.)
#[rustfmt::skip]
const JOINING: [u32; 186] = [
    0x300028, 0x304000, 0x305800, 0x310003, 0x310800, 0x31101a, 0x313003, 0x313802, 0x314003, 0x314802,
    0x315023, 0x31781a, 0x3198a3, 0x324002, 0x32480b, 0x33700b, 0x338812, 0x33a000, 0x33a812, 0x33c07b,
    0x34408a, 0x34d12b, 0x360002, 0x36080b, 0x361842, 0x366003, 0x366802, 0x367003, 0x367802, 0x36800b,
    0x36900a, 0x36a802, 0x36e800, 0x37700a, 0x37d013, 0x37f803, 0x387806, 0x388004, 0x389013, 0x38a80d,
    0x38b812, 0x38d01b, 0x38f002, 0x38f843, 0x394002, 0x394803, 0x395005, 0x395803, 0x396002, 0x39680b,
    0x397805, 0x3a6802, 0x3a7053, 0x3ac812, 0x3ae073, 0x3b580a, 0x3b681b, 0x3b8802, 0x3b9003, 0x3b980a,
    0x3ba813, 0x3bc00a, 0x3bd02b, 0x3e5103, 0x3fd003, 0x420002, 0x420823, 0x42300a, 0x424003, 0x424802,
    0x42504b, 0x42a002, 0x42a803, 0x42b012, 0x430003, 0x430800, 0x43101b, 0x433000, 0x433802, 0x434003,
    0x43480a, 0x438092, 0x44181b, 0x443808, 0x444823, 0x447002, 0x447803, 0x448008, 0x45004b, 0x455012,
    0x456800, 0x457002, 0x45780b, 0x45880a, 0x45982b, 0x45c802, 0x45d073, 0x471000, 0xc03000, 0xc03803,
    0xc05003, 0xc07000, 0xc102c3, 0xc40020, 0xc4280e, 0xc4390b, 0xc55003, 0x1006000, 0x1006803, 0x1017800,
    0x1033018, 0x542018b, 0x5439001, 0x5439800, 0x8560023, 0x8562802, 0x8563000, 0x8563802, 0x8564000, 0x856480a,
    0x8565808, 0x8566801, 0x8567022, 0x856981b, 0x856b801, 0x856c023, 0x856e802, 0x856f013, 0x8570802, 0x8571008,
    0x8572002, 0x857581b, 0x8577802, 0x85c0003, 0x85c0802, 0x85c1003, 0x85c1812, 0x85c3013, 0x85c4802, 0x85c500b,
    0x85c6002, 0x85c6803, 0x85c700a, 0x85c8003, 0x85c8802, 0x85d481a, 0x85d680b, 0x85d7800, 0x8680001, 0x8680903,
    0x8691002, 0x8691803, 0x8761002, 0x876180b, 0x876300b, 0x8798013, 0x8799802, 0x879a083, 0x87a2800, 0x87a8813,
    0x87aa002, 0x87b801b, 0x87ba00a, 0x87bb05b, 0x87d8003, 0x87d8800, 0x87d900b, 0x87da012, 0x87db800, 0x87dc003,
    0x87dc80a, 0x87dd80b, 0x87de802, 0x87df00b, 0x87e0000, 0x87e0803, 0x87e100a, 0x87e2003, 0x87e2818, 0x87e4802,
    0x87e5003, 0x87e5801, 0x885e800, 0x8866800, 0xf48021b, 0xf4a5806,
];

/// A character's joining type: none (0), the next only (1), the previous only (2), both (3);
/// Syriac alaph (4), or dalath or rish (5), which join only the previous; or transparent (6).
/// (Unlisted characters are transparent if marks or default ignorables, else non-joining.)
fn joining(letter: char) -> u8 {
    let code = letter as u32;
    let near = JOINING.partition_point(|entry| entry >> 11 <= code);
    let found = near.checked_sub(1).map(|n| JOINING[n]).filter(|entry| code - (entry >> 11) <= (entry >> 3) & 0xff);
    match found {
        Some(entry) => (entry & 7) as u8,
        None if matches!(kind(letter), Kind::Mark | Kind::Unseen)
            || crate::brahmic::passed_over(letter)
            || matches!(letter, '\u{180b}'..='\u{180d}' | '\u{180f}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}') =>
        {
            6
        }
        None => 0,
    }
}

/// A character's properties for the lookups: whether it is default ignorable (and which kind).
pub(crate) fn unicode(letter: char) -> u8 {
    let ignorable = matches!(letter as u32, 0xad | 0x34f | 0x61c | 0x17b4..=0x17b5 | 0x180b..=0x180e | 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f | 0xfe00..=0xfe0f | 0xfeff | 0xfff0..=0xfff8 | 0x1d173..=0x1d17a | 0xe0000..=0xe0fff);
    let hidden = matches!(letter as u32, 0x34f | 0x180b..=0x180d | 0xe0020..=0xe007f);
    let joiner = match letter {
        '\u{200d}' => JOINER,
        '\u{200c}' => NON_JOINER,
        _ => 0,
    };
    (ignorable as u8 * IGNORABLE) | (hidden as u8 * HIDDEN) | joiner
}

/// HarfBuzz's Arabic joining state machine: from the state the letters before leave (0 after a
/// non-joiner, 1 after a word end or lone alaph, 2 after a lone letter that may join, 3 after a
/// word end that may, 4 after a final alaph, 5 after an alaph in its second or third final
/// form, 6 after dalath or rish) and the letter's joining type: what the previous letter
/// becomes (if anything), what this one becomes, and the next state.
#[rustfmt::skip]
const JOINS: [[(u32, u32, u8); 6]; 7] = [
    [(0, 0, 0), (0, ALONE, 2), (0, ALONE, 1), (0, ALONE, 2), (0, ALONE, 1), (0, ALONE, 6)],
    [(0, 0, 0), (0, ALONE, 2), (0, ALONE, 1), (0, ALONE, 2), (0, LAST_TWO, 5), (0, ALONE, 6)],
    [(0, 0, 0), (0, ALONE, 2), (FIRST, LAST, 1), (FIRST, LAST, 3), (FIRST, LAST, 4), (FIRST, LAST, 6)],
    [(0, 0, 0), (0, ALONE, 2), (MIDDLE, LAST, 1), (MIDDLE, LAST, 3), (MIDDLE, LAST, 4), (MIDDLE, LAST, 6)],
    [(0, 0, 0), (0, ALONE, 2), (MIDDLE_TWO, ALONE, 1), (MIDDLE_TWO, ALONE, 2), (MIDDLE_TWO, LAST_TWO, 5), (MIDDLE_TWO, ALONE, 6)],
    [(0, 0, 0), (0, ALONE, 2), (ALONE, ALONE, 1), (ALONE, ALONE, 2), (ALONE, LAST_TWO, 5), (ALONE, ALONE, 6)],
    [(0, 0, 0), (0, ALONE, 2), (0, ALONE, 1), (0, ALONE, 2), (0, LAST_THREE, 5), (0, ALONE, 6)],
];

impl FontFile {
    /// Shapes a run by a plan: its glyphs in visual order, left to right, each with its
    /// position; and the total advance, in font units. A character the font lacks is glyph 0.
    pub(crate) fn shape(&self, plan: &Plan, text: &str, glyphs: &mut Vec<Laid>) -> i32 {
        // (Without `right-to-left` nothing is shaped right-to-left: only emoji are shaped then.
        // Without `brahmic`, nothing by syllable.)
        let leftward = cfg!(feature = "right-to-left") && plan.leftward;
        let syllabic = plan.syllabic.as_deref().filter(|_| cfg!(feature = "brahmic"));
        glyphs.clear();
        match syllabic {
            Some(syllabic) => crate::brahmic::characters(self, syllabic, text, glyphs),
            None => self.characters(plan, text, glyphs),
        }
        let gdef = self.table(b"GDEF");
        glyphs.iter_mut().for_each(|laid| laid.props = props(gdef, laid.glyph, laid.props));
        let (swaps, moves) = (self.table(b"GSUB"), self.table(b"GPOS"));
        let mut shaper = Shaper { gdef, leftward, serial: 0, positions: Vec::new(), work: WORK };
        for (n, stage) in plan.swaps.iter().enumerate() {
            for step in stage {
                shaper.carry(swaps, false, *step, glyphs);
            }
            if let Some(syllabic) = syllabic {
                crate::brahmic::pause(self, syllabic, n, glyphs);
            }
        }
        // (Default ignorables are removed now; in syllabic text, as HarfBuzz does, once glyphs
        // are positioned: until then a mark may not reach across a joiner.)
        if syllabic.is_none() {
            glyphs.retain(|laid| !laid.ignorable());
        }
        for laid in glyphs.iter_mut() {
            laid.advance = self.width(laid.glyph) as i32;
            if plan.downward {
                let (advance, [across, top]) = self.downward(laid.glyph);
                (laid.advance, laid.offset) = (advance, [-across, -top]);
            }
            if laid.unicode >> 4 != 0 && laid.props & LIGATED == 0 {
                laid.advance = self.spaced(laid.unicode >> 4, laid.advance);
            }
        }
        // (In a font without `GPOS`, a zeroed mark hangs over the glyph before it.)
        let hang = syllabic.is_some() && moves.0.is_empty();
        if plan.zeroing == Zeroing::Early {
            zero_marks(glyphs, hang);
        }
        for step in &plan.moves {
            shaper.carry(moves, true, *step, glyphs);
        }
        if !plan.kerned {
            self.kern_older(glyphs);
        }
        if plan.zeroing == Zeroing::Late {
            zero_marks(glyphs, hang);
        }
        fix_attached(glyphs, leftward, plan.downward);
        if syllabic.is_some() {
            glyphs.retain(|laid| !laid.ignorable());
        }
        if leftward {
            glyphs.reverse();
        }
        glyphs.iter().map(|laid| laid.advance).sum()
    }

    /// Each character's glyph, in HarfBuzz's order: a character with a variation selector the
    /// font maps as that glyph, marks after a base sorted by combining class, a mirrored form
    /// for each bracket in right-to-left text, joining forms for Arabic and Syriac, and
    /// fraction figures marked.
    fn characters(&self, plan: &Plan, text: &str, glyphs: &mut Vec<Laid>) {
        let mut letters: Vec<(char, u32)> = text.chars().zip(0..).take(LONGEST).collect();
        // (Vertical text in a font without vertical forms takes Unicode's where the font has
        // them, as in HarfBuzz.)
        if plan.upright_forms {
            letters.iter_mut().for_each(|(letter, _)| *letter = crate::vertical_text::form(*letter).filter(|form| self.glyph(*form) != 0).unwrap_or(*letter));
        }
        let mut variants = variants(self, &mut letters);
        let (letters, passed) = match plan.accents {
            Some((normalized, way)) => normalized(self, &letters, &mut variants, way),
            None => (letters.clone(), Vec::new()),
        };
        // (Without `right-to-left` no text with joining forms or mirroring comes here.)
        let (joins, leftward) = (cfg!(feature = "right-to-left") && plan.joins, cfg!(feature = "right-to-left") && plan.leftward);
        let mut forms = vec![ALL; letters.len()];
        if joins {
            joined(letters.iter().map(|(letter, _)| *letter), &mut forms);
        }
        fractions(letters.iter().map(|(letter, _)| *letter), leftward, &mut forms);
        let first = glyphs.len();
        for (n, (letter, from)) in letters.iter().copied().enumerate() {
            let mirror = if leftward { mirrored(letter) } else { None };
            let shown = mirror.filter(|mirror| self.glyph(*mirror) != 0).unwrap_or(letter);
            let mask = forms[n] | if mirror.is_some() && shown == letter { MIRROR } else { 0 } | ALL;
            let glyph = variants.iter().find(|(at, _)| *at == from).map_or_else(|| self.glyph(shown), |(_, glyph)| *glyph);
            let (glyph, spaced) = if glyph == 0 { self.instead(letter) } else { (glyph, 0) };
            glyphs.push(Laid { glyph, from, mask, unicode: unicode(letter) | spaced << 4, props: base_or_mark(letter), ..Laid::default() });
        }
        // (The joiners the lookups skip are as the plan says where it composed; else all of
        // them, as nothing came before.)
        let letters: Vec<char> = letters.iter().map(|(letter, _)| *letter).collect();
        match plan.accents {
            Some(_) => glyphs[first..].iter_mut().filter(|laid| passed.contains(&laid.from)).for_each(|laid| laid.unicode &= !HIDDEN),
            None => unhidden(&letters, &mut glyphs[first..], |_| 0),
        }
    }

    /// What HarfBuzz draws a missing character with: a width-specific space as the font's space
    /// (and which space, see `normalization::space`), a non-breaking hyphen as the plain one.
    /// Else glyph 0.
    fn instead(&self, letter: char) -> (u16, u8) {
        let space = crate::normalization::space(letter).filter(|_| self.glyph(' ') != 0);
        match (space, letter) {
            (Some(kind), _) => (self.glyph(' '), kind),
            (None, '\u{2011}') => (self.glyph('\u{2010}'), 0),
            _ => (0, 0),
        }
    }

    /// The width of a space of a kind (see `normalization::space`) drawn with the font's space,
    /// in font units, as HarfBuzz computes it from that space's `advance`.
    fn spaced(&self, kind: u8, advance: i32) -> i32 {
        let em = self.units() as i32;
        let wide = |letters: &str| letters.chars().map(|letter| self.glyph(letter)).find(|glyph| *glyph != 0).map_or(advance, |glyph| self.width(glyph) as i32);
        match kind as i32 {
            parts @ 1..=6 => (em + parts / 2) / parts,
            7 => (em + 8) / 16,
            8 => em * 4 / 18,
            9 => wide("0123456789"),
            10 => wide(".,"),
            11 => advance / 2,
            _ => advance,
        }
    }

    /// Kerns glyph pairs by the older `kern` table, for scripts `GPOS` does not kern: half of
    /// each pair's kerning to each glyph, as HarfBuzz does.
    fn kern_older(&self, glyphs: &mut [Laid]) {
        for n in 0..glyphs.len() {
            let Some(next) = (n + 1..glyphs.len()).find(|at| !glyphs[*at].mark()) else { break };
            let by = self.kern_pair(glyphs[n].glyph, glyphs[next].glyph) as i32;
            let half = by >> 1;
            glyphs[n].advance += half;
            (glyphs[next].advance, glyphs[next].offset[0]) = (glyphs[next].advance + by - half, glyphs[next].offset[0] + by - half);
        }
    }
}

/// Makes each combining grapheme joiner (U+034F) that kept no marks from being reordered
/// skippable by the lookups, as HarfBuzz does after mark reordering: one before a class-0
/// character, or between marks already in order. (Where it did separate two marks, the lookups
/// do not skip it.)
pub(crate) fn unhidden(letters: &[char], glyphs: &mut [Laid], place: impl Fn(char) -> u8) {
    for n in 1..letters.len().saturating_sub(1) {
        if letters[n] == '\u{34f}' && (place(letters[n + 1]) == 0 || place(letters[n - 1]) <= place(letters[n + 1])) {
            glyphs[n].unicode &= !HIDDEN;
        }
    }
}

/// A glyph's class for a font without `GDEF` classes: a mark if its character is one (but not
/// default ignorable), else a base.
fn base_or_mark(letter: char) -> u16 {
    if kind(letter) == Kind::Mark && unicode(letter) & IGNORABLE == 0 { MARK } else { BASE }
}

/// A glyph's `GDEF` class, with what was done to it; where `GDEF` has no classes, `guess`.
fn props(gdef: Bytes, glyph: u16, guess: u16) -> u16 {
    let done = guess & (SUBSTITUTED | LIGATED | MULTIPLIED);
    let Some(classes) = gdef.u16(4).filter(|at| *at != 0) else { return guess };
    match class(gdef, classes as usize, glyph) {
        1 => done | BASE,
        2 => done | LIGATURE,
        3 => done | MARK | (gdef.u16(10).filter(|at| *at != 0).map_or(0, |at| class(gdef, at as usize, glyph) as u16) << 8),
        _ => done,
    }
}

/// Each joining letter's form: isolated, initial, medial or final (and in Syriac, the forms
/// after alaph, dalath and rish). Marks and ignorables are skipped; a Mongolian free variation
/// selector takes the form of the letter before, as HarfBuzz does, for the font's rules to use.
pub(crate) fn joined(letters: impl Iterator<Item = char>, forms: &mut [u32]) {
    let (mut state, mut before) = (0usize, None);
    let mut selectors = Vec::new();
    for (n, letter) in letters.enumerate() {
        if matches!(letter, '\u{180b}'..='\u{180d}' | '\u{180f}') && n > 0 {
            selectors.push(n);
        }
        let way = joining(letter);
        if way == 6 {
            forms[n] = 0;
            continue;
        }
        let (previous, own, next) = JOINS[state][way as usize];
        if let (Some(before), true) = (before, previous != 0) {
            forms[before] = previous;
        }
        (forms[n], state, before) = (own, next as usize, Some(n));
    }
    for n in selectors {
        forms[n] = forms[n - 1];
    }
}

/// Marks the figures around each fraction slash (U+2044), as HarfBuzz does: all as fraction,
/// those before it (after, in right-to-left text) as numerator, the others as denominator. (A
/// slash without a figure on one side makes no fraction.)
pub(crate) fn fractions(letters: impl Iterator<Item = char> + Clone, leftward: bool, forms: &mut [u32]) {
    let slashes = letters.clone().enumerate().filter(|(_, letter)| *letter == '\u{2044}').map(|(at, _)| at);
    let digits: Vec<bool> = letters.map(crate::scripts::digit).collect();
    let [before, after] = if leftward { [DENOMINATOR, NUMERATOR] } else { [NUMERATOR, DENOMINATOR] };
    for at in slashes.collect::<Vec<_>>() {
        let start = at - digits[..at].iter().rev().take_while(|digit| **digit).count();
        let end = at + 1 + digits[at + 1..].iter().take_while(|digit| **digit).count();
        if start == at || end == at + 1 {
            continue;
        }
        forms[start..at].iter_mut().for_each(|form| *form |= before | FRACTION);
        forms[at] |= FRACTION;
        forms[at + 1..end].iter_mut().for_each(|form| *form |= after | FRACTION);
    }
}

/// Removes from a run the variation selectors (U+FE00 to U+FE0F, U+E0100 to U+E01EF) for which
/// the font maps the preceding character, as HarfBuzz does first: the character becomes that
/// glyph (returned with its index), and the selector goes. A selector the font does not know
/// stays, as its own (ignorable) glyph, for the font's lookups.
pub(crate) fn variants(font: &FontFile, letters: &mut Vec<(char, u32)>) -> Vec<(u32, u16)> {
    let selector = |letter: char| matches!(letter, '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}');
    let mut found = Vec::new();
    let mut n = 0;
    while n + 1 < letters.len() {
        let ((letter, from), next) = (letters[n], letters[n + 1].0);
        if let Some(glyph) = selector(next).then(|| font.variant(letter, next)).flatten() {
            found.push((from, glyph));
            letters.remove(n + 1);
        }
        n += 1;
    }
    found
}

// ---- carrying lookups out ----

/// The lookup engine's state: the `GDEF` table, whether the text is right-to-left, how many
/// ligatures were made (each has its id), and where a rule's matched glyphs are.
struct Shaper<'a> {
    gdef: Bytes<'a>,
    leftward: bool,
    serial: u8,
    positions: Vec<usize>,
    /// How many more subtables may be tried (see `WORK`).
    work: usize,
}

/// A lookup being applied: its table, whether it is `GPOS`, its flags, its mark filtering set
/// if any, and the step that called it.
#[derive(Clone, Copy)]
struct Doing<'a> {
    table: Bytes<'a>,
    moves: bool,
    flags: u16,
    marks: u16,
    step: Step,
    /// The syllable of the glyph it is applied at (a syllable-bound lookup matches no glyph of
    /// another).
    syllable: u8,
}

/// How deep contextual lookups may nest, as HarfBuzz allows.
const DEEPEST: usize = 64;

/// A contextual rule being applied: its lookup, subtable, its lookup records (where, how many,
/// which next), the matched glyphs' positions and their end. And for a nested rule: which of
/// the caller's glyphs it was called at, at which position, and the glyph count then.
struct Call<'a> {
    doing: Doing<'a>,
    list: Bytes<'a>,
    records: (usize, usize),
    next: usize,
    positions: Vec<usize>,
    end: usize,
    from: (usize, usize, usize),
}

impl Call<'_> {
    /// After a lookup it called at its `nth` glyph (at `place`) changed the glyph count by
    /// `delta`: moves its end, and the positions after that one, as HarfBuzz does. (Never back
    /// past where the lookup was applied: what went there was after it.)
    fn moved(&mut self, nth: usize, place: usize, mut delta: isize) {
        if delta == 0 {
            return;
        }
        let moved = self.end as isize + delta;
        if moved < place as isize {
            delta += place as isize - moved;
        }
        self.end = moved.max(place as isize) as usize;
        shift_places(&mut self.positions, nth + 1, place, delta);
    }
}

/// What a glyph is to a lookup scanning for the next it may match: skipped, matched if it fits
/// (a joiner, say), or matched.
#[derive(Clone, Copy, PartialEq)]
enum Skip {
    Yes,
    Maybe,
    No,
}

/// Lookup flags: right-to-left (1), ignore bases (2), ligatures (4) and marks (8), use mark
/// filtering set (16), and mark attachment class (second byte).
const LEFTWARD_FLAG: u16 = 1;
const IGNORE: u16 = 2 | 4 | 8;
const MARK_SET: u16 = 16;

impl<'a> Shaper<'a> {
    /// Applies one lookup along the run: at each glyph it applies to, the first subtable
    /// covering that glyph acts. (Reverse chaining lookups go from the last glyph back.)
    fn carry(&mut self, table: Bytes<'a>, moves: bool, step: Step, glyphs: &mut Vec<Laid>) {
        let Some(at) = lookup_at(table, step.lookup) else { return };
        let (flags, count) = (table.u16(at + 2).unwrap_or(0), table.u16(at + 4).unwrap_or(0) as usize);
        let marks = if flags & MARK_SET != 0 { table.u16(at + 6 + count * 2).unwrap_or(0) } else { 0 };
        let doing = Doing { table, moves, flags, marks, step, syllable: 0 };
        let backward = !moves && lists_at(table, at, 7).next().is_some_and(|(kind, _)| kind == 8);
        let limit = glyphs.len().max(1) * GROWTH;
        let mut n = if backward { glyphs.len() } else { 0 };
        loop {
            match backward {
                true if n == 0 => return,
                true => n -= 1,
                false if n >= glyphs.len() || glyphs.len() > limit.min(LONGEST * GROWTH) => return,
                false => {}
            }
            let fits = glyphs[n].mask & step.mask != 0 && self.keeps(&doing, doing.flags, &glyphs[n]) && self.spend(count);
            let here = Doing { syllable: glyphs[n].syllable, ..doing };
            let next = if fits { self.apply(&here, at, glyphs, n) } else { None };
            n = match (backward, next) {
                (true, _) => n,
                (false, Some(next)) => next,
                (false, None) => n + 1,
            };
        }
    }

    /// Charges `lists` subtables against the budget: whether any was left.
    fn spend(&mut self, lists: usize) -> bool {
        self.work = self.work.saturating_sub(lists.max(1));
        self.work > 0
    }

    /// Applies a lookup at glyph `n`: the first subtable covering it. Returns the next
    /// unprocessed glyph, if one acted.
    fn apply(&mut self, doing: &Doing<'a>, at: usize, glyphs: &mut Vec<Laid>, n: usize) -> Option<usize> {
        let far = if doing.moves { 9 } else { 7 };
        lists_at(doing.table, at, far).find_map(|(kind, list)| self.list(doing, kind, Bytes(doing.table.0.get(list..)?), glyphs, n))
    }

    /// What one subtable does at glyph `n`, by type.
    fn list(&mut self, doing: &Doing<'a>, kind: u16, list: Bytes<'a>, glyphs: &mut Vec<Laid>, n: usize) -> Option<usize> {
        let gdef = self.gdef;
        match (doing.moves, kind) {
            (false, 1) => one_for_one(gdef, list, glyphs, n),
            (false, 2) => one_for_several(gdef, list, glyphs, n),
            (false, 3) => one_of_several(gdef, list, glyphs, n),
            (false, 4) => self.ligature(doing, list, glyphs, n),
            (false, 8) => backward_rule(self, doing, list, glyphs, n),
            (true, 1) => moved(list, glyphs, n),
            (true, 2) => self.pair(doing, list, glyphs, n),
            (true, 3) => self.stroke(doing, list, glyphs, n),
            (true, 4) => self.on_base(doing, list, glyphs, n),
            (true, 5) => self.on_ligature(doing, list, glyphs, n),
            (true, 6) => self.on_mark(doing, list, glyphs, n),
            (false, 5) | (true, 7) => self.rule(doing, false, list, glyphs, n),
            (false, 6) | (true, 8) => self.rule(doing, true, list, glyphs, n),
            _ => None,
        }
    }

    /// Whether a lookup's flags keep a glyph (do not skip it): the classes it ignores, and its
    /// mark filter.
    fn keeps(&self, doing: &Doing, flags: u16, laid: &Laid) -> bool {
        if laid.props & flags & IGNORE != 0 {
            return false;
        }
        if !laid.mark() {
            return true;
        }
        if flags & MARK_SET != 0 {
            return self.in_set(doing.marks, laid.glyph);
        }
        flags & 0xff00 == 0 || flags & 0xff00 == laid.props & 0xff00
    }

    /// Whether a glyph is in a `GDEF` mark glyph set.
    fn in_set(&self, set: u16, glyph: u16) -> bool {
        let gdef = self.gdef;
        let Some(sets) = gdef.u16(12).filter(|at| *at != 0 && gdef.u16(2).is_some_and(|minor| minor >= 2)) else { return false };
        let sets = sets as usize;
        let at = gdef.u32(sets + 4 + set as usize * 4).filter(|_| gdef.u16(sets + 2).is_some_and(|count| set < count));
        at.is_some_and(|at| covered(gdef, sets + at as usize, glyph).is_some())
    }

    /// What a glyph is to a scanning lookup: skipped by its flags; or default ignorable,
    /// matched only where it fits (a joiner, where the lookup skips joiners, or always in a
    /// rule's context).
    fn skip(&self, doing: &Doing, flags: u16, laid: &Laid, around: bool) -> Skip {
        if !self.keeps(doing, flags, laid) {
            return Skip::Yes;
        }
        let ignore_non_joiner = doing.moves || (around && doing.step.non_joiners);
        let ignore_joiner = around || doing.step.joiners;
        let passed = laid.ignorable() && (ignore_non_joiner || laid.unicode & NON_JOINER == 0) && (ignore_joiner || laid.unicode & JOINER == 0) && (doing.moves || laid.unicode & HIDDEN == 0);
        if passed { Skip::Maybe } else { Skip::No }
    }

    /// The next glyph after `n` (or before, `back`) a lookup considers, by its flags (in a
    /// rule's context `around`, or its input); and whether `fits` accepts it. Nothing where an
    /// unskipped glyph does not fit, or the run ends.
    fn along(&self, doing: &Doing, flags: u16, glyphs: &[Laid], n: usize, (back, around): (bool, bool), fits: impl Fn(&Laid) -> bool) -> Option<usize> {
        let mut at = n;
        loop {
            at = if back { at.checked_sub(1)? } else { Some(at + 1).filter(|at| *at < glyphs.len())? };
            let laid = &glyphs[at];
            let skip = self.skip(doing, flags, laid, around);
            if skip == Skip::Yes {
                continue;
            }
            // (In context any glyph may match; in the input, only one the lookup's features
            // apply to. A syllable-bound lookup matches nothing of another syllable.)
            let elsewhere = doing.step.syllable && !doing.moves && doing.syllable != 0 && laid.syllable != doing.syllable;
            let wanted = (around || laid.mask & doing.step.mask != 0) && !elsewhere;
            match (wanted && fits(laid), skip) {
                (true, _) => return Some(at),
                (false, Skip::No) => return None,
                _ => {}
            }
        }
    }

    /// Ligature substitution: the first ligature for the glyph at `n` whose other components
    /// follow (skipping what the lookup skips). Marks between components stay after it, each
    /// with its component.
    fn ligature(&mut self, doing: &Doing, list: Bytes, glyphs: &mut Vec<Laid>, n: usize) -> Option<usize> {
        let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
        let set = list.u16(6 + nth * 2).filter(|_| list.u16(4).is_some_and(|count| nth < count as usize))? as usize;
        for each in 0..list.u16(set)? as usize {
            let ligature = set + list.u16(set + 2 + each * 2)? as usize;
            let (glyph, parts) = (list.u16(ligature)?, list.u16(ligature + 2)?.max(1) as usize);
            if self.matched(doing, glyphs, n, parts, |m, laid| list.u16(ligature + 4 + (m - 1) * 2) == Some(laid.glyph)) {
                return Some(self.ligate(glyphs, glyph));
            }
        }
        None
    }

    /// Whether `count` glyphs from `n` match (`n` itself first; `fits` judges the others),
    /// skipping what the lookup skips; and a ligature may not combine glyphs of different
    /// components of earlier ligatures. The positions go in `positions`.
    fn matched(&mut self, doing: &Doing, glyphs: &[Laid], n: usize, count: usize, fits: impl Fn(usize, &Laid) -> bool) -> bool {
        self.positions.clear();
        self.positions.push(n);
        let (first_id, first_comp) = (glyphs[n].lig_id, glyphs[n].component());
        for m in 1..count {
            let Some(at) = self.along(doing, doing.flags, glyphs, self.positions[m - 1], (false, false), |laid| fits(m, laid)) else { return false };
            let (id, comp) = (glyphs[at].lig_id, glyphs[at].component());
            let apart = match first_id != 0 && first_comp != 0 {
                true => first_id != id || first_comp != comp,
                false => id != 0 && comp != 0 && id != first_id,
            };
            if apart {
                return false;
            }
            self.positions.push(at);
        }
        true
    }

    /// Ligates the glyphs at `positions` into `glyph`, in the first one's place; the marks
    /// between are told their component. Returns the next glyph.
    fn ligate(&mut self, glyphs: &mut Vec<Laid>, glyph: u16) -> usize {
        let positions = std::mem::take(&mut self.positions);
        let first = positions[0];
        let marks_only = positions.iter().all(|at| glyphs[*at].mark());
        let base_only = !glyphs[first].mark() && positions[1..].iter().all(|at| glyphs[*at].mark());
        let is_ligature = !marks_only && !base_only;
        let total: u8 = positions.iter().map(|at| glyphs[*at].parts()).sum::<u8>().min(15);
        let id = if is_ligature { self.next_id() } else { 0 };
        let (mut last_id, mut last_parts) = (glyphs[first].lig_id, glyphs[first].parts());
        let mut so_far = last_parts;
        for (m, at) in positions.iter().copied().enumerate().skip(1) {
            if is_ligature {
                (positions[m - 1] + 1..at).for_each(|between| part_of(&mut glyphs[between], id, so_far, last_parts));
            }
            (last_id, last_parts) = (glyphs[at].lig_id, glyphs[at].parts());
            so_far = so_far.saturating_add(last_parts);
        }
        // Marks after the ligature that belonged to the last component of an earlier one now
        // belong to this one.
        let end = positions[positions.len() - 1];
        for after in glyphs[end + 1..].iter_mut() {
            if marks_only || last_id == 0 || after.lig_id != last_id || after.component() == 0 {
                break;
            }
            part_of(after, id, so_far, last_parts);
        }
        let laid = &mut glyphs[first];
        let class = if is_ligature { LIGATURE } else { laid.props & (BASE | LIGATURE | MARK | 0xff00) };
        (laid.glyph, laid.props) = (glyph, props(self.gdef, glyph, (laid.props & !MULTIPLIED & !(BASE | LIGATURE | MARK | 0xff00)) | SUBSTITUTED | LIGATED | class));
        if is_ligature {
            (laid.lig_id, laid.lig_comp, laid.lig_base) = (id, total, true);
        }
        for at in positions[1..].iter().rev() {
            glyphs.remove(*at);
        }
        end + 1 - (positions.len() - 1)
    }

    /// A new ligature id: one to seven, wrapping, as HarfBuzz numbers them.
    fn next_id(&mut self) -> u8 {
        self.serial = self.serial.wrapping_add(1);
        if self.serial & 7 == 0 {
            self.serial = self.serial.wrapping_add(1);
        }
        self.serial & 7
    }

    /// Whether a contextual subtable's rule matches at `n`, and if so applies its lookups at
    /// the glyphs it names. Rules match the glyphs from `n` on (and, `chained`, backtrack and
    /// lookahead). Returns the next glyph.
    fn rule(&mut self, doing: &Doing<'a>, chained: bool, list: Bytes<'a>, glyphs: &mut Vec<Laid>, n: usize) -> Option<usize> {
        let (end, records) = self.rule_holds(doing, chained, list, glyphs, n)?;
        let positions = std::mem::take(&mut self.positions);
        Some(self.call(doing, list, records, positions, end, glyphs))
    }

    /// Whether a subtable's rule matches at `n`: where its input ends, and where its lookup
    /// records are (and how many). Three formats: rule sets per first glyph naming glyphs (1);
    /// the same with classes (2); or one rule with a coverage per glyph (3).
    fn rule_holds(&mut self, doing: &Doing, chained: bool, list: Bytes, glyphs: &[Laid], n: usize) -> Option<(usize, (usize, usize))> {
        let written = list.u16(0)?;
        let classes = match (written, chained) {
            (1, _) => [0; 3],
            (2, false) => [list.u16(4)? as usize; 3],
            (2, true) => [list.u16(4)?, list.u16(6)?, list.u16(8)?].map(usize::from),
            (3, _) => return self.holds(doing, list, 2, (chained, 3), [0; 3], glyphs, n),
            _ => return None,
        };
        let sets = match (written, chained) {
            (1, _) => 4,
            (2, false) => 6,
            _ => 10,
        };
        let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
        let which = if written == 1 { nth } else { class(list, classes[1], glyphs[n].glyph) };
        let set = list.u16(sets + 2 + which * 2).filter(|at| *at != 0 && list.u16(sets).is_some_and(|count| which < count as usize))? as usize;
        (0..list.u16(set)? as usize).find_map(|m| self.holds(doing, list, set + list.u16(set + 2 + m * 2)? as usize, (chained, written), classes, glyphs, n))
    }

    /// Whether the rule at `body` matches at `n`. A rule has up to three sequences, each after
    /// its count: backtrack (nearest first), input from `n`, and lookahead. The first input
    /// glyph is named only in format 3: a rule set already belongs to it. Then its lookup
    /// records: sequence index and lookup index.
    #[allow(clippy::too_many_arguments)]
    fn holds(&mut self, doing: &Doing, list: Bytes, body: usize, (chained, written): (bool, u16), classes: [usize; 3], glyphs: &[Laid], n: usize) -> Option<(usize, (usize, usize))> {
        let (count, unnamed) = (|at: usize| list.u16(at).map(usize::from), (written != 3) as usize);
        let (rows, records) = match chained {
            true => {
                let from = body + 2 + count(body)? * 2;
                let after = from + 2 + count(from)?.checked_sub(unnamed)? * 2;
                let records = after + 2 + count(after)? * 2;
                ([(body + 2, count(body)?), (from + 2, count(from)?), (after + 2, count(after)?)], (records + 2, count(records)?))
            }
            false => ([(0, 0), (body + 4, count(body)?), (0, 0)], (body + 4 + count(body)?.checked_sub(unnamed)? * 2, count(body + 2)?)),
        };
        // A value is a glyph, a class, or a coverage offset.
        let fits = |row: usize, m: usize, laid: &Laid| match (list.u16(rows[row].0 + m * 2), written) {
            (Some(number), 1) => laid.glyph == number,
            (Some(number), 2) => class(list, classes[row], laid.glyph) == number as usize,
            (Some(number), _) => covered(list, number as usize, laid.glyph).is_some(),
            _ => false,
        };
        let [(_, before), (_, long), (_, after)] = rows;
        if long == 0 || (written == 3 && !fits(1, 0, &glyphs[n])) {
            return None;
        }
        if !self.matched(doing, glyphs, n, long, |m, laid| fits(1, m - unnamed, laid)) {
            return None;
        }
        let mut at = n;
        for m in 0..before {
            at = self.along(doing, doing.flags, glyphs, at, (true, true), |laid| fits(0, m, laid))?;
        }
        let mut at = self.positions[self.positions.len() - 1];
        for m in 0..after {
            at = self.along(doing, doing.flags, glyphs, at, (false, true), |laid| fits(2, m, laid))?;
        }
        Some((self.positions[self.positions.len() - 1] + 1, records))
    }

    /// Applies a matched rule's lookups, each once, at the glyph its record names: glyphs added
    /// or removed shift the positions after. A called lookup may be contextual itself, its
    /// matching rule calling lookups in turn (up to `DEEPEST`): those wait on a stack, before
    /// the rest of the caller's. Returns the next glyph: after the rule's input.
    fn call(&mut self, doing: &Doing<'a>, list: Bytes<'a>, records: (usize, usize), positions: Vec<usize>, end: usize, glyphs: &mut Vec<Laid>) -> usize {
        let mut calls = vec![Call { doing: *doing, list, records, next: 0, positions, end, from: (0, 0, 0) }];
        loop {
            let top = calls.len() - 1;
            if calls[top].next < calls[top].records.1 {
                let record = calls[top].records.0 + calls[top].next * 4;
                calls[top].next += 1;
                let inner = self.carry_record(&mut calls[top], record, glyphs);
                calls.extend(inner.filter(|_| calls.len() < DEEPEST));
                continue;
            }
            let done = calls.pop().unwrap_or_else(|| unreachable!());
            let Some(caller) = calls.last_mut() else { return done.end };
            let (nth, place, before) = done.from;
            caller.moved(nth, place, glyphs.len() as isize - before as isize);
        }
    }

    /// Applies one lookup a rule calls, at the glyph it names: a contextual lookup returns its
    /// matching rule, to apply in turn.
    fn carry_record(&mut self, call: &mut Call<'a>, record: usize, glyphs: &mut Vec<Laid>) -> Option<Call<'a>> {
        let (table, moves) = (call.doing.table, call.doing.moves);
        let (nth, lookup) = (call.list.u16(record)? as usize, call.list.u16(record + 2)?);
        let place = call.positions.get(nth).copied().filter(|place| *place < glyphs.len())?;
        let at = lookup_at(table, lookup)?;
        let (flags, lists) = (table.u16(at + 2).unwrap_or(0), table.u16(at + 4).unwrap_or(0) as usize);
        let marks = if flags & MARK_SET != 0 { table.u16(at + 6 + lists * 2).unwrap_or(0) } else { 0 };
        let called = Doing { flags, marks, ..call.doing };
        let far = if moves { 9 } else { 7 };
        let (kind, _) = lists_at(table, at, far).next()?;
        let before = glyphs.len();
        // (Reverse chaining lookups are not called by rules.)
        if !moves && kind == 8 || !self.spend(lists) {
            return None;
        }
        if !matches!((moves, kind), (false, 5 | 6) | (true, 7 | 8)) {
            self.apply(&called, at, glyphs, place)?;
            call.moved(nth, place, glyphs.len() as isize - before as isize);
            return None;
        }
        let chained = matches!(kind, 6 | 8);
        let holding = lists_at(table, at, far).find_map(|(_, list)| self.rule_holds(&called, chained, Bytes(table.0.get(list..)?), glyphs, place).map(|holds| (holds, list)));
        let ((end, records), list) = holding?;
        let positions = std::mem::take(&mut self.positions);
        Some(Call { doing: called, list: Bytes(table.0.get(list..)?), records, next: 0, positions, end, from: (nth, place, before) })
    }

    /// Pair positioning: by the second glyph listed for the first (1), or by both glyphs'
    /// classes (2). Returns the next glyph: the second, or after it if it was moved too.
    fn pair(&mut self, doing: &Doing, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
        let first = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
        let second = self.along(doing, doing.flags, glyphs, n, (false, false), seen)?;
        let (formats, glyph) = ([list.u16(4)?, list.u16(6)?], glyphs[second].glyph);
        let sizes = formats.map(|format| format.count_ones() as usize * 2);
        let values = match list.u16(0)? {
            1 => {
                let set = list.u16(10 + first * 2).filter(|_| list.u16(8).is_some_and(|count| first < count as usize))? as usize;
                let (count, long) = (list.u16(set)? as usize, 2 + sizes[0] + sizes[1]);
                let m = crate::font_file::first_not(count, |m| Some(list.u16(set + 2 + m * long)? < glyph))?;
                (m < count && list.u16(set + 2 + m * long)? == glyph).then_some(set + 4 + m * long)?
            }
            2 => {
                let (one, two) = (class(list, list.u16(8)? as usize, glyphs[n].glyph), class(list, list.u16(10)? as usize, glyph));
                let (ones, twos) = (list.u16(12)? as usize, list.u16(14)? as usize);
                (one < ones && two < twos).then_some(16 + (one * twos + two) * (sizes[0] + sizes[1]))?
            }
            _ => return None,
        };
        value(list, values, formats[0], &mut glyphs[n]);
        value(list, values + sizes[0], formats[1], &mut glyphs[second]);
        Some(if formats[1] != 0 { second + 1 } else { second })
    }

    /// Cursive attachment (slanted Arabic): the previous glyph's exit anchor and this one's
    /// entry anchor meet.
    fn stroke(&mut self, doing: &Doing, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
        let records = |glyph: u16| covered(list, list.u16(2)? as usize, glyph).filter(|nth| list.u16(4).is_some_and(|count| *nth < count as usize)).map(|nth| 6 + nth * 4);
        let entry = list.u16(records(glyphs[n].glyph)?).filter(|at| *at != 0)? as usize;
        let before = self.along(doing, doing.flags, glyphs, n, (true, false), seen)?;
        let exit = list.u16(records(glyphs[before].glyph)? + 2).filter(|at| *at != 0)? as usize;
        let ([exit_x, exit_y], [entry_x, entry_y]) = (anchor(list, exit)?, anchor(list, entry)?);
        let (i, j) = (before, n);
        match self.leftward {
            true => {
                let d = exit_x + glyphs[i].offset[0];
                (glyphs[i].advance, glyphs[i].offset[0]) = (glyphs[i].advance - d, glyphs[i].offset[0] - d);
                glyphs[j].advance = entry_x + glyphs[j].offset[0];
            }
            false => {
                glyphs[i].advance = exit_x + glyphs[i].offset[0];
                let d = entry_x + glyphs[j].offset[0];
                (glyphs[j].advance, glyphs[j].offset[0]) = (glyphs[j].advance - d, glyphs[j].offset[0] - d);
            }
        }
        // The child attaches to the parent: the earlier glyph to the later in a right-to-left
        // lookup, else the reverse.
        let (child, parent, y) = if doing.flags & LEFTWARD_FLAG != 0 { (i, j, entry_y - exit_y) } else { (j, i, exit_y - entry_y) };
        unchain(glyphs, child, parent);
        (glyphs[child].attach, glyphs[child].chain, glyphs[child].offset[1]) = (2, (parent as isize - child as isize) as i16, y);
        if glyphs[parent].chain == -glyphs[child].chain {
            (glyphs[parent].chain, glyphs[parent].offset[1]) = (0, 0);
        }
        Some(n + 1)
    }

    /// Mark-to-base: a mark attaches to the previous non-mark: its anchor meets that glyph's
    /// anchor for the mark's class.
    fn on_base(&mut self, doing: &Doing, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
        let mark = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
        let only = Doing { flags: MARK, ..*doing };
        let bases = list.u16(4)? as usize;
        let mut at = n;
        let base = loop {
            at = self.along(&only, MARK, glyphs, at, (true, false), seen)?;
            // (Of a multiplied glyph, only the first takes marks.)
            if taken(glyphs, at) || covered(list, bases, glyphs[at].glyph).is_some() {
                break at;
            }
        };
        let onto = covered(list, bases, glyphs[base].glyph)?;
        fix(list, mark, onto, 0, glyphs, n, base)
    }

    /// Mark-to-ligature: a mark attaches to the previous ligature, at its own component or the
    /// last.
    fn on_ligature(&mut self, doing: &Doing, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
        let mark = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
        let only = Doing { flags: MARK, ..*doing };
        let base = self.along(&only, MARK, glyphs, n, (true, false), seen)?;
        let ligature = covered(list, list.u16(4)? as usize, glyphs[base].glyph)?;
        let array = list.u16(10)? as usize;
        let attach = array + list.u16(array + 2 + ligature * 2).filter(|_| list.u16(array).is_some_and(|count| ligature < count as usize))? as usize;
        let parts = list.u16(attach).filter(|parts| *parts != 0)? as usize;
        let (id, own, comp) = (glyphs[base].lig_id, glyphs[n].lig_id, glyphs[n].component() as usize);
        let part = if id != 0 && id == own && comp > 0 { parts.min(comp) - 1 } else { parts - 1 };
        fix(list, mark, part, attach - array, glyphs, n, base)
    }

    /// Mark-to-mark: a mark attaches to the previous mark, where both belong to the same base.
    fn on_mark(&mut self, doing: &Doing, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
        let mark = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
        let before = self.along(doing, doing.flags & !IGNORE, glyphs, n, (true, false), seen)?;
        if !glyphs[before].mark() {
            return None;
        }
        let (id1, id2, comp1, comp2) = (glyphs[n].lig_id, glyphs[before].lig_id, glyphs[n].component(), glyphs[before].component());
        let same = match id1 == id2 {
            true => id1 == 0 || comp1 == comp2,
            false => (id1 > 0 && comp1 == 0) || (id2 > 0 && comp2 == 0),
        };
        if !same {
            return None;
        }
        let onto = covered(list, list.u16(4)? as usize, glyphs[before].glyph)?;
        fix(list, mark, onto, 0, glyphs, n, before)
    }
}

/// Whether a glyph is visible: what `GPOS` takes as something to position or attach to when
/// scanning (it skips invisible ones, unless the lookup says otherwise, then it stops there).
fn seen(laid: &Laid) -> bool {
    !laid.ignorable()
}

/// Tells a mark, or a component of an earlier ligature, which component of new ligature `id` it
/// belongs to: of the components of the glyph it belonged to (`last` of them; zero means the
/// last), counted after the `so_far` components before it.
fn part_of(laid: &mut Laid, id: u8, so_far: u8, last: u8) {
    let own = if laid.component() == 0 { last } else { laid.component() };
    (laid.lig_id, laid.lig_comp, laid.lig_base) = (id, so_far - last + own.min(last), false);
}

/// Shifts a rule's matched positions from the `next`th on, after a lookup it called at `place`
/// changed the count by `delta` (added glyphs come right after it; removed ones were after it),
/// as HarfBuzz does.
fn shift_places(positions: &mut Vec<usize>, next: usize, place: usize, delta: isize) {
    let shift = |position: usize| (position as isize + delta).max(0) as usize;
    if delta > 0 {
        let added: Vec<usize> = (1..=delta as usize).map(|k| place + k).collect();
        let rest: Vec<usize> = positions[next..].iter().map(|position| shift(*position)).collect();
        positions.truncate(next);
        positions.extend(added.into_iter().chain(rest));
        return;
    }
    let gone = (-delta as usize).min(positions.len() - next);
    positions.drain(next..next + gone);
    positions[next..].iter_mut().for_each(|position| *position = shift(*position));
}

/// Whether a glyph may take marks: one not multiplied, or the first of those.
fn taken(glyphs: &[Laid], at: usize) -> bool {
    let laid = &glyphs[at];
    let Some(before) = at.checked_sub(1).map(|before| &glyphs[before]) else { return true };
    laid.props & MULTIPLIED == 0 || laid.component() == 0 || before.mark() || before.props & MULTIPLIED == 0 || laid.lig_id != before.lig_id || laid.component() != before.component() + 1
}

/// Attaches the mark at `n` to the glyph at `onto`: the mark's anchor (by its index `mark` in
/// the mark array) meets the other's anchor for its class, at its index `place` (in an array
/// `from` bytes after the ligature's component anchors start, for a ligature).
fn fix(list: Bytes, mark: usize, place: usize, from: usize, glyphs: &mut [Laid], n: usize, onto: usize) -> Option<usize> {
    let (classes, marks, others) = (list.u16(6)? as usize, list.u16(8)? as usize, list.u16(10)? as usize + from);
    let within = |count: Option<u16>, m: usize| count.is_some_and(|count| m < count as usize);
    let class = list.u16(marks + 2 + mark * 4).filter(|_| within(list.u16(marks), mark))? as usize;
    let own = marks + list.u16(marks + 4 + mark * 4)? as usize;
    let theirs = list.u16(others + 2 + (place * classes + class) * 2).filter(|at| *at != 0 && class < classes && within(list.u16(others), place))?;
    let ([x, y], [base_x, base_y]) = (anchor(list, own)?, anchor(list, others + theirs as usize)?);
    // (As in HarfBuzz from 14.3, a mark moves vertically with its base as far as the base had
    // moved when attached, no further: a later vertical move of the base leaves the mark.
    // Horizontally it follows to the end.)
    let up = if marks_follow_later_moves() { 0 } else { across(glyphs, onto) };
    let laid = &mut glyphs[n];
    (laid.offset, laid.attach, laid.chain) = ([base_x - x, base_y - y + up], 1, (onto as isize - n as isize) as i16);
    Some(n + 1)
}

/// How far a glyph was moved up, with the glyphs it hangs from cursively.
fn across(glyphs: &[Laid], mut at: usize) -> i32 {
    let mut up = glyphs[at].offset[1];
    for _ in 1..glyphs.len() {
        let parent = at as isize + glyphs[at].chain as isize;
        if glyphs[at].attach != 2 || glyphs[at].chain == 0 || !(0..glyphs.len() as isize).contains(&parent) {
            break;
        }
        at = parent as usize;
        up += glyphs[at].offset[1];
    }
    up
}

/// Whether marks follow every vertical move of their base, as in HarfBuzz before 14.3: only in
/// tests against an older HarfBuzz.
fn marks_follow_later_moves() -> bool {
    #[cfg(test)]
    return older::MARKS.get();
    #[cfg(not(test))]
    false
}

/// An anchor as `GPOS` writes it: its format, then x and y.
fn anchor(list: Bytes, at: usize) -> Option<[i32; 2]> {
    Some([list.i16(at + 2)? as i32, list.i16(at + 4)? as i32])
}

/// Reverses the cursive chain a glyph was attached by, before attaching it anew, so what hung
/// from it now hangs from the new parent.
fn unchain(glyphs: &mut [Laid], child: usize, parent: usize) {
    let mut path = Vec::new();
    let mut at = child;
    while glyphs[at].chain != 0 && glyphs[at].attach == 2 && path.len() < 64 {
        let next = (at as isize + glyphs[at].chain as isize) as usize;
        glyphs[at].chain = 0;
        if next == parent || next >= glyphs.len() {
            break;
        }
        path.push((at, next));
        at = next;
    }
    for (from, to) in path.into_iter().rev() {
        let chain = (from as isize - to as isize) as i16;
        (glyphs[to].offset[1], glyphs[to].chain, glyphs[to].attach) = (-glyphs[from].offset[1], chain, 2);
    }
}

/// Adds a `GPOS` value record to a glyph: the values at `at`, a `format` bit each: x placement
/// (1), y placement (2), x advance (4); and four more unused here.
fn value(list: Bytes, at: usize, format: u16, laid: &mut Laid) {
    let mut place = at;
    for bit in 0..8 {
        if format & (1 << bit) == 0 {
            continue;
        }
        let number = list.i16(place).unwrap_or(0) as i32;
        match bit {
            0 => laid.offset[0] += number,
            1 => laid.offset[1] += number,
            2 => laid.advance += number,
            _ => {}
        }
        place += 2;
    }
}

/// Single positioning: one value for all of the subtable's glyphs (1), or one each (2).
fn moved(list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
    let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
    let format = list.u16(4)?;
    let at = match list.u16(0)? {
        1 => 6,
        2 => 8 + nth * format.count_ones() as usize * 2,
        _ => return None,
    };
    (list.u16(0)? == 1 || list.u16(6).is_some_and(|count| nth < count as usize)).then_some(())?;
    value(list, at, format, &mut glyphs[n]);
    Some(n + 1)
}

/// Single substitution: by a delta (1), or listed (2).
fn one_for_one(gdef: Bytes, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
    let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
    let glyph = match list.u16(0)? {
        1 => glyphs[n].glyph.wrapping_add(list.u16(4)?),
        2 => list.u16(6 + nth * 2).filter(|_| list.u16(4).is_some_and(|count| nth < count as usize))?,
        _ => return None,
    };
    swap(gdef, &mut glyphs[n], glyph);
    Some(n + 1)
}

/// Alternate substitution: the first alternate takes its place, as HarfBuzz does for enabled
/// features (which select the first).
fn one_of_several(gdef: Bytes, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
    let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
    let set = list.u16(6 + nth * 2).filter(|_| list.u16(4).is_some_and(|count| nth < count as usize))? as usize;
    let glyph = list.u16(set + 2).filter(|_| list.u16(set).is_some_and(|count| count > 0))?;
    swap(gdef, &mut glyphs[n], glyph);
    Some(n + 1)
}

/// Replaces a glyph, which then has its own `GDEF` class (where none, the old one's).
fn swap(gdef: Bytes, laid: &mut Laid, glyph: u16) {
    laid.glyph = glyph;
    laid.props = props(gdef, glyph, laid.props | SUBSTITUTED);
}

/// Multiple substitution: one glyph becomes several, or none, each a component of it.
fn one_for_several(gdef: Bytes, list: Bytes, glyphs: &mut Vec<Laid>, n: usize) -> Option<usize> {
    let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
    let row = list.u16(6 + nth * 2).filter(|_| list.u16(4).is_some_and(|count| nth < count as usize))? as usize;
    let count = list.u16(row)? as usize;
    let old = glyphs[n];
    let mut new = Vec::with_capacity(count);
    for m in 0..count {
        let mut laid = old;
        // (Without a `GDEF` class, a ligature's component is a base, any other's what the glyph
        // was.)
        let guess = if old.props & LIGATURE != 0 { (old.props & !(LIGATURE | MARK | 0xff00)) | BASE } else { old.props };
        laid.props = guess | MULTIPLIED;
        swap(gdef, &mut laid, list.u16(row + 2 + m * 2)?);
        (laid.lig_id, laid.lig_comp, laid.lig_base) = (0, m.min(15) as u8, false);
        new.push(laid);
    }
    glyphs.splice(n..n + 1, new);
    Some(n + count)
}

/// Reverse chaining single substitution: one glyph replaced where backtrack and lookahead
/// match.
fn backward_rule(shaper: &Shaper, doing: &Doing, list: Bytes, glyphs: &mut [Laid], n: usize) -> Option<usize> {
    let nth = covered(list, list.u16(2)? as usize, glyphs[n].glyph)?;
    let befores = list.u16(4)? as usize;
    let afters_at = 6 + befores * 2;
    let afters = list.u16(afters_at)? as usize;
    let swaps_at = afters_at + 2 + afters * 2;
    let glyph = list.u16(swaps_at + 2 + nth * 2).filter(|_| list.u16(swaps_at).is_some_and(|count| nth < count as usize))?;
    let mut at = n;
    for m in 0..befores {
        let cover = list.u16(6 + m * 2)? as usize;
        at = shaper.along(doing, doing.flags, glyphs, at, (true, true), |laid| covered(list, cover, laid.glyph).is_some())?;
    }
    let mut at = n;
    for m in 0..afters {
        let cover = list.u16(afters_at + 2 + m * 2)? as usize;
        at = shaper.along(doing, doing.flags, glyphs, at, (false, true), |laid| covered(list, cover, laid.glyph).is_some())?;
    }
    swap(shaper.gdef, &mut glyphs[n], glyph);
    Some(n)
}

/// Places each attached glyph relative to its parent, by what lies between: a mark from its
/// base, and a cursive glyph vertically with the one it hangs from. (In right-to-left text, the
/// glyphs between come after the base.)
fn fix_attached(glyphs: &mut [Laid], leftward: bool, downward: bool) {
    let mut path = Vec::new();
    for start in 0..glyphs.len() {
        path.clear();
        let mut at = start;
        while glyphs[at].chain != 0 && path.len() < 64 {
            let next = at as isize + glyphs[at].chain as isize;
            glyphs[at].chain = 0;
            if next < 0 || next as usize >= glyphs.len() {
                break;
            }
            path.push((at, next as usize));
            at = next as usize;
        }
        for (child, parent) in path.iter().rev().copied() {
            follow(glyphs, child, parent, leftward, downward);
        }
    }
}

/// Places one glyph relative to the one it attaches to (in vertical text, the glyphs between
/// are passed going up).
fn follow(glyphs: &mut [Laid], child: usize, parent: usize, leftward: bool, downward: bool) {
    if glyphs[child].attach == 2 {
        glyphs[child].offset[1] += glyphs[parent].offset[1];
        return;
    }
    let [x, y] = glyphs[parent].offset;
    glyphs[child].offset[0] += x;
    glyphs[child].offset[1] += if marks_follow_later_moves() { y } else { 0 };
    let between: i32 = match leftward {
        false => -glyphs[parent.min(child)..child.max(parent)].iter().map(|laid| laid.advance).sum::<i32>(),
        true => glyphs[parent.min(child) + 1..child.max(parent) + 1].iter().map(|laid| laid.advance).sum::<i32>(),
    };
    match downward {
        true => glyphs[child].offset[1] -= between,
        false => glyphs[child].offset[0] += between,
    }
}

/// Zeroes mark glyphs' advances; with `hang`, each is still drawn where it was, hanging over
/// the glyph before.
fn zero_marks(glyphs: &mut [Laid], hang: bool) {
    for laid in glyphs.iter_mut().filter(|laid| laid.mark()) {
        laid.offset[0] -= if hang { laid.advance } else { 0 };
        laid.advance = 0;
    }
}

// ---- what a lookup would do ----

/// Whether a `GSUB` lookup would substitute exactly these glyphs, the first where a rule
/// starts: what HarfBuzz asks of the Indic consonant form features. With `alone`, a rule that
/// also checks context does not count.
pub(crate) fn would_apply(table: Bytes, lookup: u16, glyphs: &[u16], alone: bool) -> bool {
    let Some(at) = lookup_at(table, lookup).filter(|_| !glyphs.is_empty()) else { return false };
    lists_at(table, at, 7).any(|(kind, list)| table.0.get(list..).and_then(|list| would(Bytes(list), kind, glyphs, alone)).unwrap_or(false))
}

/// Whether one subtable of a `GSUB` lookup, of a type, would substitute the glyphs.
fn would(list: Bytes, kind: u16, glyphs: &[u16], alone: bool) -> Option<bool> {
    let written = list.u16(0)?;
    let first = || covered(list, list.u16(2)? as usize, glyphs[0]);
    match (kind, written) {
        (1..=3 | 8, _) => Some(glyphs.len() == 1 && first().is_some()),
        (4, _) => would_ligate(list, first()?, glyphs),
        (5 | 6, 3) => would_cover(list, kind == 6, glyphs, alone),
        (5 | 6, 1 | 2) => would_rule(list, kind == 6, written, first()?, glyphs, alone),
        _ => Some(false),
    }
}

/// Whether a ligature subtable has one of exactly these glyphs, the first being the `nth`
/// covered glyph.
fn would_ligate(list: Bytes, nth: usize, glyphs: &[u16]) -> Option<bool> {
    let set = list.u16(6 + nth * 2).filter(|_| list.u16(4).is_some_and(|count| nth < count as usize))? as usize;
    let count = list.u16(set)? as usize;
    let holds = |ligature: usize| {
        let parts = list.u16(ligature + 2).unwrap_or(0) as usize;
        parts == glyphs.len() && (1..parts).all(|k| list.u16(ligature + 4 + (k - 1) * 2) == Some(glyphs[k]))
    };
    Some((0..count).any(|m| list.u16(set + 2 + m * 2).is_some_and(|at| holds(set + at as usize))))
}

/// Whether a contextual subtable (by glyphs, or by classes: `written`) has a rule for exactly
/// these glyphs, the first being the `nth` covered.
fn would_rule(list: Bytes, chained: bool, written: u16, nth: usize, glyphs: &[u16], alone: bool) -> Option<bool> {
    let (classes, sets) = match (written, chained) {
        (1, _) => (0, 4),
        (_, false) => (list.u16(4)? as usize, 6),
        (_, true) => (list.u16(6)? as usize, 10),
    };
    let which = if written == 1 { nth } else { class(list, classes, glyphs[0]) };
    let set = list.u16(sets + 2 + which * 2).filter(|at| *at != 0 && list.u16(sets).is_some_and(|count| which < count as usize))? as usize;
    let fits = |number: u16, glyph: u16| if written == 1 { number == glyph } else { class(list, classes, glyph) == number as usize };
    let count = list.u16(set)? as usize;
    let holds = |rule: usize| rule_would(list, rule, chained, glyphs, alone, &fits).unwrap_or(false);
    Some((0..count).any(|m| list.u16(set + 2 + m * 2).is_some_and(|at| holds(set + at as usize))))
}

/// Whether the rule at `rule` names exactly these glyphs after the first (and, with `alone`, no
/// context).
fn rule_would(list: Bytes, rule: usize, chained: bool, glyphs: &[u16], alone: bool, fits: &dyn Fn(u16, u16) -> bool) -> Option<bool> {
    let before = if chained { list.u16(rule)? as usize } else { 0 };
    let input = if chained { rule + 2 + before * 2 } else { rule };
    let count = list.u16(input)? as usize;
    let values = if chained { input + 2 } else { rule + 4 };
    let after = if chained { list.u16(values + count.saturating_sub(1) * 2)? as usize } else { 0 };
    if (alone && before + after > 0) || count != glyphs.len() {
        return Some(false);
    }
    Some((1..count).all(|k| list.u16(values + (k - 1) * 2).is_some_and(|number| fits(number, glyphs[k]))))
}

/// Whether a coverage-based rule matches exactly these glyphs.
fn would_cover(list: Bytes, chained: bool, glyphs: &[u16], alone: bool) -> Option<bool> {
    let before = if chained { list.u16(2)? as usize } else { 0 };
    let input = if chained { 4 + before * 2 } else { 2 };
    let count = list.u16(input)? as usize;
    let coverages = if chained { input + 2 } else { 6 };
    let after = if chained { list.u16(coverages + count * 2)? as usize } else { 0 };
    if (alone && before + after > 0) || count != glyphs.len() {
        return Some(false);
    }
    Some((0..count).all(|k| list.u16(coverages + k * 2).is_some_and(|at| covered(list, at as usize, glyphs[k]).is_some())))
}

// What HarfBuzz itself makes of text, for the tests of `shaping.rs` and `brahmic.rs`.
#[cfg(test)]
pub(crate) mod harfbuzz {
    /// The system HarfBuzz's shaping of some words in the font at `path` (face `face` of a
    /// collection): each glyph with its position (left to right), and the total advance, in
    /// font units. Nothing without HarfBuzz.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[cfg(any(feature = "right-to-left", feature = "brahmic"))]
    pub(crate) fn harfbuzz(path: &std::path::Path, face: u32, words: &[String]) -> Option<Vec<Shaped>> {
        harfbuzz_in(path, face, words, "")
    }

    /// The same, for words in a language as BCP 47 names it ("" for none). And from then on
    /// this thread places marks as that HarfBuzz does: before 14.3, marks followed every
    /// vertical move of their base (see `older`).
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub(crate) fn harfbuzz_in(path: &std::path::Path, face: u32, words: &[String], language: &str) -> Option<Vec<Shaped>> {
        shaped_by(path, face, words, language, false)
    }

    /// The same, vertical: each glyph's position (from the first's pen, up), and the total
    /// advance (down: negative).
    #[cfg(target_os = "linux")]
    pub(crate) fn harfbuzz_downward(path: &std::path::Path, face: u32, words: &[String]) -> Option<Vec<Shaped>> {
        shaped_by(path, face, words, "", true)
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn shaped_by(path: &std::path::Path, face: u32, words: &[String], language: &str, downward: bool) -> Option<Vec<Shaped>> {
        use std::ffi::{CString, c_char, c_int, c_uint, c_void};
        unsafe extern "C" {
            fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
            fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        }
        type Told = [i32; 5];
        // SAFETY: the library is opened by name and its functions found by theirs; each is
        // called as HarfBuzz declares it, on objects it made.
        unsafe {
            let library = if cfg!(target_os = "macos") { dlopen(c"/opt/homebrew/lib/libharfbuzz.0.dylib".as_ptr(), 2) } else { dlopen(c"libharfbuzz.so.0".as_ptr(), 2) };
            let function = |name: &std::ffi::CStr| Some(dlsym(library, name.as_ptr())).filter(|found| !library.is_null() && !found.is_null());
            let from_file: extern "C" fn(*const c_char) -> *mut c_void = std::mem::transmute(function(c"hb_blob_create_from_file")?);
            let face_of: extern "C" fn(*mut c_void, c_uint) -> *mut c_void = std::mem::transmute(function(c"hb_face_create")?);
            let font_of: extern "C" fn(*mut c_void) -> *mut c_void = std::mem::transmute(function(c"hb_font_create")?);
            let buffer: extern "C" fn() -> *mut c_void = std::mem::transmute(function(c"hb_buffer_create")?);
            let destroy: extern "C" fn(*mut c_void) = std::mem::transmute(function(c"hb_buffer_destroy")?);
            let add: extern "C" fn(*mut c_void, *const c_char, c_int, c_uint, c_int) = std::mem::transmute(function(c"hb_buffer_add_utf8")?);
            let guess: extern "C" fn(*mut c_void) = std::mem::transmute(function(c"hb_buffer_guess_segment_properties")?);
            let flags: extern "C" fn(*mut c_void, c_uint) = std::mem::transmute(function(c"hb_buffer_set_flags")?);
            let shape: extern "C" fn(*mut c_void, *mut c_void, *const c_void, c_uint) = std::mem::transmute(function(c"hb_shape")?);
            let glyphs: extern "C" fn(*mut c_void, *mut c_uint) -> *const Told = std::mem::transmute(function(c"hb_buffer_get_glyph_infos")?);
            let places: extern "C" fn(*mut c_void, *mut c_uint) -> *const Told = std::mem::transmute(function(c"hb_buffer_get_glyph_positions")?);
            let language_of: extern "C" fn(*const c_char, c_int) -> *const c_void = std::mem::transmute(function(c"hb_language_from_string")?);
            let set_language: extern "C" fn(*mut c_void, *const c_void) = std::mem::transmute(function(c"hb_buffer_set_language")?);
            let at_least: extern "C" fn(c_uint, c_uint, c_uint) -> c_int = std::mem::transmute(function(c"hb_version_atleast")?);
            let direction: extern "C" fn(*mut c_void, c_uint) = std::mem::transmute(function(c"hb_buffer_set_direction")?);
            super::older::MARKS.set(at_least(14, 3, 0) == 0);
            let font = font_of(face_of(from_file(CString::new(path.to_str()?).ok()?.as_ptr()), face));
            let named = CString::new(language).ok()?;
            let one = |word: &String| {
                let (written, mut count) = (buffer(), 0);
                add(written, word.as_ptr().cast(), word.len() as c_int, 0, word.len() as c_int);
                if !language.is_empty() {
                    set_language(written, language_of(named.as_ptr(), -1));
                }
                if downward {
                    direction(written, 6);
                }
                guess(written);
                flags(written, 8);
                shape(font, written, std::ptr::null(), 0);
                let (glyphs, places) = (glyphs(written, &mut count), places(written, &mut count));
                let (glyphs, places) = (std::slice::from_raw_parts(glyphs, count as usize), std::slice::from_raw_parts(places, count as usize));
                let (mut pen, mut placed) = (0, Vec::new());
                for (glyph, place) in glyphs.iter().zip(places) {
                    match downward {
                        true => (placed.push((glyph[0] as u16, [place[2], pen + place[3]])), pen += place[1]).1,
                        false => (placed.push((glyph[0] as u16, [pen + place[2], place[3]])), pen += place[0]).1,
                    }
                }
                destroy(written);
                (placed, pen)
            };
            Some(words.iter().map(one).collect())
        }
    }

    /// A shaped word: each glyph with its position, and the total advance.
    pub(crate) type Shaped = (Vec<(u16, [i32; 2])>, i32);
}

// How HarfBuzz placed marks before 14.3, for tests against such a version.
#[cfg(test)]
pub(crate) mod older {
    thread_local! {
        /// Whether marks follow every vertical move of their base, on this thread.
        pub(crate) static MARKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
}

// (All of these are right-to-left text: see `bidi.rs` for the feature.)
#[cfg(test)]
#[cfg(feature = "right-to-left")]
pub(crate) mod tests {
    use super::harfbuzz::Shaped;
    #[cfg(target_os = "linux")]
    use super::harfbuzz::harfbuzz;
    use super::*;

    /// Real Hebrew, Arabic, Persian and Urdu words: everyday and interface words; words with
    /// every letter of each alphabet in each form; lam-alef in all its kinds; words with vowel
    /// signs and other marks, several per letter; with ZWNJ, and with tatweel.
    pub(crate) const WORDS: [&str; 157] = [
        "שלום",
        "עולם",
        "ספר",
        "ילד",
        "ילדה",
        "בית",
        "מחשב",
        "תוכנה",
        "קובץ",
        "שמור",
        "פתח",
        "סגור",
        "ביטול",
        "אישור",
        "עריכה",
        "העתק",
        "הדבק",
        "חיפוש",
        "הגדרות",
        "עזרה",
        "מסמך",
        "תיקייה",
        "שגיאה",
        "הודעה",
        "משתמש",
        "סיסמה",
        "כניסה",
        "יציאה",
        "ירושלים",
        "ישראל",
        "תודה",
        "בבקשה",
        "בוקר",
        "ערב",
        "לילה",
        "שבת",
        "אהבה",
        "חבר",
        "עברית",
        "מילון",
        "צפון",
        "דרך",
        "גזר",
        "טלפון",
        "ץ",
        "ך",
        "ם",
        "ן",
        "ף",
        "צ׳יפס",
        "בית־ספר",
        "״שלום״",
        "שָׁלוֹם",
        "בְּרֵאשִׁית",
        "יִשְׂרָאֵל",
        "אֱלֹהִים",
        "הַשָּׁמַיִם",
        "וְאֵת",
        "הָאָרֶץ",
        "מֶלֶךְ",
        "תּוֹרָה",
        "דָּבָר",
        "שַׁבָּת",
        "כֹּל",
        "مرحبا",
        "السلام",
        "عليكم",
        "العربية",
        "كتاب",
        "مكتبة",
        "ملف",
        "حفظ",
        "فتح",
        "إغلاق",
        "إلغاء",
        "موافق",
        "تحرير",
        "نسخ",
        "لصق",
        "بحث",
        "إعدادات",
        "مساعدة",
        "مستند",
        "مجلد",
        "خطأ",
        "رسالة",
        "مستخدم",
        "كلمة",
        "المرور",
        "دخول",
        "خروج",
        "القاهرة",
        "بغداد",
        "دمشق",
        "شكرا",
        "صباح",
        "مساء",
        "ليل",
        "محمد",
        "الله",
        "لا",
        "لأن",
        "إلى",
        "الأولى",
        "لآ",
        "للغة",
        "قال",
        "فيه",
        "في",
        "بين",
        "هذا",
        "التي",
        "ذلك",
        "شيء",
        "يوم",
        "سنة",
        "طالب",
        "ظلام",
        "غرفة",
        "ضوء",
        "ثلاثة",
        "كائن",
        "مسؤول",
        "هيئة",
        "ء",
        "ى",
        "ة",
        "بِسْمِ",
        "اللَّهِ",
        "الرَّحْمَٰنِ",
        "الرَّحِيمِ",
        "مُحَمَّدٌ",
        "كِتَابٌ",
        "لَا",
        "الْعَرَبِيَّةُ",
        "قُرْآن",
        "شُكْرًا",
        "مَدْرَسَةٌ",
        "عـربـي",
        "سلام",
        "خداحافظ",
        "فارسی",
        "کتاب",
        "گربه",
        "پدر",
        "چای",
        "ژاله",
        "می‌خواهم",
        "خانه‌ها",
        "اردو",
        "پاکستان",
        "شکریہ",
        "ہے",
        "میں",
        "ٹھیک",
        "بڑا",
        "ڈاکٹر",
    ];

    /// The fonts with Hebrew or Arabic letters that may exist: the system's, those of a Windows
    /// this Linux runs under, and with `TAUT_FONTS` more (`:`-separated paths).
    #[cfg(target_os = "linux")]
    fn fonts() -> Vec<std::path::PathBuf> {
        let linux = [
            "truetype/dejavu/DejaVuSans.ttf",
            "truetype/dejavu/DejaVuSans-Bold.ttf",
            "truetype/dejavu/DejaVuSansMono.ttf",
            "noto/NotoSansArabic-Regular.ttf",
            "noto/NotoNaskhArabic-Regular.ttf",
            "noto/NotoNaskhArabicUI-Regular.ttf",
            "noto/NotoKufiArabic-Regular.ttf",
            "noto/NotoNastaliqUrdu-Regular.ttf",
            "noto/NotoSansHebrew-Regular.ttf",
            "noto/NotoSerifHebrew-Regular.ttf",
        ];
        let windows = ["segoeui", "segoeuib", "seguisb", "arial", "arialbd", "tahoma", "tahomabd", "times", "timesbd", "cour", "courbd", "micross"];
        let named = std::env::var("TAUT_FONTS").unwrap_or_default();
        let all = linux
            .iter()
            .map(|path| format!("/usr/share/fonts/{path}"))
            .chain(windows.iter().map(|name| format!("/mnt/c/Windows/Fonts/{name}.ttf")))
            .chain(named.split(':').filter(|path| !path.is_empty()).map(String::from));
        all.map(std::path::PathBuf::from).filter(|path| path.exists()).collect()
    }

    /// This library's shaping of a word: by its script, right to left.
    pub(crate) fn ours(font: &FontFile, plans: &mut Vec<(Script, Plan)>, word: &str) -> Shaped {
        let script = word.chars().find_map(Script::of).unwrap_or(Script::None);
        if !plans.iter().any(|(known, _)| *known == script) {
            plans.push((script, Plan::of(font, script, true, Language::NONE)));
        }
        let plan = &plans.iter().find(|(known, _)| *known == script).unwrap().1;
        let mut glyphs = Vec::new();
        let total = font.shape(plan, word, &mut glyphs);
        let mut pen = 0;
        let placed = glyphs.iter().map(|laid| (laid.glyph, [(pen, pen += laid.advance).0 + laid.offset[0], laid.offset[1]])).collect();
        (placed, total)
    }

    #[test]
    fn the_letters_of_words_take_their_forms_in_the_systems_font() {
        // The system font for each script: Segoe UI on Windows, elsewhere what the desktop
        // names. Every letter of every word has a glyph, within the word.
        for (script, letter) in [(Script::Hebrew, 'א'), (Script::Arabic, 'ب')] {
            let Some(font) = crate::bidi::system(crate::text::Face::Regular, letter) else {
                eprintln!("no font for {script:?} on this system: not tested");
                continue;
            };
            let mut plans = Vec::new();
            for word in WORDS.iter().filter(|word| word.chars().find_map(Script::of) == Some(script) && word.chars().all(|letter| font.glyph(letter) != 0 || unicode(letter) != 0)) {
                let (glyphs, total) = ours(&font, &mut plans, word);
                assert!(glyphs.iter().all(|(glyph, _)| *glyph != 0), "{word}: {glyphs:?}");
                // (Marks sit on their letters: within the word, not after it.)
                assert!(glyphs.iter().all(|(_, [x, _])| (-total / 4..total).contains(x)), "{word}: {glyphs:?} in {total}");
            }
            if script == Script::Hebrew {
                continue;
            }
            // In Arabic, joining letters take their contextual forms, each other than its
            // isolated glyph; and lam-alef is one ligature, or two glyphs made for each other
            // (as in Segoe UI), never the two plain letters.
            for word in ["\u{645}\u{62d}\u{645}\u{62f}", "\u{644}\u{627}"] {
                let alone: Vec<u16> = word.chars().rev().map(|letter| font.glyph(letter)).collect();
                let joined: Vec<u16> = ours(&font, &mut plans, word).0.iter().map(|(glyph, _)| *glyph).collect();
                assert!(joined.len() <= alone.len() && joined.iter().zip(&alone).all(|(joined, alone)| joined != alone), "{word}: {joined:?} and {alone:?}");
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn words_come_to_the_glyphs_harfbuzz_makes_of_them() {
        // With `TAUT_WORDS` naming a file of more words, one per line, those too.
        let more = std::env::var_os("TAUT_WORDS").and_then(|path| std::fs::read_to_string(path).ok()).unwrap_or_default();
        let words: Vec<String> = WORDS.iter().map(|word| word.to_string()).chain(more.lines().map(str::trim).filter(|line| !line.is_empty()).map(String::from)).collect();
        let fonts = fonts();
        if fonts.is_empty() {
            return eprintln!("no font with Hebrew or Arabic on this system: not tested");
        }
        let mut all_differ = Vec::new();
        for path in fonts {
            let font = FontFile::open(&path).expect("the font is read");
            // (Only words in scripts the font has letters of.)
            let has = |word: &&String| word.chars().filter(|letter| Script::of(*letter).is_some() && kind(*letter) != Kind::Mark).all(|letter| font.glyph(letter) != 0);
            let words: Vec<String> = words.iter().filter(has).cloned().collect();
            let Some(theirs) = harfbuzz(&path, 0, &words) else { return eprintln!("no HarfBuzz on this system: not tested") };
            let (mut plans, mut differ) = (Vec::new(), Vec::new());
            for (word, theirs) in words.iter().zip(theirs) {
                let ours = ours(&font, &mut plans, word);
                differ.extend((ours != theirs).then(|| format!("{word} ({}): {ours:?}, not {theirs:?}", word.escape_unicode())));
            }
            println!("{}: {} words, {} differ", path.display(), words.len(), differ.len());
            differ.iter().take(6).for_each(|line| println!("  {line}"));
            all_differ.extend(differ.into_iter().map(|line| format!("{}: {line}", path.display())));
        }
        assert!(all_differ.is_empty(), "{} words differ", all_differ.len());
    }
}

// Shaping by script, language, variation selector and fraction: tested with hand-made fonts
// everywhere, and against HarfBuzz word for word on Linux.
#[cfg(test)]
pub(crate) mod held {
    use super::harfbuzz::Shaped;
    use super::*;
    use crate::font_file::tests::{Named, file, longs, positions, tables, with, words};
    use crate::scripts::{numbered, of, tag};

    /// This library's shaping of a word in a language: by the script of its first letter with
    /// one (as HarfBuzz guesses it), right-to-left where that letter is; each glyph with its
    /// position, and the total advance.
    pub(crate) fn ours(font: &FontFile, plans: &mut Vec<(Option<Script>, Plan)>, word: &str, language: Language) -> Shaped {
        let first = word.chars().find(|letter| Script::of(*letter).is_some());
        let script = first.and_then(Script::of);
        // (As HarfBuzz has it, by script: Arabic digits in Arabic text run right to left.)
        let leftward = first.is_some_and(|letter| matches!(kind(letter), Kind::Right | Kind::Arabic) || matches!(script, Some(Script::Arabic | Script::Syriac | Script::Hebrew)));
        if !plans.iter().any(|(known, _)| *known == script) {
            plans.push((script, Plan::of(font, script.unwrap_or(Script::None), leftward, language)));
        }
        let plan = &plans.iter().find(|(known, _)| *known == script).unwrap().1;
        let mut glyphs = Vec::new();
        let total = font.shape(plan, word, &mut glyphs);
        let mut pen = 0;
        let placed = glyphs.iter().map(|laid| (laid.glyph, [(pen, pen += laid.advance).0 + laid.offset[0], laid.offset[1]])).collect();
        (placed, total)
    }

    #[test]
    fn every_character_is_of_the_script_unicode_says() {
        let named = |letter: char| String::from_utf8_lossy(tag(of(letter))).into_owned();
        let letters = ['A', 'ž', 'Ж', 'Ω', 'ܐ', 'ߊ', '\u{1e900}', 'ᠠ', 'ꡀ', '漢', 'あ', 'ア', '한', 'ა', 'Ա', 'ހ', 'ሀ', 'ᐁ'];
        let names = ["latn", "latn", "cyrl", "grek", "syrc", "nko ", "adlm", "mong", "phag", "hani", "kana", "kana", "hang", "geor", "armn", "thaa", "ethi", "cans"];
        assert_eq!(letters.map(named), names);
        // Figures, spaces, signs and common marks have no script.
        assert_eq!(['1', ' ', '.', '\u{301}', '\u{200d}', '\u{fe0f}', '\u{1f600}', '\u{640}'].map(of), [0; 8]);
        assert_eq!(['A', 'Ω', 'Ж', '1'].map(Script::of), [Some(Script::Plain(numbered(b"latn"))), Some(Script::Plain(numbered(b"grek"))), Some(Script::Plain(numbered(b"cyrl"))), None]);
        assert_eq!(['ب', 'ܐ', 'א'].map(Script::of), [Some(Script::Arabic), Some(Script::Syriac), Some(Script::Hebrew)]);
        // The window kerns by scripts of its own, which are these for Latin, Greek and
        // Cyrillic.
        let kerned = |letter: char| match tag(of(letter)) {
            b"latn" => 1,
            b"grek" => 2,
            b"cyrl" => 3,
            _ => 0,
        };
        let differ: Vec<String> =
            (0..0x30000).filter_map(char::from_u32).filter(|letter| crate::font_file::script(*letter) != kerned(*letter)).map(|letter| format!("{:04x}", letter as u32)).collect();
        assert!(differ.is_empty(), "the window's scripts differ for {} characters: {:?}", differ.len(), &differ[..differ.len().min(20)]);
    }

    #[test]
    fn languages_are_named_as_harfbuzz_names_them() {
        let systems = |name: &str| Language::named(name).systems().map(|system| String::from_utf8_lossy(system).into_owned()).collect::<Vec<_>>().join(",");
        let names = [
            "sr",
            "sr-Latn-RS",
            "sr_RS",
            "mk",
            "ro",
            "tr",
            "nl",
            "pl",
            "ca",
            "de",
            "ja",
            "ko",
            "zh",
            "zh-Hans",
            "zh-Hant",
            "zh-TW",
            "zh-HK",
            "zh-Hant-HK",
            "zh-MO",
            "yue",
            "zh-yue",
            "el-polyton",
            "syr-Syre",
            "xyz",
            "en",
            "",
            "x",
        ];
        let said = [
            "SRB ",
            "SRB ",
            "SRB ",
            "MKD ",
            "ROM ",
            "TRK ",
            "NLD ",
            "PLK ",
            "CAT ",
            "DEU ",
            "JAN ",
            "KOR ,KOH ",
            "ZHS ",
            "ZHS ",
            "ZHT ",
            "ZHT ",
            "ZHH ",
            "ZHH ",
            "ZHTM,ZHH ",
            "ZHH ",
            "ZHH ",
            "PGR ",
            "SYRE",
            "XYZ ",
            "ENG ",
            "",
            "",
        ];
        assert_eq!(names.map(systems), said);
        assert_eq!(Language::default(), Language::NONE);
    }

    #[test]
    fn figures_around_a_fraction_slash_are_told_what_they_are() {
        let told = |text: &str, leftward: bool| {
            let mut forms = vec![0; text.chars().count()];
            fractions(text.chars(), leftward, &mut forms);
            forms
        };
        let (n, d, f) = (NUMERATOR | FRACTION, DENOMINATOR | FRACTION, FRACTION);
        assert_eq!(told("a12\u{2044}345b", false), [0, n, n, f, d, d, d, 0]);
        // Figures of any script count; a slash without a figure on one side makes no fraction;
        // and in right-to-left text the numerator is after the slash.
        assert_eq!(told("\u{663}\u{2044}\u{664}", false), [n, f, d]);
        assert_eq!([told("1\u{2044}", false), told("\u{2044}2", false), told("1/2", false)], [vec![0, 0], vec![0, 0], vec![0, 0, 0]]);
        assert_eq!(told("1\u{2044}2", true), [d, f, n]);
    }

    #[test]
    fn syriac_letters_take_their_forms_after_alaph_dalath_and_rish() {
        let forms = |text: &str| {
            let mut forms = vec![0; text.chars().count()];
            joined(text.chars(), &mut forms);
            forms
        };
        // Alaph alone, after a joining letter, after a non-joining one, after dalath.
        assert_eq!(forms("\u{710}"), [ALONE]);
        assert_eq!(forms("\u{712}\u{710}"), [FIRST, LAST]);
        assert_eq!(forms("\u{710}\u{710}"), [ALONE, LAST_TWO]);
        assert_eq!(forms("\u{715}\u{710}"), [ALONE, LAST_THREE]);
        // An alaph mid-word after a final alaph, before a joining letter.
        assert_eq!(forms("\u{712}\u{710}\u{712}"), [FIRST, MIDDLE_TWO, ALONE]);
        // Arabic and N'Ko as before; a mark is skipped; Mongolian's selector takes its letter's
        // form.
        assert_eq!(forms("\u{628}\u{64e}\u{628}"), [FIRST, 0, LAST]);
        assert_eq!(forms("\u{7ca}\u{7cb}\u{7cc}"), [FIRST, MIDDLE, LAST]);
        assert_eq!(forms("\u{1820}\u{180b}\u{1821}"), [FIRST, FIRST, LAST]);
    }

    /// The hand-made font's tables with a `cmap` that has variation sequences too: "A" with
    /// selector 1 is its default glyph, with selector 2 glyph 5; "B" with selector 2 glyph 6.
    fn varied() -> Vec<u8> {
        let usual = [longs(&[1]), vec![0, 0, 0x41, 0]].concat();
        let own = [longs(&[2]), vec![0, 0, 0x41, 0, 5, 0, 0, 0x42, 0, 6]].concat();
        let head = 10 + 2 * 11;
        let record = |selector: u32, usual_at: u32, own_at: u32| [&selector.to_be_bytes()[1..], &longs(&[usual_at, own_at])].concat();
        let records = [record(0xfe00, head as u32, 0), record(0xfe01, 0, (head + usual.len()) as u32)].concat();
        let length = head + usual.len() + own.len();
        let fourteen = [words(&[14]), longs(&[length as u32, 2]), records, usual, own].concat();
        let cmap = tables().iter().find(|(name, _)| name == b"cmap").unwrap().1.clone();
        // The format 4 subtable for the BMP, after a list of two.
        let four = cmap[12..].to_vec();
        let lists = [words(&[0, 2, 0, 5]), longs(&[20]), words(&[3, 1]), longs(&[20 + fourteen.len() as u32])].concat();
        file(&[0, 1, 0, 0], &with(tables(), b"cmap", [lists, fourteen, four].concat()))
    }

    #[test]
    fn a_variation_selector_picks_the_glyph_the_font_names() {
        let font = FontFile::read(varied()).unwrap();
        let (a, b) = (font.glyph('A'), font.glyph('B'));
        assert_eq!([font.variant('A', '\u{fe00}'), font.variant('A', '\u{fe01}'), font.variant('B', '\u{fe01}')], [Some(a), Some(5), Some(6)]);
        assert_eq!([font.variant('B', '\u{fe00}'), font.variant('A', '\u{fe02}'), font.variant('C', '\u{fe01}')], [None; 3]);
        // In a word, a character and its selector are one glyph; an unknown selector goes after
        // the font's lookups, which skip it.
        let shaped = |text: &str| ours(&font, &mut Vec::new(), text, Language::NONE).0.iter().map(|(glyph, _)| *glyph).collect::<Vec<_>>();
        assert_eq!([shaped("A\u{fe01}B"), shaped("B\u{fe00}A"), shaped("A\u{fe00}\u{fe01}")], [vec![5, b], vec![b, a], vec![a]]);
    }

    /// A `GSUB` table with one feature, `locl`, under Latin: no lookup for no language; for
    /// Turkish the one replacing glyph 1 (the font's "A") with glyph 3; the same for a `dflt`
    /// language system.
    fn local(dflt: bool) -> Vec<u8> {
        let language = if dflt { b"dflt" } else { b"TRK " };
        // A script: its default features (zero), then one language's.
        let script = [words(&[10, 1]), language.to_vec(), words(&[16, 0, 0xffff, 0, 0, 0xffff, 1, 1])].concat();
        let scripts = [words(&[1]), b"latn".to_vec(), words(&[8]), script].concat();
        let features = [words(&[2]), b"locl".to_vec(), words(&[14]), b"locl".to_vec(), words(&[18, 0, 0, 0, 1, 0])].concat();
        // Lookup 0: single substitution by delta (glyph 1 becomes 3).
        let list = words(&[1, 6, 2, 1, 1, 1]);
        let lookups = [words(&[1, 4, 1, 0, 1, 8]), list].concat();
        let at = [10, 10 + scripts.len(), 10 + scripts.len() + features.len()];
        [words(&[1, 0, at[0] as i32, at[1] as i32, at[2] as i32]), scripts, features, lookups].concat()
    }

    #[test]
    fn a_fonts_forms_for_a_language_are_used_for_text_in_it() {
        for dflt in [false, true] {
            let font = FontFile::read(file(&[0, 1, 0, 0], &[tables(), vec![(*b"GSUB", local(dflt))]].concat())).unwrap();
            let shaped = |language: Language| ours(&font, &mut Vec::new(), "AB", language).0.iter().map(|(glyph, _)| *glyph).collect::<Vec<_>>();
            let (a, b) = (font.glyph('A'), font.glyph('B'));
            match dflt {
                // Turkish has its own form; Dutch and no language have none.
                false => assert_eq!([shaped(Language::named("tr")), shaped(Language::named("nl")), shaped(Language::NONE)], [vec![3, b], vec![a, b], vec![a, b]]),
                // A font's `dflt` stands for any language it does not name, and for none.
                true => assert_eq!([shaped(Language::named("tr")), shaped(Language::NONE)], [vec![3, b], vec![3, b]]),
            }
        }
    }

    /// A `GSUB` 1.1 table whose one feature, `locl` under Latin, names no lookup; but where the
    /// font's first axis is between `low` and `high` (in F2DOT14), a variation substitutes the
    /// lookup replacing glyph 1 with 3.
    fn varying(low: i32, high: i32) -> Vec<u8> {
        let scripts = [words(&[1]), b"latn".to_vec(), words(&[8, 4, 0, 0, 0xffff, 1, 0])].concat();
        let features = [words(&[1]), b"locl".to_vec(), words(&[8, 0, 0])].concat();
        let lookups = words(&[1, 4, 1, 0, 1, 8, 1, 6, 2, 1, 1, 1]);
        let conditions = [words(&[1]), longs(&[6]), words(&[1, 0, low, high])].concat();
        let instead = [words(&[1, 0, 1, 0]), longs(&[12]), words(&[0, 1, 0])].concat();
        let variations = [words(&[1, 0]), longs(&[1, 16, 16 + conditions.len() as u32]), conditions, instead].concat();
        let at = [14, 14 + scripts.len(), 14 + scripts.len() + features.len(), 14 + scripts.len() + features.len() + lookups.len()];
        [words(&[1, 1, at[0] as i32, at[1] as i32, at[2] as i32]), longs(&[at[3] as u32]), scripts, features, lookups, variations].concat()
    }

    #[test]
    fn a_variable_fonts_features_are_those_for_where_it_is_drawn() {
        // At the default position (zero on every axis): the substitutes whose conditions hold
        // there.
        for ((low, high), glyph) in [((-8192, 8192), 3), ((8192, 16384), 1), ((-16384, 0), 3)] {
            let font = FontFile::read(file(&[0, 1, 0, 0], &[tables(), vec![(*b"GSUB", varying(low, high))]].concat())).unwrap();
            assert_eq!(ours(&font, &mut Vec::new(), "A", Language::NONE).0[0].0, glyph, "{low}..{high}");
        }
    }

    #[test]
    fn the_apps_own_letters_take_the_forms_of_a_language_where_it_asks() {
        use crate::text::{Font, TextSystem};
        let read = || FontFile::read(file(&[0, 1, 0, 0], &[tables(), vec![(*b"GSUB", local(false))]].concat())).unwrap();
        let (font, size) = (read(), Font::new(1024.0));
        let [usual, turkish] = [font.width(font.glyph('A')), font.width(3)];
        // Unless the app asks, letters take the no-language forms, whatever the window's
        // language; asked, the language's, and the window's when set.
        let mut plain = crate::font_file::AppFonts::new(read());
        plain.set_language(Language::named("tr"));
        let mut asked = crate::font_file::AppFonts::new(read()).language(Language::named("tr"));
        assert_eq!([plain.measure("A", size, 1.0, None), asked.measure("A", size, 1.0, None)], [usual, turkish]);
        asked.set_language(Language::named("nl"));
        assert_eq!(asked.measure("A", size, 1.0, None), usual);
    }

    /// Fonts with Latin, Greek and Cyrillic letters that may exist on this system.
    fn western() -> Vec<std::path::PathBuf> {
        let windows = std::env::var_os("WINDIR").map_or("/mnt/c/Windows".into(), std::path::PathBuf::from).join("Fonts");
        let names = ["segoeui.ttf", "arial.ttf", "times.ttf", "tahoma.ttf", "verdana.ttf", "georgia.ttf", "calibri.ttf", "cour.ttf"];
        let linux = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
        ];
        names.iter().map(|name| windows.join(name)).chain(linux.iter().map(std::path::PathBuf::from)).filter(|path| path.exists()).collect()
    }

    #[test]
    fn the_window_and_the_page_measure_latin_greek_and_cyrillic_alike() {
        use crate::text::{Face, Font, TextSystem};
        // Words with the most kerned pairs, and none of the letters they ligate.
        let words = ["AVATAR", "WAVE", "Typography", "Toyota", "LTAV", "Yo", "P.", "Привет", "МОСКВА", "ТУАЛЕТ", "Україна", "ΑΥΤΟ", "Καλημέρα", "ΤΑΥΤΟΤΗΤΑ", "A1V"];
        let fonts = western();
        if fonts.is_empty() {
            return eprintln!("no font with Latin, Greek and Cyrillic letters on this system: not tested");
        }
        for path in fonts {
            let units = FontFile::open(&path).unwrap().units();
            let mut own = crate::font_file::AppFonts::new(FontFile::open(&path).unwrap());
            let mut paper = crate::paper::Paper::new();
            (paper.language, _) = (Language::NONE, paper.font(Face::Regular, FontFile::open(&path).unwrap()));
            let font = Font::new(units);
            let file = FontFile::open(&path).unwrap();
            for word in words.iter().filter(|word| word.chars().all(|letter| file.glyph(letter) != 0 || crate::scripts::of(letter) == 0)) {
                let window = own.measure(word, font, 1.0, None);
                let page = paper.stops(word, font, &mut Vec::new());
                assert_eq!(window, page, "{}: {word}", path.display());
            }
        }
    }

    #[test]
    fn latin_is_shaped_by_what_the_font_has_for_latin() {
        // Kerning only under `latn` (as DejaVu Sans has it): the page kerns "AB" as the window
        // does, and Cyrillic text by the font's default script (none here).
        let list = crate::font_file::tests::by_seconds(&[1, 1, 1], [4, 0], &[&[1, 2, -40]]);
        let scripts: [Named; 2] = [(b"DFLT", &[1]), (b"latn", &[0])];
        let features: [Named; 2] = [(b"kern", &[0]), (b"cpsp", &[0])];
        let font = FontFile::read(file(&[0, 1, 0, 0], &[tables(), vec![(*b"GPOS", positions(&scripts, &features, &[(2, vec![list])]))]].concat())).unwrap();
        let (placed, pen) = ours(&font, &mut Vec::new(), "AB", Language::NONE);
        assert_eq!(placed[1].1[0], font.width(font.glyph('A')) as i32 - 40);
        assert_eq!(pen as f32, font.advance('A', 1024.0) + font.advance('B', 1024.0) + font.kerning('A', 'B', 1024.0));
    }

    /// Words tested against HarfBuzz in every font with their letters: Latin, Greek and
    /// Cyrillic everyday words, kerning pairs, ligatures and accented letters; fractions;
    /// characters with variation selectors (emoji text or picture presentation, Phags-pa's
    /// mirrored letters, Mongolian forms); and after a line `=` and a language, words in that
    /// language, where fonts have forms of their own.
    #[cfg(target_os = "linux")]
    const WORDS: &str = "
        AVATAR WAVE Typography Toyota LTAV TA Te To Tr Ty Va Vo Wa Yo AT Ay F. P, r. y. 'A' \"Yes\" (Hi) [J] {j}
        office affluent fjord fifty waffle flight shuffle fi fl ff ffi ffl Th ct st
        Straße naïve façade ÆØÅ Œuvre Ångström Dvořák Łódź Zürich Cañón Þórr ðæ Việt Nguyễn Tiếng ǅ Ǆ ĳ ŉ ǰ
        Καλημέρα ΑΥΤΟ Ελληνικά ΤΑΥΤΟΤΗΤΑ ψυχή ἀρχῇ ΆΈΉΊΌΎΏ ϋΰ Ὦ ᾯ
        Привет МОСКВА ТУАЛЕТ Україна Ђорђе бвгдпт Љубљана ЃЌЅ ӁӐӔ щъыьэюя Ѣѣ Ѳѳ
        1⁄2 3⁄4 10⁄100 1/2 ½ x² ⁄5 7⁄ ١⁄٢ ۱⁄۲ ①
        ☺\u{fe0e} ☺\u{fe0f} ❤\u{fe0e} ❤\u{fe0f} #\u{fe0f}\u{20e3} ↔\u{fe0e} ✌\u{fe0f} ©\u{fe0e} ™\u{fe0f} 0\u{fe0f}\u{20e3}
        ꡖ\u{fe00} ꡜ\u{fe00} ꡞ\u{fe00} ꡟ\u{fe00} ꡠ\u{fe00} ꡀꡖ\u{fe00} ꡀꡨ\u{fe00} ꡀꡁꡂ
        ᠠ\u{180b} ᠠ\u{180c}ᠠ ᠠᠠ\u{180c}ᠠ ᠠᠡ\u{180b} ᠨ\u{180b}ᠠ ᠠᠨ\u{180d}ᠠ ᠭ\u{180b}ᠠ ᠠᠭ\u{180d}ᠠ ᠮᠣᠩᠭᠣᠯ ᠬᠡᠯᠡ ᠪᠢᠴᠢᠭ
        =sr бгдпт Ђорђе шта
        =mk бгдпт ѓќ
        =bg вдгжзийклптцчшщъю
        =ro şţŞŢ ș ț
        =tr fi fil ﬁ İi
        =nl ijs IJs ĳ IJ
        =pl ąćęłńóśźż ÓŚĆŹŃ
        =ca l·l L·L
        =ja 直 骨 化 今 令 雪 角 曜 刃
        =ko 直 骨 化 今 令 雪 角 曜 刃
        =zh-Hans 直 骨 化 今 令 雪 角 曜 刃
        =zh-Hant 直 骨 化 今 令 雪 角 曜 刃
        =zh-HK 直 骨 化 今 令 雪 角 曜 刃
    ";

    /// Words with their languages, from text written as `WORDS` is.
    #[cfg(target_os = "linux")]
    fn words_of(text: &str) -> Vec<(String, String)> {
        let mut language = String::new();
        let mut all = Vec::new();
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
            match line.strip_prefix('=') {
                Some(named) => language = named.trim().to_string(),
                None => all.extend(line.split_whitespace().map(|word| (language.clone(), word.to_string()))),
            }
        }
        all
    }

    /// The fonts to test words against HarfBuzz in, each with its face index: the system's,
    /// Windows' (under `/mnt/c` in WSL), and with `TAUT_HELD_FONTS` more (`:`-separated paths,
    /// each with `#` and a face index for a collection).
    #[cfg(target_os = "linux")]
    pub(crate) fn fonts() -> Vec<(std::path::PathBuf, u32)> {
        let linux = [
            "truetype/dejavu/DejaVuSans.ttf",
            "truetype/dejavu/DejaVuSans-Bold.ttf",
            "truetype/dejavu/DejaVuSansMono.ttf",
            "truetype/dejavu/DejaVuSerif.ttf",
            "truetype/ubuntu/Ubuntu-R.ttf",
            "truetype/ubuntu/Ubuntu-B.ttf",
            "truetype/ubuntu/UbuntuMono-R.ttf",
            "truetype/ubuntu/Ubuntu[wdth,wght].ttf",
            "truetype/ubuntu/UbuntuSans[wdth,wght].ttf",
            "noto/NotoSans-Regular.ttf",
            "noto/NotoSans-Bold.ttf",
            "noto/NotoSerif-Regular.ttf",
            "noto/NotoSansMono-Regular.ttf",
            "noto/NotoColorEmoji.ttf",
            "noto/NotoSansMongolian-Regular.ttf",
            "noto/NotoSansPhagsPa-Regular.ttf",
            "Adwaita/AdwaitaSans-Regular.ttf",
            "Adwaita/AdwaitaMono-Regular.ttf",
        ];
        let windows = [
            "segoeui",
            "segoeuib",
            "segoeuisl",
            "arial",
            "arialbd",
            "times",
            "timesbd",
            "cour",
            "tahoma",
            "verdana",
            "georgia",
            "calibri",
            "consola",
            "candara",
            "corbel",
            "constan",
            "trebuc",
            "sylfaen",
            "micross",
            "seguisym",
            "seguiemj",
            "monbaiti",
            "phagspa",
            "bahnschrift",
        ];
        let named = std::env::var("TAUT_HELD_FONTS").unwrap_or_default();
        let all = linux
            .iter()
            .map(|path| format!("/usr/share/fonts/{path}"))
            .chain(windows.iter().map(|name| format!("/mnt/c/Windows/Fonts/{name}.ttf")))
            .chain(["/mnt/c/Windows/Fonts/cambria.ttc#0".to_string(), "/mnt/c/Windows/Fonts/msgothic.ttc#0".to_string(), "/mnt/c/Windows/Fonts/YuGothR.ttc#0".to_string()])
            .chain(named.split(':').filter(|path| !path.is_empty()).map(String::from));
        let split = |path: String| match path.rsplit_once('#') {
            Some((path, face)) => (std::path::PathBuf::from(path), face.parse().unwrap_or(0)),
            None => (std::path::PathBuf::from(path), 0),
        };
        all.map(split).filter(|(path, _)| path.exists()).collect()
    }

    /// A font's ideographic variation sequences (`cmap` 14, selectors from U+E0100), as words:
    /// at most `most`, spread over all.
    #[cfg(target_os = "linux")]
    fn sequences(font: &FontFile, most: usize) -> Vec<String> {
        let all = font.bytes();
        let Some(cmap) = crate::font_file::listed(all, font.start, b"cmap") else { return Vec::new() };
        let table = all.within(&cmap);
        let lists = table.u16(2).unwrap_or(0) as usize;
        let list = (0..lists).find(|n| table.u16(4 + n * 8) == Some(0) && table.u16(6 + n * 8) == Some(5)).and_then(|n| table.u32(8 + n * 8));
        let Some(list) = list.map(|at| crate::font_file::Bytes(&table.0[at as usize..])) else { return Vec::new() };
        let mut found = Vec::new();
        for n in 0..list.u32(6).unwrap_or(0) as usize {
            let selector = list.u24(10 + n * 11).unwrap_or(0);
            let (usual, own) = (list.u32(13 + n * 11).unwrap_or(0) as usize, list.u32(17 + n * 11).unwrap_or(0) as usize);
            let Some(selector) = char::from_u32(selector).filter(|selector| *selector >= '\u{e0100}') else { continue };
            for m in (0..if usual == 0 { 0 } else { list.u32(usual).unwrap_or(0) as usize }).step_by(7) {
                found.extend(list.u24(usual + 4 + m * 4).and_then(char::from_u32).map(|letter| format!("{letter}{selector}")));
            }
            for m in (0..if own == 0 { 0 } else { list.u32(own).unwrap_or(0) as usize }).step_by(3) {
                found.extend(list.u24(own + 4 + m * 5).and_then(char::from_u32).map(|letter| format!("{letter}{selector}")));
            }
        }
        let step = found.len().div_ceil(most.max(1)).max(1);
        found.into_iter().step_by(step).collect()
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn words_of_every_script_come_to_the_glyphs_harfbuzz_makes_of_them() {
        // With `TAUT_HELD_WORDS` naming files of more words (`:`-separated), those too.
        let more: String =
            std::env::var("TAUT_HELD_WORDS").unwrap_or_default().split(':').filter(|path| !path.is_empty()).map(|path| std::fs::read_to_string(path).expect("the words are read") + "\n=\n").collect();
        let words = words_of(&format!("{WORDS}\n=\n{more}"));
        let fonts = fonts();
        let (mut tried, mut all_differ) = (0, Vec::new());
        for (path, face) in fonts {
            let Some(font) = FontFile::open(&path).and_then(|font| if face == 0 { Some(font) } else { font.nth(face as usize) }) else { continue };
            // (Only words whose letters the font has: except an unknown selector, shaped as its
            // own glyph, and figures and signs, which are the font's notdef where missing, in
            // both.)
            let has = |word: &&(String, String)| word.1.chars().filter(|letter| Script::of(*letter).is_some() || crate::emoji::picture(*letter, &word.1) > 0).all(|letter| font.glyph(letter) != 0);
            // (Words only a feature shapes as HarfBuzz does are tested only with it:
            // right-to-left ones, syllabic ones.)
            let first = |word: &str| word.chars().find(|letter| of(*letter) != 0);
            let leftward = |word: &str| {
                first(word).is_some_and(|letter| matches!(kind(letter), Kind::Right | Kind::Arabic) || matches!(Script::of(letter), Some(Script::Arabic | Script::Syriac | Script::Hebrew)))
            };
            let syllabic = |word: &str| first(word).is_some_and(|letter| (of(letter) as usize) <= crate::scripts::BRAHMIC);
            let shaped = |word: &&(String, String)| (cfg!(feature = "right-to-left") || !leftward(&word.1)) && (cfg!(feature = "brahmic") || !syllabic(&word.1));
            let has = |word: &&(String, String)| has(word) && shaped(word);
            let mut held: Vec<(String, String)> = words.iter().filter(has).cloned().collect();
            held.extend(sequences(&font, 400).into_iter().map(|word| (String::new(), word)));
            let mut languages: Vec<String> = held.iter().map(|(language, _)| language.clone()).collect();
            languages.sort();
            languages.dedup();
            let mut differ = Vec::new();
            for language in &languages {
                let these: Vec<String> = held.iter().filter(|(said, _)| said == language).map(|(_, word)| word.clone()).collect();
                let Some(theirs) = super::harfbuzz::harfbuzz_in(&path, face, &these, language) else { return eprintln!("no HarfBuzz on this system: not tested") };
                let mut plans = Vec::new();
                for (word, theirs) in these.iter().zip(theirs) {
                    let mine = ours(&font, &mut plans, word, Language::named(language));
                    differ.extend((mine != theirs).then(|| format!("[{language}] {word} ({}): {mine:?}, not {theirs:?}", word.escape_unicode())));
                }
                tried += these.len();
            }
            println!("{}#{face}: {} words, {} differ", path.display(), held.len(), differ.len());
            differ.iter().take(8).for_each(|line| println!("  {line}"));
            all_differ.extend(differ.into_iter().map(|line| format!("{}: {line}", path.display())));
        }
        println!("{tried} words and fonts held, {} differ", all_differ.len());
        assert!(all_differ.is_empty(), "{} words differ", all_differ.len());
    }
}
