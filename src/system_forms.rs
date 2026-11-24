//! The system's text in a language's forms (feature `shaping`): Serbian and Macedonian б, г, д,
//! п, т; Bulgarian Cyrillic; Romanian ș with a comma; Japanese, Chinese or Korean forms of a
//! CJK character. GDI and FreeType take a character's glyph from the font's character map
//! alone. Where the font of a face has localized forms (`locl`) for the window's language, the
//! system's text draws those glyphs instead, found in the font's own tables as text in app fonts
//! finds them (`shaping::local`).
//!
//! Only lines of simple scripts change: every character in the face's own font, none a mark or
//! of a script shaped by syllable or right to left. Others are left to the system as they are.
use crate::font_file::FontFile;
use crate::scripts::Language;
use crate::shaping::Script;
use crate::text::Face;
use std::collections::HashMap;

/// A language's forms in the system's fonts, as looked up so far.
pub(crate) struct Forms {
    language: Language,
    /// Each face's font, where it has forms for the language; and whether it has some for
    /// Latin, which ASCII text may take.
    fonts: Vec<(Face, Option<FontFile>, bool)>,
    /// Each character's form looked up, by face: none where it has none, or the font lacks it.
    found: HashMap<(Face, char), Option<u16>>,
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
        Forms { language: Language::of_the_system(), fonts: Vec::new(), found: HashMap::new() }
    }

    /// Takes another language's forms from now on: whether that is a change.
    pub fn set(&mut self, language: Language) -> bool {
        if language == self.language {
            return false;
        }
        (self.language, self.fonts, self.found) = (language, Vec::new(), HashMap::new());
        true
    }

    /// A line's glyphs in a face's font where a character of it takes a form there: each
    /// character's own and the one drawn, one per UTF-16 unit, as GDI and FreeType count them.
    /// Nothing where none does, or the line is not all simple script in that font (see above).
    pub fn glyphs(&mut self, face: Face, text: &str) -> Option<Vec<[u16; 2]>> {
        if self.language == Language::NONE || (text.is_ascii() && !self.font(face).is_some_and(|(_, latin)| latin)) {
            return None;
        }
        let mut any = false;
        for letter in text.chars() {
            any |= self.form(face, letter)?.is_some();
        }
        let file = self.fonts.iter().find(|(known, ..)| *known == face).and_then(|(_, file, _)| file.as_ref()).filter(|_| any)?;
        let glyph = |letter: char| {
            let own = file.glyph(letter);
            [own, self.found.get(&(face, letter)).copied().flatten().unwrap_or(own)]
        };
        Some(text.chars().map(glyph).collect())
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
        if !self.fonts.iter().any(|(known, ..)| *known == face) {
            let file = crate::system_fonts::file_for(face, 'a').filter(|file| has_forms(file, self.language, None));
            let latin = file.as_ref().is_some_and(|file| has_forms(file, self.language, Some(b"latn")));
            self.fonts.push((face, file, latin));
        }
        let (_, file, latin) = self.fonts.iter().find(|(known, ..)| *known == face)?;
        Some((file.as_ref()?, *latin))
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

#[cfg(test)]
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
