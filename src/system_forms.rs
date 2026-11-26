//! The system's text in a language's forms (feature `shaping`): Serbian and Macedonian б, г, д,
//! п, т; Bulgarian Cyrillic; Romanian ș with a comma; Japanese, Chinese or Korean forms of a
//! CJK character. GDI, FreeType and CoreText take a character's glyph from the font's character
//! map alone. Where the font a letter is drawn in has localized forms (`locl`) for the window's
//! language, the system's text draws those glyphs instead, found in the font's own tables as
//! text in app fonts finds them (`shaping::local`).
//!
//! A line of simple script (every character in the face's own font, none a mark or of a script
//! shaped by syllable or right to left) is still drawn by GDI or FreeType, with those glyphs.
//! Any other line with a letter that takes a form (a mark after it, a letter of a fallback
//! font; on macOS and iOS every such line) is shaped in the fonts its letters are drawn in, as
//! right-to-left text is (`bidi.rs`). Scripts shaped by syllable take their forms there too,
//! with `brahmic`.
use crate::font_file::FontFile;
use crate::scripts::Language;
use crate::shaping::Script;
use crate::text::Face;
use std::collections::HashMap;

/// A language's forms in the system's fonts, as looked up so far.
pub(crate) struct Forms {
    language: Language,
    /// Each face's own font.
    fonts: Vec<OwnFont>,
    /// The font a face draws a script's letters in where its own lacks them, if it has forms
    /// for the language.
    fallbacks: Vec<(Face, Option<Script>, Option<FontFile>)>,
    /// Each character's form looked up, by face: none where it has none, or the font lacks it.
    found: HashMap<(Face, char), Option<u16>>,
    /// Whether a letter the face's font lacks takes a form in the font drawing it, by face.
    fallen: HashMap<(Face, char), bool>,
}

/// A face's own font, if the system gives its file: whether it has forms for the language, and
/// whether some for Latin, which ASCII text may take.
struct OwnFont {
    face: Face,
    file: Option<FontFile>,
    forms: bool,
    latin: bool,
}

impl std::fmt::Debug for Forms {
    /// The language, and how many characters were looked up.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forms").field("language", &self.language).field("found", &self.found.len()).finish_non_exhaustive()
    }
}

impl Forms {
    /// The system language's forms.
    pub fn new() -> Forms {
        Forms { language: Language::of_the_system(), fonts: Vec::new(), fallbacks: Vec::new(), found: HashMap::new(), fallen: HashMap::new() }
    }

    /// Takes another language's forms from now on: whether that is a change.
    pub fn set(&mut self, language: Language) -> bool {
        if language == self.language {
            return false;
        }
        *self = Forms { language, ..Forms::new() };
        true
    }

    /// A line's glyphs in a face's font where a character of it takes a form there: each
    /// character's own and the one drawn, one per UTF-16 unit, as GDI and FreeType count them.
    /// Nothing where none does, or the line is not all simple script in that font (see above).
    #[cfg(not(target_vendor = "apple"))]
    pub fn glyphs(&mut self, face: Face, text: &str) -> Option<Vec<[u16; 2]>> {
        if self.language == Language::NONE || (text.is_ascii() && !self.font(face).is_some_and(|(_, latin)| latin)) {
            return None;
        }
        let mut any = false;
        for letter in text.chars() {
            any |= self.form(face, letter)?.is_some();
        }
        self.font(face).filter(|_| any)?;
        let file = self.fonts.iter().find(|own| own.face == face)?.file.as_ref()?;
        let glyph = |letter: char| {
            let own = file.glyph(letter);
            [own, self.found.get(&(face, letter)).copied().flatten().unwrap_or(own)]
        };
        Some(text.chars().map(glyph).collect())
    }

    /// Whether a line is shaped in its letters' fonts for their forms (see above): it has a
    /// letter that takes one, and is no line `glyphs` gives (on Apple's systems, any line).
    pub fn shaped_here(&mut self, face: Face, text: &str) -> bool {
        if self.language == Language::NONE || (text.is_ascii() && !self.font(face).is_some_and(|(_, latin)| latin)) {
            return false;
        }
        #[cfg(not(target_vendor = "apple"))]
        if self.glyphs(face, text).is_some() {
            return false;
        }
        text.chars().any(|letter| self.takes_form(face, letter))
    }

    /// Whether a letter takes a form in the font it is drawn in: the face's own, or where that
    /// lacks it, its fallback's.
    fn takes_form(&mut self, face: Face, letter: char) -> bool {
        if letter > '\u{ffff}' || !matches!(Script::of(letter), Some(Script::Plain(_))) {
            return false;
        }
        match self.own(face).file.as_ref().map(|file| file.glyph(letter) != 0) {
            Some(true) => self.form(face, letter).flatten().is_some(),
            _ => self.falls_into_form(face, letter),
        }
    }

    /// Whether a letter the face's font lacks takes a form in the fallback font drawing it.
    fn falls_into_form(&mut self, face: Face, letter: char) -> bool {
        if let Some(takes) = self.fallen.get(&(face, letter)) {
            return *takes;
        }
        let (script, language) = (Script::of(letter), self.language);
        if !self.fallbacks.iter().any(|(known, of, _)| *known == face && *of == script) {
            let file = crate::system_fonts::file_for(face, letter).filter(|file| has_forms(file, language, None));
            self.fallbacks.push((face, script, file));
        }
        let file = self.fallbacks.iter().find(|(known, of, _)| *known == face && *of == script).and_then(|(.., file)| file.as_ref());
        let takes = file.is_some_and(|file| file.glyph(letter) != 0 && crate::shaping::local(file, letter, language) != file.glyph(letter));
        self.fallen.insert((face, letter), takes);
        takes
    }

    /// A character's form, if it has one: nothing where the line is to be left to the system
    /// (a character the font lacks, a mark, one beyond the 65,536 first, one of a script
    /// shaped otherwise).
    fn form(&mut self, face: Face, letter: char) -> Option<Option<u16>> {
        use crate::bidi::{Kind, kind};
        let jamo = matches!(letter, '\u{1100}'..='\u{11ff}' | '\u{a960}'..='\u{a97f}' | '\u{d7b0}'..='\u{d7ff}');
        let simple = letter <= '\u{ffff}' && !matches!(kind(letter), Kind::Mark | Kind::Right | Kind::Arabic) && matches!(Script::of(letter), None | Some(Script::Plain(_)));
        if !simple || jamo || letter.is_control() {
            return None;
        }
        if let Some(found) = self.found.get(&(face, letter)) {
            return Some(*found);
        }
        let language = self.language;
        let (file, _) = self.font(face)?;
        let own = file.glyph(letter);
        let form = crate::shaping::local(file, letter, language);
        (own != 0).then_some(())?;
        let found = (form != own).then_some(form);
        self.found.insert((face, letter), found);
        Some(found)
    }

    /// A face's font, where it has forms for the language; and whether it has some for Latin.
    fn font(&mut self, face: Face) -> Option<(&FontFile, bool)> {
        let own = self.own(face);
        Some((own.file.as_ref().filter(|_| own.forms)?, own.latin))
    }

    /// A face's own font, looked up the first time.
    fn own(&mut self, face: Face) -> &OwnFont {
        if let Some(at) = self.fonts.iter().position(|own| own.face == face) {
            return &self.fonts[at];
        }
        let file = crate::system_fonts::file_for(face, 'a');
        let forms = file.as_ref().is_some_and(|file| has_forms(file, self.language, None));
        let latin = forms && file.as_ref().is_some_and(|file| has_forms(file, self.language, Some(b"latn")));
        self.fonts.push(OwnFont { face, file, forms, latin });
        &self.fonts[self.fonts.len() - 1]
    }
}

/// Whether a font has localized forms for a language: under a script (`only`), or any.
fn has_forms(file: &FontFile, language: Language, only: Option<&[u8; 4]>) -> bool {
    let (gsub, languages) = (file.table(b"GSUB"), language.systems().copied().collect::<Vec<_>>());
    let scripts = gsub.u16(4).filter(|_| gsub.u16(0) == Some(1)).unwrap_or(0) as usize;
    let count = if scripts == 0 { 0 } else { gsub.u16(scripts).unwrap_or(0) as usize };
    let tags = (0..count).filter_map(|n| gsub.u32(scripts + 2 + n * 6)).map(u32::to_be_bytes);
    tags.filter(|tag| only.is_none_or(|only| tag == only)).any(|tag| crate::font_file::lookups(gsub, (&[&tag], &languages, file.place()), &[b"locl"]).is_some_and(|found| !found.is_empty()))
}

// (Apple's systems swap no forms into lines: see `bidi.rs` for theirs.)
#[cfg(all(test, not(target_vendor = "apple")))]
mod tests {
    use super::*;
    use crate::text::{Font, TextSystem};

    /// System families with forms of their own for a language, where installed, and words that
    /// take them.
    const FAMILIES: [(&str, &str, &str); 6] = [
        ("Bahnschrift", "sr", "бгдпт"),
        ("Bahnschrift", "bg", "вдгжзийклптцчшщъю"),
        ("Calibri", "ro", "şţ"),
        ("Noto Sans", "bg", "вдгжзийклптцчшщъю"),
        ("Noto Sans", "sr", "бгдпт"),
        ("DejaVu Sans", "sr", "бгдпт"),
    ];

    /// A word in a face, as the system's text draws it on any surface in a language: its pixels.
    fn drawn(face: Face, word: &str, language: Language) -> Vec<u32> {
        let mut text = crate::platform::SystemText::new().expect("the system's text");
        text.set_language(language);
        let mut pixels = vec![0u32; 400 * 60];
        text.draw(&mut crate::canvas::Canvas::new(&mut pixels, 400, 60, (0, 0), 1.0), word, 4.0, 4.0, Font { face, ..Font::new(32.0) }, crate::color::Color::WHITE);
        pixels
    }

    #[test]
    fn the_systems_text_takes_a_languages_forms() {
        let mut tried = 0;
        for (family, language, word) in FAMILIES {
            let (face, language) = (Face::named(family), Language::named(language));
            let mut forms = Forms::new();
            forms.set(language);
            // (Where the family is not installed the system's own font stands in: one without
            // such forms, often.)
            let Some(glyphs) = forms.glyphs(face, word) else { continue };
            let file = crate::system_fonts::file_for(face, 'a').expect("a font with forms");
            let theirs: Vec<[u16; 2]> = word.chars().map(|letter| [file.glyph(letter), crate::shaping::local(&file, letter, language)]).collect();
            assert_eq!(glyphs, theirs, "{family}, {word}");
            assert!(glyphs.iter().any(|[own, form]| own != form), "{family}, {word}");
            // Unchanged lines are left to the system: ASCII where the font has no Latin forms, a
            // mark, a letter the font lacks.
            assert_eq!([forms.glyphs(face, "Hello"), forms.glyphs(face, &format!("{word}\u{301}")), forms.glyphs(face, &format!("{word}\u{10380}"))], [None, None, None]);
            // The system's text draws them: other pictures than in no language, or in Russian.
            let [own, plain, russian] = [language, Language::NONE, Language::named("ru")].map(|language| drawn(face, word, language));
            assert!(own != plain && plain == russian, "{family}, {word}");
            // And measures them: on Windows by GDI, the glyphs' own widths.
            let mut text = crate::platform::SystemText::new().unwrap();
            let size = Font { face, ..Font::new(32.0) };
            text.set_language(language);
            let wide = text.measure(word, size, 1.0, None);
            assert!(wide > 0.0, "{family}");
            tried += 1;
        }
        if tried == 0 {
            eprintln!("no system font with such forms: not tested");
        }
    }
}
