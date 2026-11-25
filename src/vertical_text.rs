//! Vertical text, top to bottom with lines right to left, as Chinese, Japanese and Korean are
//! written: how each character stands in a column, and the fonts it is written in. Columns are
//! laid out in `vertical_layout.rs`; labels and paragraphs are `vertical_paragraph.rs`, text
//! boxes `vertical_text_box.rs`.
//!
//! Each character stands as Unicode's `Vertical_Orientation` (UAX #50) says: upright
//! (ideographs, kana, Hangul) or rotated a quarter turn clockwise (Latin and other horizontal
//! scripts, reading down the column). A few take vertical forms (East Asian full stop and comma
//! at the top right, brackets opening downwards, the long vowel mark drawn down): from the
//! font's `vert`, or in a font without it, Unicode's own vertical-form characters (as HarfBuzz
//! does). Of those Unicode allows either way, one with no such form in the font is rotated.
//!
//! Upright runs are shaped vertically (`Plan::downward`): each glyph centred in the column,
//! advancing by the font's `vmtx`, its top below the pen as `VORG` or `vmtx` say (as HarfBuzz
//! does; held against it). Rotated runs are shaped horizontally and turned, their letters'
//! middle on the column's middle. Each piece uses the font of the text system that has its
//! first letter: the app's own ([`AppFonts`](crate::AppFonts)), or the system's CJK font for
//! the language (on Windows Yu Gothic, Microsoft YaHei, Microsoft JhengHei or Malgun Gothic;
//! elsewhere what Fontconfig names). The label is its glyphs' outlines, filled as a shape.
//!
//! Not done: the systems' own vertical engines (GDI's "@" fonts, FreeType's vertical layout); a
//! font's `vrt2` (rotated glyphs for layouts that rotate nothing), as in HarfBuzz and browsers;
//! `GPOS` for vertical text only (`vkrn`, `vpal`), which HarfBuzz does not apply by itself
//! either.
use crate::font_file::FontFile;
use crate::scripts::Language;
use crate::shaping::{Plan, Script};

/// How a character stands in a column (UAX #50).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stands {
    /// Rotated a quarter turn clockwise (R).
    Turned,
    /// Upright (U).
    Upright,
    /// Upright, in a vertical form where the font has one (Tu).
    UprightInForm,
    /// In a vertical form where the font has one, else rotated (Tr).
    InFormOrTurned,
}

/// How a character stands in a column, by Unicode's table.
pub(crate) fn stands(letter: char) -> Stands {
    let code = letter as u32;
    let n = STANDING.partition_point(|stretch| stretch >> 2 <= code);
    match n.checked_sub(1).map_or(0, |n| STANDING[n] & 3) {
        1 => Stands::Upright,
        2 => Stands::UprightInForm,
        3 => Stands::InFormOrTurned,
        _ => Stands::Turned,
    }
}

/// Unicode's vertical-form character for a character (HarfBuzz's list), for fonts with no
/// `vert`.
pub(crate) fn form(letter: char) -> Option<char> {
    let code = letter as u32;
    let found = FORMS.iter().find(|(of, _)| *of as u32 == code)?;
    char::from_u32(found.1 as u32)
}

/// The characters of `form`, each with its vertical form.
#[rustfmt::skip]
const FORMS: [(u16, u16); 34] = [
    (0x2013, 0xfe32), (0x2014, 0xfe31), (0x2025, 0xfe30), (0x2026, 0xfe19), (0x3001, 0xfe11), (0x3002, 0xfe12),
    (0x3008, 0xfe3f), (0x3009, 0xfe40), (0x300a, 0xfe3d), (0x300b, 0xfe3e), (0x300c, 0xfe41), (0x300d, 0xfe42),
    (0x300e, 0xfe43), (0x300f, 0xfe44), (0x3010, 0xfe3b), (0x3011, 0xfe3c), (0x3014, 0xfe39), (0x3015, 0xfe3a),
    (0x3016, 0xfe17), (0x3017, 0xfe18), (0xfe4f, 0xfe34), (0xff01, 0xfe15), (0xff08, 0xfe35), (0xff09, 0xfe36),
    (0xff0c, 0xfe10), (0xff1a, 0xfe13), (0xff1b, 0xfe14), (0xff1f, 0xfe16), (0xff3b, 0xfe47), (0xff3d, 0xfe48),
    (0xff3f, 0xfe33), (0xff5b, 0xfe37), (0xff5d, 0xfe38), (0, 0),
];

/// Where a rotated run's baseline goes across the column: left of the middle by enough that its
/// letters' middle (between ascent and descent) is on it.
pub(crate) fn middle(font: &FontFile, unit: f32) -> f32 {
    let metrics = font.metrics(unit * font.units());
    -(metrics.ascent - metrics.descent) / 2.0
}

/// The length in bytes of a column's next run, and whether it is rotated: as far as its
/// characters stand alike, each with what belongs to it (combining marks, emoji sequences). A
/// Tr character stands upright where the font's `vert` (or Unicode's forms) gives it a form.
pub(crate) fn next_run(fonts: &[&FontFile], text: &str) -> (usize, bool) {
    let turned = |cluster: &str| {
        let first = cluster.chars().next().unwrap_or(' ');
        match stands(first) {
            Stands::Turned => true,
            Stands::Upright | Stands::UprightInForm => false,
            Stands::InFormOrTurned => !has_form(fonts, first),
        }
    };
    let first = crate::text::cluster(text);
    let way = turned(&text[..first]);
    let mut long = first;
    while long < text.len() {
        let size = crate::text::cluster(&text[long..]);
        if turned(&text[long..long + size]) != way {
            break;
        }
        long += size;
    }
    (long, way)
}

/// Whether the font writing a character has a vertical form for it: its `vert` substitutes it,
/// or (without `vert`) it has Unicode's.
fn has_form(fonts: &[&FontFile], letter: char) -> bool {
    let Some(font) = fonts.iter().find(|font| font.glyph(letter) != 0) else { return false };
    let plan = Plan::downward(font, Script::of(letter).unwrap_or(Script::None), Language::NONE);
    let mut glyphs = Vec::new();
    font.shape(&plan, letter.encode_utf8(&mut [0; 4]), &mut glyphs);
    glyphs.first().is_some_and(|laid| laid.glyph != font.glyph(letter))
}

thread_local! {
    /// The system's CJK font for vertical text, looked up once.
    static SYSTEM: std::cell::OnceCell<Option<std::rc::Rc<FontFile>>> = const { std::cell::OnceCell::new() };
}

/// The system's CJK font for the system's language (see `find_system_font`), looked up once.
pub(crate) fn system_font() -> Option<std::rc::Rc<FontFile>> {
    SYSTEM.with(|system| system.get_or_init(|| find_system_font(Language::of_the_system()).map(std::rc::Rc::new)).clone())
}

/// The system's CJK font for the language: on Windows Microsoft YaHei (Simplified Chinese),
/// Microsoft JhengHei (Traditional), Malgun Gothic (Korean), else Yu Gothic.
#[cfg(windows)]
fn find_system_font(language: Language) -> Option<FontFile> {
    let folder = std::path::Path::new(&std::env::var_os("WINDIR")?).join("Fonts");
    let named = |tags: &[&[u8; 4]]| language.systems().any(|system| tags.contains(&system));
    let names: &[&str] = match () {
        _ if named(&[b"ZHS "]) => &["msyh.ttc", "simsun.ttc"],
        _ if named(&[b"ZHT ", b"ZHH "]) => &["msjh.ttc", "mingliub.ttc"],
        _ if named(&[b"KOR "]) => &["malgun.ttf", "gulim.ttc"],
        _ => &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc", "msyh.ttc", "malgun.ttf"],
    };
    names.iter().find_map(|name| FontFile::open(folder.join(name)))
}

/// Elsewhere, the font Fontconfig names for a character of the language: kana for Japanese and
/// others, Hangul for Korean, an ideograph for Chinese.
#[cfg(not(windows))]
fn find_system_font(language: Language) -> Option<FontFile> {
    let named = |tags: &[&[u8; 4]]| language.systems().any(|system| tags.contains(&system));
    let letter = match () {
        _ if named(&[b"KOR "]) => '가',
        _ if named(&[b"ZHS ", b"ZHT ", b"ZHH "]) => '中',
        _ => 'あ',
    };
    crate::bidi::system(crate::text::Face::Regular, letter)
}

/// How characters stand in a column, as ranges: each range's first character (times 4) and its
/// kind (0 R, 1 U, 2 Tu, 3 Tr). Unicode 17 `VerticalOrientation.txt`.
#[rustfmt::skip]
const STANDING: [u32; 280] = [
    0x0, 0x29d, 0x2a0, 0x2a5, 0x2a8, 0x2b9, 0x2bc, 0x2c5, 0x2c8, 0x2f1,
    0x2fc, 0x35d, 0x360, 0x3dd, 0x3e0, 0xba9, 0xbb0, 0x4401, 0x4800, 0x5005,
    0x5a00, 0x62c1, 0x6400, 0x8059, 0x805c, 0x8063, 0x8068, 0x8073, 0x8078, 0x8081,
    0x8088, 0x80c1, 0x80c8, 0x80ed, 0x80f4, 0x8109, 0x810c, 0x811d, 0x8128, 0x8145,
    0x8148, 0x8195, 0x8198, 0x8375, 0x8384, 0x8389, 0x8394, 0x8401, 0x8408, 0x840d,
    0x8428, 0x843d, 0x8440, 0x844d, 0x8454, 0x8459, 0x8460, 0x8479, 0x8490, 0x8495,
    0x8498, 0x849d, 0x84a0, 0x84a5, 0x84a8, 0x84b9, 0x84bc, 0x84d5, 0x8500, 0x8515,
    0x852c, 0x8531, 0x8538, 0x853d, 0x8628, 0x8631, 0x8640, 0x8879, 0x887c, 0x88d1,
    0x88d8, 0x8c01, 0x8c20, 0x8c31, 0x8c80, 0x8c91, 0x8ca7, 0x8cad, 0x8cb0, 0x8df5,
    0x8e6c, 0x8ef9, 0x8f38, 0x8f3d, 0x8f40, 0x8f45, 0x8f70, 0x8f89, 0x908c, 0x9091,
    0x9400, 0x9681, 0x9868, 0x9881, 0x9da0, 0x9dd9, 0x9e50, 0xac49, 0xacc0, 0xad41,
    0xad68, 0xae5d, 0xae60, 0xaee1, 0xaf48, 0xaf4d, 0xafb0, 0xafc1, 0xb000, 0xb941,
    0xb948, 0xba01, 0xc006, 0xc00d, 0xc023, 0xc049, 0xc053, 0xc081, 0xc0c3, 0xc0c5,
    0xc106, 0xc109, 0xc10e, 0xc111, 0xc116, 0xc119, 0xc11e, 0xc121, 0xc126, 0xc129,
    0xc18e, 0xc191, 0xc20e, 0xc211, 0xc216, 0xc219, 0xc21e, 0xc221, 0xc23a, 0xc23d,
    0xc256, 0xc25d, 0xc26e, 0xc275, 0xc283, 0xc286, 0xc289, 0xc28e, 0xc291, 0xc296,
    0xc299, 0xc29e, 0xc2a1, 0xc2a6, 0xc2a9, 0xc30e, 0xc311, 0xc38e, 0xc391, 0xc396,
    0xc399, 0xc39e, 0xc3a1, 0xc3ba, 0xc3bd, 0xc3d6, 0xc3dd, 0xc3f3, 0xc3f5, 0xc49e,
    0xc4a1, 0xc6d2, 0xc6e1, 0xc6ee, 0xc6f1, 0xc7c2, 0xc801, 0xcbfe, 0xcd61, 0xcdee,
    0xce01, 0x29340, 0x2a581, 0x2a600, 0x2b001, 0x36000, 0x38001, 0x3ec00, 0x3f841, 0x3f880,
    0x3f8c1, 0x3f924, 0x3f942, 0x3f94d, 0x3f960, 0x3f967, 0x3f97d, 0x3f98c, 0x3f99d, 0x3f9c0,
    0x3fc06, 0x3fc09, 0x3fc23, 0x3fc29, 0x3fc32, 0x3fc34, 0x3fc3a, 0x3fc3d, 0x3fc6b, 0x3fc70,
    0x3fc7e, 0x3fc81, 0x3fcef, 0x3fcf1, 0x3fcf7, 0x3fcf9, 0x3fcff, 0x3fd01, 0x3fd6f, 0x3fd84,
    0x3ff81, 0x3ff8f, 0x3ff91, 0x3ffa0, 0x3ffc1, 0x3ffe4, 0x3fff1, 0x3fff8, 0x42601, 0x42680,
    0x45601, 0x45800, 0x46801, 0x46b00, 0x4c001, 0x51a00, 0x5bf81, 0x63800, 0x6bfc1, 0x6c4ca,
    0x6c4cd, 0x6c542, 0x6c54d, 0x6c556, 0x6c559, 0x6c592, 0x6c5a1, 0x6cc00, 0x73b01, 0x73f40,
    0x74001, 0x74800, 0x74b81, 0x74e00, 0x76001, 0x76ac0, 0x7c001, 0x7c802, 0x7c809, 0x7e000,
    0x7e401, 0x7ec00, 0x80001, 0xbfff8, 0xc0001, 0xffff8, 0x3c0001, 0x3ffff8, 0x400001, 0x43fff8,
];

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::font_file::tests::{file, glyph, longs, one_for_one, positions, tables_of, with, words};

    /// A 1024-unit font for vertical text: a box for "あ" (U+3042), the ideographic full stop
    /// (U+3002) low at the left, its vertical form high at the right (substituted by `vert`, or
    /// as U+FE12 with `unicode`), and an "A" half as wide; with heights in `vmtx` and top
    /// bearings.
    pub(crate) fn writing_down(vert: bool, unicode: bool) -> FontFile {
        // Each with its bounding box before its points, as glyphs have it.
        let boxed = |x: i32, y: i32, w: i32, h: i32| {
            let mut points = glyph(&[&[(x, y, true), (x, y + h, true), (x + w, y + h, true), (x + w, y, true)]]);
            points[2..10].copy_from_slice(&words(&[x, y, x + w, y + h]));
            points
        };
        let glyphs = vec![boxed(0, 0, 800, 800), boxed(50, 0, 200, 200), boxed(700, 700, 200, 200), boxed(100, -100, 800, 900), boxed(0, 0, 500, 700)];
        let mut letters = vec![(0x41u32, 4), (0x3002, 1), (0x3042, 3)];
        if unicode {
            letters.push((0xfe12, 2));
        }
        let groups: Vec<u8> = letters.iter().flat_map(|(letter, glyph)| longs(&[*letter, *letter, *glyph])).collect();
        let cmap = [words(&[0, 1, 3, 10]), longs(&[12]), words(&[12, 0]), longs(&[16 + groups.len() as u32, 0, letters.len() as u32]), groups].concat();
        let mut tables = with(tables_of(glyphs), b"cmap", cmap);
        tables = with(tables, b"maxp", [words(&[1, 0, 5]), vec![0; 26]].concat());
        tables = with(tables, b"hhea", [words(&[1, 0, 832, -192]), vec![0; 26], words(&[5])].concat());
        tables = with(tables, b"hmtx", words(&[1000, 0, 1000, 50, 1000, 700, 1000, 100, 500, 0]));
        tables.push((*b"vhea", [words(&[1, 0x1000, 512, -512]), vec![0; 26], words(&[4])].concat()));
        tables.push((*b"vmtx", words(&[1024, 112, 1024, 812, 1024, 112, 1024, 12, 312])));
        if vert {
            tables.push((*b"GSUB", positions(&[(b"DFLT", &[0])], &[(b"vert", &[0])], &[(1, vec![one_for_one(&[1], &[2])])])));
        }
        tables.sort_by_key(|(name, _)| *name);
        FontFile::read(file(&[0, 1, 0, 0], &tables)).expect("the font is read")
    }

    /// A word shaped vertically: each glyph, the pen's advance down after it, and its origin
    /// from the pen.
    fn down(font: &FontFile, word: &str) -> Vec<(u16, i32, [i32; 2])> {
        let mut glyphs = Vec::new();
        font.shape(&Plan::downward(font, Script::of('あ').unwrap(), Language::NONE), word, &mut glyphs);
        glyphs.iter().map(|laid| (laid.glyph, laid.advance, laid.offset)).collect()
    }

    #[test]
    fn each_character_stands_as_unicode_says() {
        let letters = ['あ', '漢', '한', '\u{1f600}', 'A', '1', '-', '\u{3002}', '\u{ff01}', '\u{30fc}', '\u{300c}', '\u{ff08}'];
        let (u, r, tu, tr) = (Stands::Upright, Stands::Turned, Stands::UprightInForm, Stands::InFormOrTurned);
        assert_eq!(letters.map(stands), [u, u, u, u, r, r, r, tu, tu, tr, tr, tr]);
        assert_eq!((form('\u{3002}'), form('\u{300c}'), form('あ')), (Some('\u{fe12}'), Some('\u{fe41}'), None));
    }

    #[test]
    fn written_down_each_glyph_goes_by_its_height_in_the_middle_of_the_column() {
        let font = writing_down(true, false);
        // The pen advances by each glyph's height; each glyph is half its width left and its
        // top (box top plus bearing) below the pen. The full stop takes its vertical form from
        // `vert`.
        assert_eq!(down(&font, "\u{3042}\u{3002}"), [(3, 1024, [-500, -(800 + 12)]), (2, 1024, [-500, -(900 + 112)])]);
        // Without `vert`, Unicode's form where the font has it; without either, as it is.
        assert_eq!(down(&writing_down(false, true), "\u{3002}"), [(2, 1024, [-500, -(900 + 112)])]);
        assert_eq!(down(&writing_down(false, false), "\u{3002}"), [(1, 1024, [-500, -(200 + 812)])]);
        // A glyph past those with their own `vmtx` height takes the last one's, and its top
        // bearing from the list after them.
        assert_eq!(down(&font, "A"), [(4, 1024, [-250, -(700 + 312)])]);
    }

    #[test]
    fn a_label_written_down_takes_a_column_and_is_told_as_text() {
        let mut ui = crate::ui::Ui::new(Box::new(crate::font_file::AppFonts::new(writing_down(true, false))), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 200.0, h: 200.0 }, 2.0);
        ui.set_screen_reader(true);
        ui.run(&mut |ui: &mut crate::ui::Ui| {
            ui.column().pad(10.0).open();
            ui.vertical_label(
                "あ。
A",
            );
            ui.close();
        });
        assert!(ui.visible_access_nodes().iter().any(|said| said.name
            == "あ。
A"));
        // Two columns a line wide each (from 10 units in), as long as the longer: ink in the
        // right one where "あ" is and in the left where "A" is, none past their end.
        let size = ui.theme.font.size;
        let screen = crate::surface::Surface::draw(&mut ui, 64);
        let ink = |x: f32, y: f32| screen.pixels[(y * 2.0) as usize * screen.width + (x * 2.0) as usize] != screen.pixels[0];
        let (wide, long) = (size.ceil(), (2.0 * size).ceil());
        assert!(ink(10.0 + wide * 1.5, 10.0 + size * 0.5) && ink(10.0 + wide * 0.5, 10.0 + size * 0.25));
        assert!(!ink(10.0 + wide * 1.5, 10.0 + long + 2.0) && !ink(10.0 + wide * 2.0 + 2.0, 10.0 + size * 0.5));
    }
}

/// Pictures of vertical text with the system's text and its Japanese font, into the folder
/// `TAUT_SHOTS` names, as `downward-<system>.png` (none without it).
#[cfg(test)]
mod pictures {
    #[test]
    fn pictures_of_japanese_written_down() {
        let Ok(folder) = std::env::var("TAUT_SHOTS") else { return eprintln!("TAUT_SHOTS names no folder: no pictures") };
        let text: Option<Box<dyn crate::text::TextSystem>> = crate::platform::SystemText::new().map(|text| Box::new(text) as Box<dyn crate::text::TextSystem>);
        let Some(text) = text else { return eprintln!("no text of the system's own here: no pictures") };
        let mut ui = crate::ui::Ui::new(text, Box::new(crate::platform::LocalClipboard::default()));
        ui.theme = crate::theme::Theme::light();
        ui.theme.font.size = 22.0;
        ui.resize(crate::geom::Size { w: 420.0, h: 560.0 }, 1.5);
        ui.run(&mut |ui: &mut crate::ui::Ui| {
            ui.row().pad(16.0).gap(24.0).open();
            ui.vertical_label("吾輩は猫である。名前はまだ無い。\nどこで生れたかとんと見当がつかぬ。\n「ＴＡＵＴ」で縦書き、Rust 1.90 も。");
            ui.vertical_label("〔注〕ラーメン（醤油）……\n東京都千代田区一丁目\nか\u{3099}と は\u{309a}、ゃゅょっ");
            ui.close();
        });
        let screen = crate::surface::Surface::draw(&mut ui, 64);
        let name = if cfg!(windows) { "windows" } else { "linux" };
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(std::path::Path::new(&folder).join(format!("downward-{name}.png")), crate::png::encode(screen.width, &screen.pixels).unwrap()).unwrap();
    }
}

/// Held against HarfBuzz's vertical layout on Linux: Chinese, Japanese and Korean with their
/// punctuation, in the systems' fonts.
#[cfg(all(test, target_os = "linux"))]
mod held {
    use super::*;

    #[test]
    fn upright_text_comes_to_the_glyphs_and_places_harfbuzz_gives_it_written_down() {
        let words: Vec<String> = [
            "縦書きの文章です。",
            "「こんにちは」と言った。",
            "日本語、中文、한국어",
            "ラーメン（醤油）",
            "東京都千代田区",
            "一二三四五六七八九十",
            "ぁぃぅぇぉっゃゅょ",
            "が\u{3099}ぱ",
            "か\u{3099}き\u{309a}",
            "我爱北京天安门！",
            "這是繁體中文。",
            "안녕하세요、세계。",
            "【注意】『本』〈一〉《二》",
            "……——",
            "〜～",
            "！？：；",
            "ｱｲｳｴｵ",
            "漢字\u{e0100}",
        ]
        .iter()
        .map(|word| word.to_string())
        .collect();
        let fonts = [
            "/mnt/c/Windows/Fonts/msgothic.ttc",
            "/mnt/c/Windows/Fonts/YuGothR.ttc",
            "/mnt/c/Windows/Fonts/meiryo.ttc",
            "/mnt/c/Windows/Fonts/msyh.ttc",
            "/mnt/c/Windows/Fonts/simsun.ttc",
            "/mnt/c/Windows/Fonts/msjh.ttc",
            "/mnt/c/Windows/Fonts/malgun.ttf",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSerifCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ];
        let (mut tried, mut differ) = (0, Vec::new());
        for path in fonts.iter().map(std::path::Path::new).filter(|path| path.exists()) {
            let Some(font) = FontFile::open(path) else { continue };
            let Some(theirs) = crate::shaping::harfbuzz::harfbuzz_downward(path, 0, &words) else { return eprintln!("no HarfBuzz on this system: not tested") };
            let mut here = 0;
            for (word, theirs) in words.iter().zip(theirs) {
                let script = word.chars().find_map(Script::of).unwrap_or(Script::None);
                let mut glyphs = Vec::new();
                let total = font.shape(&Plan::downward(&font, script, Language::NONE), word, &mut glyphs);
                let mut pen = 0;
                let mine: Vec<(u16, [i32; 2])> = glyphs.iter().map(|laid| (laid.glyph, [laid.offset[0], (pen, pen -= laid.advance).0 + laid.offset[1]])).collect();
                // Where the font lacks a letter, HarfBuzz places a mark by its class, which is
                // not done here: only glyphs are compared there.
                let whole = theirs.0.iter().all(|(glyph, _)| *glyph != 0);
                let same = if whole { (mine.clone(), -total) == theirs } else { mine.iter().map(|(glyph, _)| *glyph).eq(theirs.0.iter().map(|(glyph, _)| *glyph)) };
                if !same {
                    here += 1;
                    if here <= 3 {
                        println!("  {word}: {mine:?} {}, not {theirs:?}", -total);
                    }
                    differ.push(word.clone());
                }
                tried += 1;
            }
            println!("{}: {} words, {here} differ", path.display(), words.len());
        }
        println!("{tried} words and fonts held, {} differ", differ.len());
        assert!(differ.is_empty(), "{} words differ", differ.len());
    }
}
