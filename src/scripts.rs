//! Which script each character is in, and OpenType's script and language tags.
//!
//! OpenType fonts keep their substitutions and positions (`GSUB`, `GPOS`) by script (`latn`,
//! `cyrl`, `arab`, `hani`) and, under each, by language where it writes differently (`SRB `
//! Serbian, `TRK ` Turkish, `JAN ` Japanese). Text is shaped by its letters' script as Unicode
//! gives it (`Scripts.txt`), with HarfBuzz's tags; characters common to all scripts (digits,
//! spaces, combining marks) take the script around them. It is shaped in a language where one
//! is named, for a piece of text, a page, a window or the system ([`Language`]). Languages are
//! named in BCP 47 ("sr", "zh-Hant-HK") and mapped to OpenType tags as HarfBuzz does: its table
//! of two-letter languages, Chinese by script and region, other three-letter languages by their
//! own letters.
//!
//! Not done: HarfBuzz's table of three-letter languages whose OpenType tags differ from their
//! letters (Arabic dialects as `ARA `: here tagged by their own letters), and the variants it
//! reads from later subtags except for Chinese, polytonic Greek, Syriac's three scripts and the
//! phonetic alphabets.

/// How many of `TAGS` `brahmic.rs` shapes as syllables: they come first, in its order.
pub(crate) const BRAHMIC: usize = 101;

/// A character's script: its index in `TAGS` from one; zero for none of its own.
pub(crate) fn of(letter: char) -> u8 {
    let code = letter as u32;
    let n = SCRIPTS.partition_point(|entry| entry >> 8 <= code);
    n.checked_sub(1).map_or(0, |n| (SCRIPTS[n] & 0xff) as u8)
}

/// A script's OpenType tag, by its index in `TAGS` (from one).
pub(crate) fn tag(n: u8) -> &'static [u8; 4] {
    &TAGS[(n as usize).clamp(1, TAGS.len()) - 1]
}

/// The index in `TAGS` (from one) of the script tagged `name`.
pub(crate) const fn numbered(name: &[u8; 4]) -> u8 {
    let mut n = 0;
    while n < TAGS.len() && u32::from_be_bytes(TAGS[n]) != u32::from_be_bytes(*name) {
        n += 1;
    }
    n as u8 + 1
}

/// The scripts shaped in ways of their own.
pub(crate) const ARABIC: u8 = numbered(b"arab");
pub(crate) const SYRIAC: u8 = numbered(b"syrc");
pub(crate) const HEBREW: u8 = numbered(b"hebr");

/// Whether a character is a decimal digit of any script: what a fraction is written with.
pub(crate) fn digit(letter: char) -> bool {
    let code = letter as u32;
    let n = DIGITS.partition_point(|entry| entry >> 8 <= code);
    n.checked_sub(1).is_some_and(|n| code - (DIGITS[n] >> 8) < DIGITS[n] & 0xff)
}

/// The language of a piece of text, so a font gives its letters the forms of that language.
/// Serbian and Macedonian write some Cyrillic letters differently from Russian; Romanian puts a
/// comma, not a cedilla, under s and t; Turkish keeps the dot of an i after an f; Dutch writes
/// ij as one letter; and CJK fonts have a form of many characters per language. Named in BCP
/// 47: [`Language::named("sr")`](Language::named),
/// [`Language::named("zh-Hant-HK")`](Language::named).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Language {
    /// The language's OpenType tags in the order they are looked for (some have more than one);
    /// zero after the last.
    systems: [[u8; 4]; 3],
}

impl Language {
    /// No language: a font's default forms.
    pub const NONE: Language = Language { systems: [[0; 4]; 3] };

    /// A language by its BCP 47 name: "sr", "ro", "tr", "nl", "ja", "zh-Hant-HK". One OpenType
    /// has no tag for is `NONE`.
    pub fn named(name: &str) -> Language {
        let name = name.trim().to_ascii_lowercase().replace('_', "-");
        let mut language = Language::NONE;
        for (n, system) in systems_of(&name).into_iter().take(3).enumerate() {
            language.systems[n] = system;
        }
        language
    }

    /// The system's language: on Windows the user's setting, elsewhere `LC_ALL`, `LC_CTYPE` or
    /// `LANG`. Read once.
    pub fn of_the_system() -> Language {
        static READ: std::sync::OnceLock<Language> = std::sync::OnceLock::new();
        *READ.get_or_init(|| Language::named(&system_language()))
    }

    /// The language's OpenType tags, in the order they are looked for.
    pub(crate) fn systems(&self) -> impl Iterator<Item = &[u8; 4]> {
        self.systems.iter().take_while(|system| **system != [0; 4])
    }
}

/// Chinese languages, which a following script or region makes more precise, and their OpenType
/// tags where nothing does.
const CHINESE: [(&str, &[u8; 4]); 16] = [
    ("zh", b"ZHS "),
    ("cdo", b"ZHS "),
    ("cjy", b"ZHS "),
    ("cmn", b"ZHS "),
    ("cpx", b"ZHS "),
    ("csp", b"ZHS "),
    ("czh", b"ZHS "),
    ("czo", b"ZHS "),
    ("gan", b"ZHS "),
    ("hak", b"ZHS "),
    ("hsn", b"ZHS "),
    ("lzh", b"ZHT "),
    ("mnp", b"ZHS "),
    ("nan", b"ZHS "),
    ("wuu", b"ZHS "),
    ("yue", b"ZHH "),
];

/// The OpenType tags of a language named in BCP 47 (lower case), as HarfBuzz works them out: a
/// few variants by a later subtag; Chinese by script and region; else the language or its
/// extended language (`zh-yue`) by HarfBuzz's two-letter table, or three letters as themselves.
fn systems_of(name: &str) -> Vec<[u8; 4]> {
    let parts: Vec<&str> = name.split('-').collect();
    let later = |part: &str| parts[1..].contains(&part);
    let variant = [("polyton", b"PGR "), ("syre", b"SYRE"), ("syrj", b"SYRJ"), ("syrn", b"SYRN"), ("fonipa", b"IPPH")];
    if let Some((_, system)) = variant.iter().find(|(part, _)| name.len() >= 7 && later(part)) {
        return vec![**system];
    }
    let chinese = CHINESE.iter().find(|(language, _)| *language == parts[0]);
    let (script, region) = (parts.get(1).copied(), parts.get(2).copied());
    let taiwan = match (chinese.is_some(), script, region) {
        (false, ..) => None,
        (_, Some("hant"), Some("hk")) => Some(vec![*b"ZHH "]),
        (_, Some("hant"), Some("mo")) => Some(vec![*b"ZHTM", *b"ZHH "]),
        (_, Some("hans"), _) => Some(vec![*b"ZHS "]),
        (_, Some("hant"), _) => Some(vec![*b"ZHT "]),
        _ if later("hk") => Some(vec![*b"ZHH "]),
        _ if later("mo") => Some(vec![*b"ZHTM", *b"ZHH "]),
        _ if later("tw") => Some(vec![*b"ZHT "]),
        _ => None,
    };
    if let Some(systems) = taiwan {
        return systems;
    }
    let extended = parts.get(1).filter(|part| part.len() == 3 && part.starts_with(|letter: char| letter.is_ascii_alphabetic()) && name.len() >= 6);
    let language = extended.copied().unwrap_or(parts[0]);
    if let Some((_, system)) = CHINESE.iter().find(|(chinese, _)| *chinese == language) {
        return vec![**system];
    }
    match language.len() {
        2 => LANGUAGES.as_chunks::<6>().0.iter().filter(|entry| entry[..2] == *language.as_bytes()).map(|entry| [entry[2], entry[3], entry[4], entry[5]]).collect(),
        3 if language.bytes().all(|letter| letter.is_ascii_alphabetic()) => {
            let [a, b, c] = [0, 1, 2].map(|n| language.as_bytes()[n].to_ascii_uppercase());
            vec![[a, b, c, b' ']]
        }
        _ => Vec::new(),
    }
}

/// The system language's name as Windows has it ("sr-Cyrl-RS").
#[cfg(windows)]
pub(crate) fn system_language() -> String {
    let mut name = [0u16; 85];
    // SAFETY: the system writes at most the given number of characters into room that has them,
    // and returns how many with the terminating zero.
    let wrote = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..(wrote.max(1) - 1) as usize])
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetUserDefaultLocaleName(name: *mut u16, room: i32) -> i32;
}

/// The language the environment names ("sr_RS.UTF-8" is "sr_RS"), as the C library looks it up
/// for `LC_CTYPE`; nothing for "C".
#[cfg(not(windows))]
pub(crate) fn system_language() -> String {
    let named = ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|variable| std::env::var(variable).ok().filter(|name| !name.is_empty())).unwrap_or_default();
    let name = named.split(['.', '@']).next().unwrap_or_default();
    if matches!(name, "C" | "POSIX") { String::new() } else { name.to_string() }
}

/// Every character's script, as ranges: each range's first character (times 256) plus its
/// script index in `TAGS` from one (zero: Unicode's Common and Inherited). Unicode 17.
#[rustfmt::skip]
const SCRIPTS: [u32; 1469] = [
    0x4186, 0x5b00, 0x6186, 0x7b00, 0xaa86, 0xab00, 0xba86, 0xbb00, 0xc086, 0xd700, 0xd886, 0xf700,
    0xf886, 0x2b900, 0x2e086, 0x2e500, 0x2ea6c, 0x2ec00, 0x3707c, 0x37400, 0x3757c, 0x37800, 0x37a7c, 0x37e00,
    0x37f7c, 0x38000, 0x3847c, 0x38500, 0x3867c, 0x38700, 0x3887c, 0x38b00, 0x38c7c, 0x38d00, 0x38e7c, 0x3a200,
    0x3a37c, 0x3e272, 0x3f07c, 0x40075, 0x48500, 0x48775, 0x53000, 0x53168, 0x55700, 0x55968, 0x58b00, 0x58d68,
    0x59000, 0x59180, 0x5c800, 0x5d080, 0x5eb00, 0x5ef80, 0x5f500, 0x60067, 0x60500, 0x60667, 0x60c00, 0x60d67,
    0x61b00, 0x61c67, 0x61f00, 0x62067, 0x64000, 0x64167, 0x64b00, 0x65667, 0x67000, 0x67167, 0x6dd00, 0x6de67,
    0x700a6, 0x70e00, 0x70fa6, 0x74b00, 0x74da6, 0x75067, 0x780a8, 0x7b200, 0x7c043, 0x7fb00, 0x7fd43, 0x800a2,
    0x82e00, 0x830a2, 0x83f00, 0x84046, 0x85c00, 0x85e46, 0x85f00, 0x860a6, 0x86b00, 0x87067, 0x89200, 0x89767,
    0x8e200, 0x8e367, 0x90001, 0x95100, 0x95501, 0x96400, 0x96601, 0x98002, 0x98400, 0x98502, 0x98d00, 0x98f02,
    0x99100, 0x99302, 0x9a900, 0x9aa02, 0x9b100, 0x9b202, 0x9b300, 0x9b602, 0x9ba00, 0x9bc02, 0x9c500, 0x9c702,
    0x9c900, 0x9cb02, 0x9cf00, 0x9d702, 0x9d800, 0x9dc02, 0x9de00, 0x9df02, 0x9e400, 0x9e602, 0x9ff00, 0xa0103,
    0xa0400, 0xa0503, 0xa0b00, 0xa0f03, 0xa1100, 0xa1303, 0xa2900, 0xa2a03, 0xa3100, 0xa3203, 0xa3400, 0xa3503,
    0xa3700, 0xa3803, 0xa3a00, 0xa3c03, 0xa3d00, 0xa3e03, 0xa4300, 0xa4703, 0xa4900, 0xa4b03, 0xa4e00, 0xa5103,
    0xa5200, 0xa5903, 0xa5d00, 0xa5e03, 0xa5f00, 0xa6603, 0xa7700, 0xa8104, 0xa8400, 0xa8504, 0xa8e00, 0xa8f04,
    0xa9200, 0xa9304, 0xaa900, 0xaaa04, 0xab100, 0xab204, 0xab400, 0xab504, 0xaba00, 0xabc04, 0xac600, 0xac704,
    0xaca00, 0xacb04, 0xace00, 0xad004, 0xad100, 0xae004, 0xae400, 0xae604, 0xaf200, 0xaf904, 0xb0000, 0xb0105,
    0xb0400, 0xb0505, 0xb0d00, 0xb0f05, 0xb1100, 0xb1305, 0xb2900, 0xb2a05, 0xb3100, 0xb3205, 0xb3400, 0xb3505,
    0xb3a00, 0xb3c05, 0xb4500, 0xb4705, 0xb4900, 0xb4b05, 0xb4e00, 0xb5505, 0xb5800, 0xb5c05, 0xb5e00, 0xb5f05,
    0xb6400, 0xb6605, 0xb7800, 0xb8206, 0xb8400, 0xb8506, 0xb8b00, 0xb8e06, 0xb9100, 0xb9206, 0xb9600, 0xb9906,
    0xb9b00, 0xb9c06, 0xb9d00, 0xb9e06, 0xba000, 0xba306, 0xba500, 0xba806, 0xbab00, 0xbae06, 0xbba00, 0xbbe06,
    0xbc300, 0xbc606, 0xbc900, 0xbca06, 0xbce00, 0xbd006, 0xbd100, 0xbd706, 0xbd800, 0xbe606, 0xbfb00, 0xc0007,
    0xc0d00, 0xc0e07, 0xc1100, 0xc1207, 0xc2900, 0xc2a07, 0xc3a00, 0xc3c07, 0xc4500, 0xc4607, 0xc4900, 0xc4a07,
    0xc4e00, 0xc5507, 0xc5700, 0xc5807, 0xc5b00, 0xc5c07, 0xc5e00, 0xc6007, 0xc6400, 0xc6607, 0xc7000, 0xc7707,
    0xc8008, 0xc8d00, 0xc8e08, 0xc9100, 0xc9208, 0xca900, 0xcaa08, 0xcb400, 0xcb508, 0xcba00, 0xcbc08, 0xcc500,
    0xcc608, 0xcc900, 0xcca08, 0xcce00, 0xcd508, 0xcd700, 0xcdc08, 0xcdf00, 0xce008, 0xce400, 0xce608, 0xcf000,
    0xcf108, 0xcf400, 0xd0009, 0xd0d00, 0xd0e09, 0xd1100, 0xd1209, 0xd4500, 0xd4609, 0xd4900, 0xd4a09, 0xd5000,
    0xd5409, 0xd6400, 0xd6609, 0xd8000, 0xd810a, 0xd8400, 0xd850a, 0xd9700, 0xd9a0a, 0xdb200, 0xdb30a, 0xdbc00,
    0xdbd0a, 0xdbe00, 0xdc00a, 0xdc700, 0xdca0a, 0xdcb00, 0xdcf0a, 0xdd500, 0xdd60a, 0xdd700, 0xdd80a, 0xde000,
    0xde60a, 0xdf000, 0xdf20a, 0xdf500, 0xe010b, 0xe3b00, 0xe400b, 0xe5c00, 0xe810c, 0xe8300, 0xe840c, 0xe8500,
    0xe860c, 0xe8b00, 0xe8c0c, 0xea400, 0xea50c, 0xea600, 0xea70c, 0xebe00, 0xec00c, 0xec500, 0xec60c, 0xec700,
    0xec80c, 0xecf00, 0xed00c, 0xeda00, 0xedc0c, 0xee000, 0xf000d, 0xf4800, 0xf490d, 0xf6d00, 0xf710d, 0xf9800,
    0xf990d, 0xfbd00, 0xfbe0d, 0xfcd00, 0xfce0d, 0xfd500, 0xfd90d, 0xfdb00, 0x10000e, 0x10a079, 0x10c600, 0x10c779,
    0x10c800, 0x10cd79, 0x10ce00, 0x10d079, 0x10fb00, 0x10fc79, 0x11007e, 0x120078, 0x124900, 0x124a78, 0x124e00, 0x125078,
    0x125700, 0x125878, 0x125900, 0x125a78, 0x125e00, 0x126078, 0x128900, 0x128a78, 0x128e00, 0x129078, 0x12b100, 0x12b278,
    0x12b600, 0x12b878, 0x12bf00, 0x12c078, 0x12c100, 0x12c278, 0x12c600, 0x12c878, 0x12d700, 0x12d878, 0x131100, 0x131278,
    0x131600, 0x131878, 0x135b00, 0x135d78, 0x137d00, 0x138078, 0x139a00, 0x13a071, 0x13f600, 0x13f871, 0x13fe00, 0x14006e,
    0x168093, 0x169d00, 0x16a0a1, 0x16eb00, 0x16eea1, 0x16f900, 0x17001a, 0x171600, 0x171f1a, 0x17201b, 0x173500, 0x17401c,
    0x175400, 0x17601d, 0x176d00, 0x176e1d, 0x177100, 0x17721d, 0x177400, 0x17800f, 0x17de00, 0x17e00f, 0x17ea00, 0x17f00f,
    0x17fa00, 0x180040, 0x180200, 0x180440, 0x180500, 0x180640, 0x181a00, 0x182040, 0x187900, 0x188040, 0x18ab00, 0x18b06e,
    0x18f600, 0x190018, 0x191f00, 0x192018, 0x192c00, 0x193018, 0x193c00, 0x194018, 0x194100, 0x194418, 0x195019, 0x196e00,
    0x197019, 0x197500, 0x198091, 0x19ac00, 0x19b091, 0x19ca00, 0x19d091, 0x19db00, 0x19de91, 0x19e00f, 0x1a0013, 0x1a1c00,
    0x1a1e13, 0x1a2014, 0x1a5f00, 0x1a6014, 0x1a7d00, 0x1a7f14, 0x1a8a00, 0x1a9014, 0x1a9a00, 0x1aa014, 0x1aae00, 0x1b0010,
    0x1b4d00, 0x1b4e10, 0x1b8012, 0x1bc016, 0x1bf400, 0x1bfc16, 0x1c0017, 0x1c3800, 0x1c3b17, 0x1c4a00, 0x1c4d17, 0x1c5094,
    0x1c8075, 0x1c8b00, 0x1c9079, 0x1cbb00, 0x1cbd79, 0x1cc012, 0x1cc800, 0x1d0086, 0x1d267c, 0x1d2b75, 0x1d2c86, 0x1d5d7c,
    0x1d6286, 0x1d667c, 0x1d6b86, 0x1d7875, 0x1d7986, 0x1dbf7c, 0x1dc000, 0x1e0086, 0x1f007c, 0x1f1600, 0x1f187c, 0x1f1e00,
    0x1f207c, 0x1f4600, 0x1f487c, 0x1f4e00, 0x1f507c, 0x1f5800, 0x1f597c, 0x1f5a00, 0x1f5b7c, 0x1f5c00, 0x1f5d7c, 0x1f5e00,
    0x1f5f7c, 0x1f7e00, 0x1f807c, 0x1fb500, 0x1fb67c, 0x1fc500, 0x1fc67c, 0x1fd400, 0x1fd67c, 0x1fdc00, 0x1fdd7c, 0x1ff000,
    0x1ff27c, 0x1ff500, 0x1ff67c, 0x1fff00, 0x207186, 0x207200, 0x207f86, 0x208000, 0x209086, 0x209d00, 0x21267c, 0x212700,
    0x212a86, 0x212c00, 0x213286, 0x213300, 0x214e86, 0x214f00, 0x216086, 0x218900, 0x28006d, 0x290000, 0x2c007a, 0x2c6086,
    0x2c8072, 0x2cf400, 0x2cf972, 0x2d0079, 0x2d2600, 0x2d2779, 0x2d2800, 0x2d2d79, 0x2d2e00, 0x2d3042, 0x2d6800, 0x2d6f42,
    0x2d7100, 0x2d7f42, 0x2d8078, 0x2d9700, 0x2da078, 0x2da700, 0x2da878, 0x2daf00, 0x2db078, 0x2db700, 0x2db878, 0x2dbf00,
    0x2dc078, 0x2dc700, 0x2dc878, 0x2dcf00, 0x2dd078, 0x2dd700, 0x2dd878, 0x2ddf00, 0x2de075, 0x2e0000, 0x2e807d, 0x2e9a00,
    0x2e9b7d, 0x2ef400, 0x2f007d, 0x2fd600, 0x30057d, 0x300600, 0x30077d, 0x300800, 0x30217d, 0x302a00, 0x302e7e, 0x303000,
    0x30387d, 0x303c00, 0x304181, 0x309700, 0x309d81, 0x30a000, 0x30a185, 0x30fb00, 0x30fd85, 0x310000, 0x31056c, 0x313000,
    0x31317e, 0x318f00, 0x31a06c, 0x31c000, 0x31f085, 0x32007e, 0x321f00, 0x32607e, 0x327f00, 0x32d085, 0x32ff00, 0x330085,
    0x335800, 0x34007d, 0x4dc000, 0x4e007d, 0xa000ac, 0xa48d00, 0xa490ac, 0xa4c700, 0xa4d089, 0xa500aa, 0xa62c00, 0xa64075,
    0xa6a06a, 0xa6f800, 0xa72286, 0xa78800, 0xa78b86, 0xa7dd00, 0xa7f186, 0xa8001e, 0xa82d00, 0xa84044, 0xa87800, 0xa8801f,
    0xa8c600, 0xa8ce1f, 0xa8da00, 0xa8e001, 0xa90020, 0xa92e00, 0xa92f20, 0xa93021, 0xa95400, 0xa95f21, 0xa9607e, 0xa97d00,
    0xa98011, 0xa9ce00, 0xa9d011, 0xa9da00, 0xa9de11, 0xa9e00e, 0xa9ff00, 0xaa0015, 0xaa3700, 0xaa4015, 0xaa4e00, 0xaa5015,
    0xaa5a00, 0xaa5c15, 0xaa600e, 0xaa8023, 0xaac300, 0xaadb23, 0xaae022, 0xaaf700, 0xab0178, 0xab0700, 0xab0978, 0xab0f00,
    0xab1178, 0xab1700, 0xab2078, 0xab2700, 0xab2878, 0xab2f00, 0xab3086, 0xab5b00, 0xab5c86, 0xab657c, 0xab6686, 0xab6a00,
    0xab7071, 0xabc022, 0xabee00, 0xabf022, 0xabfa00, 0xac007e, 0xd7a400, 0xd7b07e, 0xd7c700, 0xd7cb7e, 0xd7fc00, 0xf9007d,
    0xfa6e00, 0xfa707d, 0xfada00, 0xfb0086, 0xfb0700, 0xfb1368, 0xfb1800, 0xfb1d80, 0xfb3700, 0xfb3880, 0xfb3d00, 0xfb3e80,
    0xfb3f00, 0xfb4080, 0xfb4200, 0xfb4380, 0xfb4500, 0xfb4680, 0xfb5067, 0xfd3e00, 0xfd4067, 0xfdd000, 0xfdf067, 0xfe0000,
    0xfe2e75, 0xfe3000, 0xfe7067, 0xfe7500, 0xfe7667, 0xfefd00, 0xff2186, 0xff3b00, 0xff4186, 0xff5b00, 0xff6685, 0xff7000,
    0xff7185, 0xff9e00, 0xffa07e, 0xffbf00, 0xffc27e, 0xffc800, 0xffca7e, 0xffd000, 0xffd27e, 0xffd800, 0xffda7e, 0xffdd00,
    0x1000088, 0x1000c00, 0x1000d88, 0x1002700, 0x1002888, 0x1003b00, 0x1003c88, 0x1003e00, 0x1003f88, 0x1004e00, 0x1005088, 0x1005e00,
    0x1008088, 0x100fb00, 0x101407c, 0x1018f00, 0x101a07c, 0x101a100, 0x102808a, 0x1029d00, 0x102a06f, 0x102d100, 0x1030096, 0x1032400,
    0x1032d96, 0x103307b, 0x1034b00, 0x1035098, 0x1037b00, 0x10380a9, 0x1039e00, 0x1039fa9, 0x103a099, 0x103c400, 0x103c899, 0x103d600,
    0x1040076, 0x10450a3, 0x104809d, 0x1049e00, 0x104a09d, 0x104aa00, 0x104b09c, 0x104d400, 0x104d89c, 0x104fc00, 0x1050077, 0x1052800,
    0x1053070, 0x1056400, 0x1056f70, 0x105705b, 0x1057b00, 0x1057c5b, 0x1058b00, 0x1058c5b, 0x1059300, 0x105945b, 0x1059600, 0x105975b,
    0x105a200, 0x105a35b, 0x105b200, 0x105b35b, 0x105ba00, 0x105bb5b, 0x105bd00, 0x105c062, 0x105f400, 0x1060087, 0x1073700, 0x1074087,
    0x1075600, 0x1076087, 0x1076800, 0x1078086, 0x1078600, 0x1078786, 0x107b100, 0x107b286, 0x107bb00, 0x1080074, 0x1080600, 0x1080874,
    0x1080900, 0x1080a74, 0x1083600, 0x1083774, 0x1083900, 0x1083c74, 0x1083d00, 0x1083f74, 0x1084082, 0x1085600, 0x1085782, 0x108609e,
    0x1088090, 0x1089f00, 0x108a790, 0x108b000, 0x108e07f, 0x108f300, 0x108f47f, 0x108f600, 0x108fb7f, 0x10900a0, 0x1091c00, 0x1091fa0,
    0x109208b, 0x1093a00, 0x1093f8b, 0x1094064, 0x1095a00, 0x109808e, 0x109a08d, 0x109b800, 0x109bc8d, 0x109d000, 0x109d28d, 0x10a0041,
    0x10a0400, 0x10a0541, 0x10a0700, 0x10a0c41, 0x10a1400, 0x10a1541, 0x10a1800, 0x10a1941, 0x10a3600, 0x10a3841, 0x10a3b00, 0x10a3f41,
    0x10a4900, 0x10a5041, 0x10a5900, 0x10a609a, 0x10a8097, 0x10aa000, 0x10ac049, 0x10ae700, 0x10aeb49, 0x10af700, 0x10b0069, 0x10b3600,
    0x10b3969, 0x10b4084, 0x10b5600, 0x10b5884, 0x10b6083, 0x10b7300, 0x10b7883, 0x10b804b, 0x10b9200, 0x10b994b, 0x10b9d00, 0x10ba94b,
    0x10bb000, 0x10c009b, 0x10c4900, 0x10c8095, 0x10cb300, 0x10cc095, 0x10cf300, 0x10cfa95, 0x10d004d, 0x10d2800, 0x10d304d, 0x10d3a00,
    0x10d405d, 0x10d6600, 0x10d695d, 0x10d8600, 0x10d8e5d, 0x10d9000, 0x10e6067, 0x10e7f00, 0x10e8056, 0x10eaa00, 0x10eab56, 0x10eae00,
    0x10eb056, 0x10eb200, 0x10ec267, 0x10ec800, 0x10ed067, 0x10ed900, 0x10efa67, 0x10f004f, 0x10f2800, 0x10f3050, 0x10f5a00, 0x10f7058,
    0x10f8a00, 0x10fb054, 0x10fcc00, 0x10fe051, 0x10ff700, 0x1100024, 0x1104e00, 0x1105224, 0x1107600, 0x1107f24, 0x1108025, 0x110c300,
    0x110cd25, 0x110ce00, 0x110d0a5, 0x110e900, 0x110f0a5, 0x110fa00, 0x1110026, 0x1113500, 0x1113626, 0x1114800, 0x111502c, 0x1117700,
    0x1118027, 0x111e000, 0x111e10a, 0x111f500, 0x112002a, 0x1121200, 0x112132a, 0x1124200, 0x1128031, 0x1128700, 0x1128831, 0x1128900,
    0x1128a31, 0x1128e00, 0x1128f31, 0x1129e00, 0x1129f31, 0x112aa00, 0x112b02b, 0x112eb00, 0x112f02b, 0x112fa00, 0x1130029, 0x1130400,
    0x1130529, 0x1130d00, 0x1130f29, 0x1131100, 0x1131329, 0x1132900, 0x1132a29, 0x1133100, 0x1133229, 0x1133400, 0x1133529, 0x1133a00,
    0x1133c29, 0x1134500, 0x1134729, 0x1134900, 0x1134b29, 0x1134e00, 0x1135029, 0x1135100, 0x1135729, 0x1135800, 0x1135d29, 0x1136400,
    0x1136629, 0x1136d00, 0x1137029, 0x1137500, 0x113803e, 0x1138a00, 0x1138b3e, 0x1138c00, 0x1138e3e, 0x1138f00, 0x113903e, 0x113b600,
    0x113b73e, 0x113c100, 0x113c23e, 0x113c300, 0x113c53e, 0x113c600, 0x113c73e, 0x113cb00, 0x113cc3e, 0x113d600, 0x113d73e, 0x113d900,
    0x113e13e, 0x113e300, 0x1140034, 0x1145c00, 0x1145d34, 0x1146200, 0x114802f, 0x114c800, 0x114d02f, 0x114da00, 0x115802e, 0x115b600,
    0x115b82e, 0x115de00, 0x116002d, 0x1164500, 0x116502d, 0x1165a00, 0x1166040, 0x1166d00, 0x1168028, 0x116ba00, 0x116c028, 0x116ca00,
    0x116d00e, 0x116e400, 0x1170030, 0x1171b00, 0x1171d30, 0x1172c00, 0x1173030, 0x1174700, 0x1180038, 0x1183c00, 0x118a0ab, 0x118f300,
    0x118ffab, 0x119003c, 0x1190700, 0x119093c, 0x1190a00, 0x1190c3c, 0x1191400, 0x119153c, 0x1191700, 0x119183c, 0x1193600, 0x119373c,
    0x1193900, 0x1193b3c, 0x1194700, 0x119503c, 0x1195a00, 0x119a03b, 0x119a800, 0x119aa3b, 0x119d800, 0x119da3b, 0x119e500, 0x11a0037,
    0x11a4800, 0x11a5036, 0x11aa300, 0x11ab06e, 0x11ac09f, 0x11af900, 0x11b0001, 0x11b0a00, 0x11b6027, 0x11b6800, 0x11bc061, 0x11be200,
    0x11bf061, 0x11bfa00, 0x11c0032, 0x11c0900, 0x11c0a32, 0x11c3700, 0x11c3832, 0x11c4600, 0x11c5032, 0x11c6d00, 0x11c7033, 0x11c9000,
    0x11c9233, 0x11ca800, 0x11ca933, 0x11cb700, 0x11d0035, 0x11d0700, 0x11d0835, 0x11d0a00, 0x11d0b35, 0x11d3700, 0x11d3a35, 0x11d3b00,
    0x11d3c35, 0x11d3e00, 0x11d3f35, 0x11d4800, 0x11d5035, 0x11d5a00, 0x11d6039, 0x11d6600, 0x11d6739, 0x11d6900, 0x11d6a39, 0x11d8f00,
    0x11d9039, 0x11d9200, 0x11d9339, 0x11d9900, 0x11da039, 0x11daa00, 0x11db065, 0x11ddc00, 0x11de065, 0x11dea00, 0x11ee03a, 0x11ef900,
    0x11f003d, 0x11f1100, 0x11f123d, 0x11f3b00, 0x11f3e3d, 0x11f5b00, 0x11fb089, 0x11fb100, 0x11fc006, 0x11ff200, 0x11fff06, 0x1200073,
    0x1239a00, 0x1240073, 0x1246f00, 0x1247073, 0x1247500, 0x1248073, 0x1254400, 0x12f9057, 0x12ff300, 0x1300045, 0x1345600, 0x1346045,
    0x143fb00, 0x1440066, 0x1464700, 0x161005e, 0x1613a00, 0x168006a, 0x16a3900, 0x16a408f, 0x16a5f00, 0x16a608f, 0x16a6a00, 0x16a6e8f,
    0x16a7059, 0x16abf00, 0x16ac059, 0x16aca00, 0x16ad06b, 0x16aee00, 0x16af06b, 0x16af600, 0x16b004a, 0x16b4600, 0x16b504a, 0x16b5a00,
    0x16b5b4a, 0x16b6200, 0x16b634a, 0x16b7800, 0x16b7d4a, 0x16b9000, 0x16d405f, 0x16d7a00, 0x16e404e, 0x16e9b00, 0x16ea063, 0x16eb900,
    0x16ebb63, 0x16ed400, 0x16f0047, 0x16f4b00, 0x16f4f47, 0x16f8800, 0x16f8f47, 0x16fa000, 0x16fe0a7, 0x16fe192, 0x16fe27d, 0x16fe455,
    0x16fe500, 0x16ff07d, 0x16ff700, 0x17000a7, 0x18b0055, 0x18cd600, 0x18cff55, 0x18d00a7, 0x18d1f00, 0x18d80a7, 0x18df300, 0x1aff085,
    0x1aff400, 0x1aff585, 0x1affc00, 0x1affd85, 0x1afff00, 0x1b00085, 0x1b00181, 0x1b12085, 0x1b12300, 0x1b13281, 0x1b13300, 0x1b15081,
    0x1b15300, 0x1b15585, 0x1b15600, 0x1b16485, 0x1b16800, 0x1b17092, 0x1b2fc00, 0x1bc0048, 0x1bc6b00, 0x1bc7048, 0x1bc7d00, 0x1bc8048,
    0x1bc8900, 0x1bc9048, 0x1bc9a00, 0x1bc9c48, 0x1bca000, 0x1d2007c, 0x1d24600, 0x1d800a4, 0x1da8c00, 0x1da9ba4, 0x1daa000, 0x1daa1a4,
    0x1dab000, 0x1df0086, 0x1df1f00, 0x1df2586, 0x1df2b00, 0x1e0007a, 0x1e00700, 0x1e0087a, 0x1e01900, 0x1e01b7a, 0x1e02200, 0x1e0237a,
    0x1e02500, 0x1e0267a, 0x1e02b00, 0x1e03075, 0x1e06e00, 0x1e08f75, 0x1e09000, 0x1e10052, 0x1e12d00, 0x1e13052, 0x1e13e00, 0x1e14052,
    0x1e14a00, 0x1e14e52, 0x1e15000, 0x1e2905a, 0x1e2af00, 0x1e2c053, 0x1e2fa00, 0x1e2ff53, 0x1e30000, 0x1e4d05c, 0x1e4fa00, 0x1e5d060,
    0x1e5fb00, 0x1e5ff60, 0x1e60000, 0x1e6c03f, 0x1e6df00, 0x1e6e03f, 0x1e6f600, 0x1e6fe3f, 0x1e70000, 0x1e7e078, 0x1e7e700, 0x1e7e878,
    0x1e7ec00, 0x1e7ed78, 0x1e7ef00, 0x1e7f078, 0x1e7ff00, 0x1e8008c, 0x1e8c500, 0x1e8c78c, 0x1e8d700, 0x1e9004c, 0x1e94c00, 0x1e9504c,
    0x1e95a00, 0x1e95e4c, 0x1e96000, 0x1ee0067, 0x1ee0400, 0x1ee0567, 0x1ee2000, 0x1ee2167, 0x1ee2300, 0x1ee2467, 0x1ee2500, 0x1ee2767,
    0x1ee2800, 0x1ee2967, 0x1ee3300, 0x1ee3467, 0x1ee3800, 0x1ee3967, 0x1ee3a00, 0x1ee3b67, 0x1ee3c00, 0x1ee4267, 0x1ee4300, 0x1ee4767,
    0x1ee4800, 0x1ee4967, 0x1ee4a00, 0x1ee4b67, 0x1ee4c00, 0x1ee4d67, 0x1ee5000, 0x1ee5167, 0x1ee5300, 0x1ee5467, 0x1ee5500, 0x1ee5767,
    0x1ee5800, 0x1ee5967, 0x1ee5a00, 0x1ee5b67, 0x1ee5c00, 0x1ee5d67, 0x1ee5e00, 0x1ee5f67, 0x1ee6000, 0x1ee6167, 0x1ee6300, 0x1ee6467,
    0x1ee6500, 0x1ee6767, 0x1ee6b00, 0x1ee6c67, 0x1ee7300, 0x1ee7467, 0x1ee7800, 0x1ee7967, 0x1ee7d00, 0x1ee7e67, 0x1ee7f00, 0x1ee8067,
    0x1ee8a00, 0x1ee8b67, 0x1ee9c00, 0x1eea167, 0x1eea400, 0x1eea567, 0x1eeaa00, 0x1eeab67, 0x1eebc00, 0x1eef067, 0x1eef200, 0x1f20081,
    0x1f20100, 0x200007d, 0x2a6e000, 0x2a7007d, 0x2b81e00, 0x2b8207d, 0x2ceae00, 0x2ceb07d, 0x2ebe100, 0x2ebf07d, 0x2ee5e00, 0x2f8007d,
    0x2fa1e00, 0x300007d, 0x3134b00, 0x313507d, 0x3347a00,
];
/// Each script's OpenType tag as HarfBuzz has it: those `brahmic.rs` shapes first, in its
/// order.
#[rustfmt::skip]
const TAGS: [[u8; 4]; 172] = [
    *b"deva" /* Devanagari */, *b"beng" /* Bengali */, *b"guru" /* Gurmukhi */, *b"gujr" /* Gujarati */,
    *b"orya" /* Oriya */, *b"taml" /* Tamil */, *b"telu" /* Telugu */, *b"knda" /* Kannada */,
    *b"mlym" /* Malayalam */, *b"sinh" /* Sinhala */, *b"thai" /* Thai */, *b"lao " /* Lao */,
    *b"tibt" /* Tibetan */, *b"mymr" /* Myanmar */, *b"khmr" /* Khmer */, *b"bali" /* Balinese */,
    *b"java" /* Javanese */, *b"sund" /* Sundanese */, *b"bugi" /* Buginese */, *b"lana" /* Tai_Tham */,
    *b"cham" /* Cham */, *b"batk" /* Batak */, *b"lepc" /* Lepcha */, *b"limb" /* Limbu */,
    *b"tale" /* Tai_Le */, *b"tglg" /* Tagalog */, *b"hano" /* Hanunoo */, *b"buhd" /* Buhid */,
    *b"tagb" /* Tagbanwa */, *b"sylo" /* Syloti_Nagri */, *b"saur" /* Saurashtra */, *b"kali" /* Kayah_Li */,
    *b"rjng" /* Rejang */, *b"mtei" /* Meetei_Mayek */, *b"tavt" /* Tai_Viet */, *b"brah" /* Brahmi */,
    *b"kthi" /* Kaithi */, *b"cakm" /* Chakma */, *b"shrd" /* Sharada */, *b"takr" /* Takri */,
    *b"gran" /* Grantha */, *b"khoj" /* Khojki */, *b"sind" /* Khudawadi */, *b"mahj" /* Mahajani */,
    *b"modi" /* Modi */, *b"sidd" /* Siddham */, *b"tirh" /* Tirhuta */, *b"ahom" /* Ahom */,
    *b"mult" /* Multani */, *b"bhks" /* Bhaiksuki */, *b"marc" /* Marchen */, *b"newa" /* Newa */,
    *b"gonm" /* Masaram_Gondi */, *b"soyo" /* Soyombo */, *b"zanb" /* Zanabazar_Square */, *b"dogr" /* Dogra */,
    *b"gong" /* Gunjala_Gondi */, *b"maka" /* Makasar */, *b"nand" /* Nandinagari */, *b"diak" /* Dives_Akuru */,
    *b"kawi" /* Kawi */, *b"tutg" /* Tulu_Tigalari */, *b"tayo" /* Tai_Yo */, *b"mong" /* Mongolian */,
    *b"khar" /* Kharoshthi */, *b"tfng" /* Tifinagh */, *b"nko " /* Nko */, *b"phag" /* Phags_Pa */,
    *b"egyp" /* Egyptian_Hieroglyphs */, *b"mand" /* Mandaic */, *b"plrd" /* Miao */, *b"dupl" /* Duployan */,
    *b"mani" /* Manichaean */, *b"hmng" /* Pahawh_Hmong */, *b"phlp" /* Psalter_Pahlavi */, *b"adlm" /* Adlam */,
    *b"rohg" /* Hanifi_Rohingya */, *b"medf" /* Medefaidrin */, *b"sogo" /* Old_Sogdian */, *b"sogd" /* Sogdian */,
    *b"elym" /* Elymaic */, *b"hmnp" /* Nyiakeng_Puachue_Hmong */, *b"wcho" /* Wancho */, *b"chrs" /* Chorasmian */,
    *b"kits" /* Khitan_Small_Script */, *b"yezi" /* Yezidi */, *b"cpmn" /* Cypro_Minoan */, *b"ougr" /* Old_Uyghur */,
    *b"tnsa" /* Tangsa */, *b"toto" /* Toto */, *b"vith" /* Vithkuqi */, *b"nagm" /* Nag_Mundari */,
    *b"gara" /* Garay */, *b"gukh" /* Gurung_Khema */, *b"krai" /* Kirat_Rai */, *b"onao" /* Ol_Onal */,
    *b"sunu" /* Sunuwar */, *b"todr" /* Todhri */, *b"berf" /* Beria_Erfe */, *b"sidt" /* Sidetic */,
    *b"tols" /* Tolong_Siki */, *b"hluw" /* Anatolian_Hieroglyphs */, *b"arab" /* Arabic */, *b"armn" /* Armenian */,
    *b"avst" /* Avestan */, *b"bamu" /* Bamum */, *b"bass" /* Bassa_Vah */, *b"bopo" /* Bopomofo */,
    *b"brai" /* Braille */, *b"cans" /* Canadian_Aboriginal */, *b"cari" /* Carian */, *b"aghb" /* Caucasian_Albanian */,
    *b"cher" /* Cherokee */, *b"copt" /* Coptic */, *b"xsux" /* Cuneiform */, *b"cprt" /* Cypriot */,
    *b"cyrl" /* Cyrillic */, *b"dsrt" /* Deseret */, *b"elba" /* Elbasan */, *b"ethi" /* Ethiopic */,
    *b"geor" /* Georgian */, *b"glag" /* Glagolitic */, *b"goth" /* Gothic */, *b"grek" /* Greek */,
    *b"hani" /* Han */, *b"hang" /* Hangul */, *b"hatr" /* Hatran */, *b"hebr" /* Hebrew */,
    *b"kana" /* Hiragana */, *b"armi" /* Imperial_Aramaic */, *b"phli" /* Inscriptional_Pahlavi */, *b"prti" /* Inscriptional_Parthian */,
    *b"kana" /* Katakana */, *b"latn" /* Latin */, *b"lina" /* Linear_A */, *b"linb" /* Linear_B */,
    *b"lisu" /* Lisu */, *b"lyci" /* Lycian */, *b"lydi" /* Lydian */, *b"mend" /* Mende_Kikakui */,
    *b"merc" /* Meroitic_Cursive */, *b"mero" /* Meroitic_Hieroglyphs */, *b"mroo" /* Mro */, *b"nbat" /* Nabataean */,
    *b"talu" /* New_Tai_Lue */, *b"nshu" /* Nushu */, *b"ogam" /* Ogham */, *b"olck" /* Ol_Chiki */,
    *b"hung" /* Old_Hungarian */, *b"ital" /* Old_Italic */, *b"narb" /* Old_North_Arabian */, *b"perm" /* Old_Permic */,
    *b"xpeo" /* Old_Persian */, *b"sarb" /* Old_South_Arabian */, *b"orkh" /* Old_Turkic */, *b"osge" /* Osage */,
    *b"osma" /* Osmanya */, *b"palm" /* Palmyrene */, *b"pauc" /* Pau_Cin_Hau */, *b"phnx" /* Phoenician */,
    *b"runr" /* Runic */, *b"samr" /* Samaritan */, *b"shaw" /* Shavian */, *b"sgnw" /* SignWriting */,
    *b"sora" /* Sora_Sompeng */, *b"syrc" /* Syriac */, *b"tang" /* Tangut */, *b"thaa" /* Thaana */,
    *b"ugar" /* Ugaritic */, *b"vai " /* Vai */, *b"wara" /* Warang_Citi */, *b"yi  " /* Yi */,
];

/// Two-letter languages and their OpenType tags as HarfBuzz has them: six bytes each, sorted by
/// language.
const LANGUAGES: &[u8; 1224] = b"aaAFR abABK afAFK akAKA amAMH anARG arARA asASM avAVR ayAYM azAZE baBSH beBEL bgBGR biBIS biCPP bmBMB bnBEN boTIB brBRE bsBOS caCAT ceCHE chCHA coCOS crCRE csCSY cuCSL cvCHU cyWEL daDAN deDEU dvDIV dvDHV dzDZN eeEWE elELL enENG eoNTO esESP etETI euEUQ faFAR ffFUL fiFIN fjFJI foFOS frFRA fyFRI gaIRI gaIRT gdGAE glGAL gnGUA guGUJ gvMNX haHAU heIWR hiHIN hoHMO hoCPP hrHRV htHAI htCPP huHUN hyHYE0hyHYE hzHER iaINA idIND idMLY ieILE igIBO iiYIM ikIPK inIND inMLY ioIDO isISL itITA iuINU iuINUKiwIWR jaJAN jiJII jvJAV jwJAV kaKAT kgKON0kiKIK kjKUA kkKAZ klGRN kmKHM knKAN koKOR koKOH krKNR ksKSH kuKUR kvKOM kwCOR kyKIR laLAT lbLTZ lgLUG liLIM lnLIN loLAO ltLTH luLUB lvLVI mgMLG mhMAH miMRI mkMKD mlMAL mlMLR mnMNG moMOL moROM mrMAR msMLY mtMTS myBRM naNAU nbNOR ndNDB neNEP ngNDG nlNLD nnNYN noNOR nrNDB nvNAV nvATH nyCHI ocOCI ojOJB omORO orORI osOSS paPAN piPAL plPLK psPAS ptPTG quQUZ rmRMS rnRUN roROM ruRUS rwRUA saSAN scSRD sdSND seNSM sgSGO shBOS shHRV shSRB siSNH skSKY slSLV smSMO snSNA0soSML sqSQI srSRB ssSWZ stSOT suSUN svSVE swSWK taTAM teTEL tgTAJ thTHA tiTGY tkTKM tlTGL tnTNA toTGN trTRK tsTSG ttTAT twTWI twAKA tyTHT ugUYG ukUKR urURD uzUZB veVEN viVIT voVOL waWLN woWLF xhXHS yiJII yoYBA zaZHA zhZHS zuZUL ";

/// Unicode's decimal digits as ranges: the first (times 256) and how many.
#[rustfmt::skip]
const DIGITS: [u32; 72] = [
    0x300a, 0x6600a, 0x6f00a, 0x7c00a, 0x9660a, 0x9e60a, 0xa660a, 0xae60a, 0xb660a, 0xbe60a, 0xc660a, 0xce60a,
    0xd660a, 0xde60a, 0xe500a, 0xed00a, 0xf200a, 0x10400a, 0x10900a, 0x17e00a, 0x18100a, 0x19460a, 0x19d00a, 0x1a800a,
    0x1a900a, 0x1b500a, 0x1bb00a, 0x1c400a, 0x1c500a, 0xa6200a, 0xa8d00a, 0xa9000a, 0xa9d00a, 0xa9f00a, 0xaa500a, 0xabf00a,
    0xff100a, 0x104a00a, 0x10d300a, 0x10d400a, 0x110660a, 0x110f00a, 0x111360a, 0x111d00a, 0x112f00a, 0x114500a, 0x114d00a, 0x116500a,
    0x116c00a, 0x116d014, 0x117300a, 0x118e00a, 0x119500a, 0x11bf00a, 0x11c500a, 0x11d500a, 0x11da00a, 0x11de00a, 0x11f500a, 0x161300a,
    0x16a600a, 0x16ac00a, 0x16b500a, 0x16d700a, 0x1ccf00a, 0x1d7ce32, 0x1e1400a, 0x1e2f00a, 0x1e4f00a, 0x1e5f10a, 0x1e9500a, 0x1fbf00a,
];
