//! The system's text on Android: its own fonts in `/system/fonts`, read and drawn by the
//! library ([`AppFonts`]), as Android has no text drawing for native code. Roboto for the
//! interface (bold and italic from its axes where it is variable), Droid Sans Mono for
//! monospace, and for characters Roboto lacks the fallback fonts `/system/etc/fonts.xml` lists,
//! in its order, each opened the first time it is needed.
// Elsewhere only its tests use it: the same frame drawn on Linux as on Android.
#![cfg_attr(not(target_os = "android"), allow(dead_code))]
use crate::canvas::Canvas;
use crate::color::Color;
use crate::font_file::{AppFonts, FontFile};
use crate::scripts::Language;
use crate::text::{Face, Font, Metrics, TextSystem};
use std::path::{Path, PathBuf};

/// Where the system's fonts are, and the lists of them.
const FONTS: &str = "/system/fonts";
const LISTS: [&str; 2] = ["/system/etc/font_fallback.xml", "/system/etc/fonts.xml"];

/// The system's text (see the module).
pub struct SystemText {
    fonts: AppFonts,
}

impl std::fmt::Debug for SystemText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemText").field("fonts", &self.fonts).finish()
    }
}

impl SystemText {
    /// The system's fonts, from `/system/fonts`. Nothing if Roboto is not there.
    pub fn new() -> Option<Self> {
        SystemText::in_folder(Path::new(FONTS), &LISTS.iter().find_map(|list| std::fs::read_to_string(list).ok()).unwrap_or_default())
    }

    /// The fonts in `folder`, falling back as `list` (a `fonts.xml`) says.
    pub(crate) fn in_folder(folder: &Path, list: &str) -> Option<Self> {
        let open = |name: &str| FontFile::open(folder.join(name));
        let name = ["Roboto-Regular.ttf", "RobotoStatic-Regular.ttf"].into_iter().find(|name| folder.join(name).exists())?;
        let regular = open(name)?;
        let axes = regular.axes();
        let has = |tag: &[u8; 4]| axes.iter().any(|axis| &axis.name == tag);
        // A variable Roboto (Android 12 and later) is bold and italic along its axes.
        let bold = if has(b"wght") { open(name).map(|file| file.with_variation(b"wght", 700.0)) } else { open("Roboto-Bold.ttf") };
        let italic = if has(b"ital") { open(name).map(|file| file.with_variation(b"ital", 1.0)) } else { open("Roboto-Italic.ttf") };
        let mono = open("DroidSansMono.ttf").or_else(|| open("CutiveMono.ttf"));
        let faces = [(Face::Bold, bold), (Face::Italic, italic), (Face::Mono, mono)];
        let fonts = faces.into_iter().fold(AppFonts::new(regular), |fonts, (face, file)| with_face(fonts, face, file));
        let fallbacks = Fallbacks { slots: fallback_files(list).into_iter().map(|(name, nth)| Slot::Untried(folder.join(name), nth)).collect() };
        Some(SystemText { fonts: fonts.fallback(Box::new(fallbacks)) })
    }
}

/// The fonts with a face's file, if there is one.
fn with_face(fonts: AppFonts, face: Face, file: Option<FontFile>) -> AppFonts {
    match (face, file) {
        (Face::Bold, Some(file)) => fonts.bold(file),
        (Face::Italic, Some(file)) => fonts.italic(file),
        (Face::Mono, Some(file)) => fonts.mono(file),
        _ => fonts,
    }
}

/// The fallback fonts of a `fonts.xml`, in its order: of each family with no name, its first
/// regular upright font (or its first), and its index in a collection.
pub(crate) fn fallback_files(list: &str) -> Vec<(String, usize)> {
    let mut files = Vec::new();
    for family in list.split("<family").skip(1) {
        let (opening, body) = family.split_once('>').unwrap_or((family, ""));
        if opening.contains("name=") || opening.trim_end().ends_with('/') {
            continue;
        }
        let body = body.split("</family>").next().unwrap_or_default();
        let fonts: Vec<&str> = body.split("<font").skip(1).filter(|font| font.starts_with([' ', '>', '\n', '\t'])).collect();
        let usual = fonts.iter().find(|font| attribute(font, "weight").is_none_or(|weight| weight == "400") && attribute(font, "style").is_none_or(|style| style == "normal"));
        let Some(font) = usual.or(fonts.first()) else { continue };
        let name = font.split_once('>').map_or("", |(_, rest)| rest.split('<').next().unwrap_or_default()).trim();
        let nth = attribute(font, "index").and_then(|nth| nth.parse().ok()).unwrap_or(0);
        if !name.is_empty() && !files.iter().any(|(known, at)| known == name && *at == nth) {
            files.push((name.to_string(), nth));
        }
    }
    files
}

/// An attribute's value in a tag's text: `weight="400"`.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let opening = tag.split('>').next()?;
    let at = opening.find(&format!("{name}=\""))? + name.len() + 2;
    opening[at..].split('"').next()
}

/// A fallback font: not yet opened, opened, or not to be had.
enum Slot {
    Untried(PathBuf, usize),
    Open(Box<AppFonts>),
    Gone,
}

/// The fallback fonts, each opened when first needed: what Roboto lacks.
struct Fallbacks {
    slots: Vec<Slot>,
}

impl Fallbacks {
    /// The first font that has `letter`, opening fonts until one does: its slot. The first that
    /// opens, if none has it (its missing-glyph box is drawn).
    fn slot_for(&mut self, letter: char) -> Option<usize> {
        let mut first = None;
        for at in 0..self.slots.len() {
            let Some(file) = self.opened(at) else { continue };
            first = first.or(Some(at));
            if file.glyph(letter) != 0 {
                return Some(at);
            }
        }
        first
    }

    /// The file of slot `at`, opened now if it was not.
    fn opened(&mut self, at: usize) -> Option<&FontFile> {
        if let Slot::Untried(path, nth) = &self.slots[at] {
            let file = FontFile::open(path).and_then(|file| if *nth == 0 { Some(file) } else { file.nth(*nth) });
            self.slots[at] = file.map_or(Slot::Gone, |file| Slot::Open(Box::new(AppFonts::new(file))));
        }
        match &mut self.slots[at] {
            Slot::Open(fonts) => fonts.file(Face::Regular, ' '),
            _ => None,
        }
    }

    /// The opened fonts of a slot.
    fn fonts(&mut self, at: usize) -> Option<&mut AppFonts> {
        match &mut self.slots[at] {
            Slot::Open(fonts) => Some(fonts),
            _ => None,
        }
    }

    /// `text` cut into runs each drawn by one font: byte range and slot.
    fn runs(&mut self, text: &str) -> Vec<(usize, usize, Option<usize>)> {
        let mut runs: Vec<(usize, usize, Option<usize>)> = Vec::new();
        for (at, letter) in text.char_indices() {
            // Marks and joiners stay with what they follow.
            let slot = match runs.last() {
                Some(last) if letter.is_whitespace() || is_joined(letter) => last.2,
                _ => self.slot_for(letter),
            };
            match runs.last_mut() {
                Some(last) if last.2 == slot => last.1 = at + letter.len_utf8(),
                _ => runs.push((at, at + letter.len_utf8(), slot)),
            }
        }
        runs
    }

    /// The first open font's metrics: where every run's baseline is put.
    fn first_metrics(&mut self, font: Font, scale: f32) -> Metrics {
        let first = (0..self.slots.len()).find(|at| self.opened(*at).is_some());
        first.and_then(|at| self.fonts(at)).map_or(Metrics { ascent: font.size * 0.93, descent: font.size * 0.24, cap_height: font.size * 0.71, x_height: font.size * 0.53 }, |fonts| {
            fonts.metrics(Font { face: Face::Regular, ..font }, scale)
        })
    }
}

/// A character drawn with the one before: a combining mark, a joiner or a variation selector.
fn is_joined(letter: char) -> bool {
    matches!(letter, '\u{300}'..='\u{36f}' | '\u{200c}' | '\u{200d}' | '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}')
}

impl TextSystem for Fallbacks {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.first_metrics(font, scale)
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, mut stops: Option<&mut Vec<f32>>) -> f32 {
        let font = Font { face: Face::Regular, ..font };
        let mut wide = 0.0;
        for (start, end, slot) in self.runs(text) {
            let from = stops.as_ref().map_or(0, |stops| stops.len());
            let piece = &text[start..end];
            let run = match slot.and_then(|at| self.fonts(at)) {
                Some(fonts) => fonts.measure(piece, font, scale, stops.as_deref_mut()),
                None => (stops.iter_mut().for_each(|stops| stops.extend(piece.chars().map(|_| 0.0))), 0.0).1,
            };
            stops.iter_mut().for_each(|stops| stops[from..].iter_mut().for_each(|stop| *stop += wide));
            wide += run;
        }
        wide
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let font = Font { face: Face::Regular, ..font };
        let (scale, ascent) = (canvas.scale(), self.first_metrics(font, canvas.scale()).ascent);
        let mut left = x;
        for (start, end, slot) in self.runs(text) {
            let Some(fonts) = slot.and_then(|at| self.fonts(at)) else { continue };
            let piece = &text[start..end];
            let lower = ascent - fonts.metrics(font, scale).ascent;
            fonts.draw(canvas, piece, left, y + lower, font, color);
            left += fonts.measure(piece, font, scale, None);
        }
    }

    fn file(&mut self, _face: Face, letter: char) -> Option<&FontFile> {
        let at = self.slot_for(letter)?;
        self.opened(at)
    }

    fn set_language(&mut self, language: Language) {
        for slot in &mut self.slots {
            if let Slot::Open(fonts) = slot {
                fonts.set_language(language);
            }
        }
    }
}

impl TextSystem for SystemText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.fonts.metrics(font, scale)
    }
    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        self.fonts.measure(text, font, scale, stops)
    }
    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        self.fonts.draw(canvas, text, x, y, font, color);
    }
    fn file(&mut self, face: Face, letter: char) -> Option<&FontFile> {
        self.fonts.file(face, letter)
    }
    fn set_language(&mut self, language: Language) {
        self.fonts.set_language(language);
    }
}

/// The first of the system's fonts (Roboto, then the fallbacks in order) that has `letter`.
pub(crate) fn file_for(letter: char) -> Option<FontFile> {
    let list = LISTS.iter().find_map(|list| std::fs::read_to_string(list).ok()).unwrap_or_default();
    let named = ["Roboto-Regular.ttf".to_string()].into_iter().map(|name| (name, 0));
    let opened = |(name, nth): (String, usize)| FontFile::open(Path::new(FONTS).join(name)).and_then(|file| if nth == 0 { Some(file) } else { file.nth(nth) });
    named.chain(fallback_files(&list)).filter_map(opened).find(|file| file.glyph(letter) != 0)
}

/// The system's colour emoji font.
pub(crate) fn emoji_file() -> Option<FontFile> {
    FontFile::open(Path::new(FONTS).join("NotoColorEmoji.ttf"))
}

/// The named families of `fonts.xml` and their styles (one family's, if asked): as
/// `system_fonts` lists them.
pub(crate) fn listed(family: Option<&str>) -> Vec<(String, crate::system_fonts::Style)> {
    let list = LISTS.iter().rev().find_map(|list| std::fs::read_to_string(list).ok()).unwrap_or_default();
    let mut found = Vec::new();
    for block in list.split("<family").skip(1) {
        let Some(name) = attribute(block, "name").filter(|name| family.is_none_or(|family| name.eq_ignore_ascii_case(family))) else { continue };
        let body = block.split("</family>").next().unwrap_or_default();
        for font in body.split("<font").skip(1).filter(|font| font.starts_with([' ', '>'])) {
            let weight = attribute(font, "weight").and_then(|weight| weight.parse().ok()).unwrap_or(400);
            found.push((name.to_string(), crate::system_fonts::Style { weight, italic: attribute(font, "style") == Some("italic") }));
        }
    }
    found
}

// The gallery example, drawn by the tests below as an emulator shows it.
#[cfg(test)]
#[allow(dead_code, unused_attributes, unused_imports, clippy::all)]
#[path = "../examples/gallery.rs"]
mod gallery;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Size;
    use crate::platform::LocalClipboard;
    use crate::preferences::Preferences;
    use crate::surface::Surface;
    use crate::ui::Ui;
    use std::process::Command;
    use std::time::Duration;

    const LIST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<familyset>
    <family name="sans-serif">
        <font weight="400" style="normal">Roboto-Regular.ttf
            <axis tag="wght" stylevalue="400" />
        </font>
        <font weight="700" style="normal">Roboto-Regular.ttf</font>
        <font weight="400" style="italic">Roboto-Regular.ttf</font>
    </family>
    <alias name="arial" to="sans-serif" />
    <family lang="und-Arab" variant="elegant">
        <font weight="400" style="normal" postScriptName="NotoNaskhArabic">NotoNaskhArabic-Regular.ttf</font>
        <font weight="700" style="normal">NotoNaskhArabic-Bold.ttf</font>
    </family>
    <family lang="zh-Hans">
        <font weight="400" style="normal" index="2">NotoSansCJK-Regular.ttc</font>
    </family>
    <family lang="ja">
        <font weight="400" style="normal" index="0">NotoSansCJK-Regular.ttc</font>
    </family>
    <family lang="zh-Hant">
        <font weight="400" style="normal" index="2">NotoSansCJK-Regular.ttc</font>
    </family>
    <family name="serif">
        <font weight="400" style="normal">NotoSerif-Regular.ttf</font>
    </family>
</familyset>"#;

    #[test]
    fn the_fallbacks_are_the_unnamed_families_in_order() {
        let found = fallback_files(LIST);
        assert_eq!(found, [("NotoNaskhArabic-Regular.ttf".to_string(), 0), ("NotoSansCJK-Regular.ttc".into(), 2), ("NotoSansCJK-Regular.ttc".into(), 0)]);
        assert_eq!(attribute(r#" weight="400" style="normal">x.ttf"#, "style"), Some("normal"));
        assert_eq!(attribute(r#">x.ttf weight="400""#, "weight"), None, "only the opening tag is read");
    }

    /// What the app said of its window (`android.rs` logs it as it is measured).
    #[derive(Debug, Default)]
    struct Said {
        size: [usize; 2],
        insets: [usize; 4],
        scale: f32,
        preferences: Preferences,
    }

    fn said(line: &str) -> Option<Said> {
        let words: Vec<&str> = line.split("window ").nth(1)?.split_whitespace().collect();
        let number = |at: usize| words.get(at).and_then(|word| word.parse::<f32>().ok());
        let flag = |at: usize| words.get(at) == Some(&"1");
        let accent = words.get(12).and_then(|word| u32::from_str_radix(word, 16).ok()).filter(|accent| *accent != 0);
        Some(Said {
            size: [number(0)? as usize, number(1)? as usize],
            insets: [number(3)? as usize, number(4)? as usize, number(5)? as usize, number(6)? as usize],
            scale: number(8)?,
            preferences: Preferences { dark: flag(10), accent: accent.map(Color), text_scale: number(14)?, reduced_motion: flag(16), ..Preferences::default() },
        })
    }

    #[test]
    fn what_the_window_says_is_read_back() {
        let line = "11-24 10:00:00.000  123  123 I taut    : window 1080 2400 insets 0 136 0 63 scale 2.625 dark 0 accent ff0061a4 text 1 still 0";
        let said = said(line).expect("a window line");
        assert_eq!((said.size, said.insets, said.scale), ([1080, 2400], [0, 136, 0, 63], 2.625));
        assert_eq!((said.preferences.dark, said.preferences.accent, said.preferences.text_scale), (false, Some(Color(0xff00_61a4)), 1.0));
    }

    /// `adb`, from the SDK or the path.
    fn adb(args: &[&str]) -> Vec<u8> {
        let tools = std::env::var_os("ANDROID_HOME").map(|home| std::path::PathBuf::from(home).join("platform-tools").join("adb"));
        let program = tools.filter(|path| path.exists()).unwrap_or_else(|| "adb".into());
        Command::new(program).args(args).output().map(|done| done.stdout).unwrap_or_default()
    }

    fn shell(command: &str) -> String {
        String::from_utf8_lossy(&adb(&["shell", command])).into_owned()
    }

    fn settle(seconds: u64) {
        std::thread::sleep(Duration::from_secs(seconds));
    }

    /// One node of a `uiautomator dump`.
    #[derive(Clone, Debug, Default)]
    struct Node {
        class: String,
        text: String,
        checked: bool,
        focused: bool,
        bounds: [i32; 4],
    }

    impl Node {
        fn middle(&self) -> (i32, i32) {
            ((self.bounds[0] + self.bounds[2]) / 2, (self.bounds[1] + self.bounds[3]) / 2)
        }
    }

    /// The window's nodes, as `uiautomator` reads them (through the library's provider); the
    /// dump is kept in `out`.
    fn dump(out: &Path, name: &str) -> Vec<Node> {
        shell("uiautomator dump /sdcard/taut-ui.xml");
        let xml = String::from_utf8_lossy(&adb(&["exec-out", "cat", "/sdcard/taut-ui.xml"])).into_owned();
        let _ = std::fs::write(out.join(format!("{name}.xml")), &xml);
        xml.split("<node ").skip(1).map(node_of).collect()
    }

    fn node_of(tag: &str) -> Node {
        let value = |name: &str| attribute(tag, name).unwrap_or_default().replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
        let corners: Vec<i32> = value("bounds").split(|c: char| !c.is_ascii_digit() && c != '-').filter_map(|part| part.parse().ok()).collect();
        Node { class: value("class"), text: value("text"), checked: value("checked") == "true", focused: value("focused") == "true", bounds: corners.try_into().unwrap_or_default() }
    }

    fn find<'a>(nodes: &'a [Node], class: &str, text: &str) -> Option<&'a Node> {
        nodes.iter().find(|node| node.class == class && node.text.starts_with(text))
    }

    fn tap(node: &Node) {
        let (x, y) = node.middle();
        shell(&format!("input tap {x} {y}"));
        settle(2);
    }

    /// The last window line the app logged.
    fn window_said() -> Option<Said> {
        let log = String::from_utf8_lossy(&adb(&["logcat", "-d", "-s", "taut:I"])).into_owned();
        log.lines().rev().find_map(said)
    }

    /// The screen, as a picture: width and pixels.
    fn screen(out: &Path, name: &str) -> (usize, Vec<u32>) {
        let png = adb(&["exec-out", "screencap", "-p"]);
        let _ = std::fs::write(out.join(format!("{name}.png")), &png);
        crate::png::decode(&png).unwrap_or_default()
    }

    /// The gallery as the app drew it, the same size and scale, with the emulator's own fonts
    /// pulled into `out` and its settings: the pixels inside the insets, and how many differ.
    fn linux_frame(out: &Path, said: &Said, (width, shown): &(usize, Vec<u32>)) -> usize {
        // Each file copied by itself: `adb pull` of the folder once left none.
        let fonts = out.join("fonts");
        std::fs::create_dir_all(&fonts).unwrap();
        let names = shell("ls /system/fonts");
        for name in names.split_whitespace().filter(|name| name.ends_with(".ttf") || name.ends_with(".ttc") || name.ends_with(".otf")) {
            std::fs::write(fonts.join(name), adb(&["exec-out", "cat", &format!("/system/fonts/{name}")])).unwrap();
        }
        eprintln!("fonts copied: {}", std::fs::read_dir(&fonts).map_or(0, |files| files.count()));
        for name in ["Roboto-Regular.ttf", "RobotoStatic-Regular.ttf", "DroidSansMono.ttf"] {
            let bytes = std::fs::read(fonts.join(name)).unwrap_or_default();
            eprintln!("{name}: {} bytes, starts {:02x?}, read {}", bytes.len(), &bytes[..bytes.len().min(8)], FontFile::read(bytes.clone()).is_some());
        }
        let list = ["/system/etc/font_fallback.xml", "/system/etc/fonts.xml"].iter().find_map(|list| Some(adb(&["exec-out", "cat", list])).filter(|text| text.starts_with(b"<?xml")));
        let text = SystemText::in_folder(&fonts, &String::from_utf8_lossy(&list.unwrap_or_default())).expect("the emulator's Roboto");
        let mut ui = Ui::new(Box::new(text), Box::new(LocalClipboard::default()));
        let inner = [said.size[0] - said.insets[0] - said.insets[2], said.size[1] - said.insets[1] - said.insets[3]];
        ui.set_preferences(said.preferences);
        ui.resize(Size { w: inner[0] as f32 / said.scale, h: inner[1] as f32 / said.scale }, said.scale);
        let mut app = super::gallery::app();
        (0..3).for_each(|_| _ = ui.run(&mut |ui: &mut Ui| super::gallery::describe(&mut app, ui)));
        let drawn = Surface::draw(&mut ui, 64);
        let _ = std::fs::write(out.join("linux-gallery.png"), drawn.to_png());
        let mut cut = Vec::with_capacity(drawn.width * drawn.height);
        for y in 0..drawn.height {
            let row = (y + said.insets[1]) * width + said.insets[0];
            cut.extend(shown.get(row..row + drawn.width).unwrap_or_default());
        }
        let _ = std::fs::write(out.join("android-gallery-inside.png"), crate::png::encode(drawn.width, &cut).unwrap_or_default());
        let differ = drawn.pixels.iter().zip(&cut).filter(|(a, b)| (**a ^ **b) & 0xff_ffff != 0).count();
        differ + drawn.pixels.len().abs_diff(cut.len())
    }

    /// The gallery on an Android emulator (`TAUT_ANDROID_APK` names its APK, made by `cargo run
    /// --example apk`; `adb` reaches the emulator): its first page is the frame Linux draws with
    /// the same fonts, size and scale; a tap ticks the tick box; keys type into a text box and
    /// move focus; a tap on a tab shows its page; it turns with the screen, comes back as it was after
    /// the home screen, and closes with the back button. Readers' nodes are read with
    /// `uiautomator dump`. What was seen is kept in `TAUT_PICTURES` (or a temporary folder).
    #[test]
    fn the_gallery_on_an_android_emulator() {
        let Some(apk) = std::env::var_os("TAUT_ANDROID_APK") else { return eprintln!("no TAUT_ANDROID_APK") };
        let out = std::env::var_os("TAUT_PICTURES").map_or_else(|| std::env::temp_dir().join("taut-android"), std::path::PathBuf::from);
        std::fs::create_dir_all(&out).unwrap();
        let mut wrong: Vec<String> = Vec::new();
        adb(&["install", "-r", &apk.to_string_lossy()]);
        adb(&["logcat", "-c"]);
        // A slow emulator's "isn't responding" dialogs would cover the app.
        shell("settings put global hide_error_dialogs 1; settings put system accelerometer_rotation 0; settings put system user_rotation 0");
        shell("am start -W -n taut.example.gallery/android.app.NativeActivity");
        settle(6);
        for _ in 0..20 {
            if dump(&out, "first").iter().any(|node| node.class == "android.widget.CheckBox") {
                break;
            }
            shell("am broadcast -a android.intent.action.CLOSE_SYSTEM_DIALOGS");
            settle(3);
        }
        let said = window_said().expect("the app logs its window");
        eprintln!("window: {said:?}");
        // The first page, as Linux draws it.
        let shown = screen(&out, "android-gallery");
        assert_eq!((shown.0, shown.1.len() / shown.0.max(1)), (said.size[0], said.size[1]), "the screen is the window");
        let differ = linux_frame(&out, &said, &shown);
        eprintln!("pixels that differ from the Linux frame: {differ}");
        if differ != 0 {
            wrong.push(format!("{differ} pixels differ from the Linux frame"));
        }
        // What readers are told.
        let nodes = dump(&out, "first");
        let tick = find(&nodes, "android.widget.CheckBox", "A tick box").cloned().expect("the tick box is told");
        assert!(!tick.checked);
        // (At phone width the top bar's buttons give way: the filter box and the tabs stay.)
        for (class, text) in [("android.widget.TextView", "Gallery: 60 items"), ("android.widget.EditText", ""), ("android.widget.Button", "List"), ("android.widget.TabWidget", "")] {
            if find(&nodes, class, text).is_none() {
                wrong.push(format!("no {class} \"{text}\" told"));
            }
        }
        // A tap ticks the box.
        tap(&tick);
        let nodes = dump(&out, "ticked");
        if !find(&nodes, "android.widget.CheckBox", "A tick box").is_some_and(|node| node.checked) {
            wrong.push("the tap did not tick the box".into());
        }
        // Keys type into the filter box, and Backspace takes one back; Tab moves focus.
        let filter = nodes.iter().filter(|node| node.class == "android.widget.EditText").min_by_key(|node| node.bounds[1]).cloned().expect("the filter box");
        tap(&filter);
        shell("input text item1");
        settle(2);
        shell("input keyevent KEYCODE_DEL");
        settle(2);
        let nodes = dump(&out, "typed");
        if !nodes.iter().any(|node| node.class == "android.widget.EditText" && node.text == "item") {
            wrong.push("typing \"item1\" and Backspace did not leave \"item\"".into());
        }
        // (The library's own view has Android's focus throughout: the parts are its virtual views.)
        let told_focus = |nodes: &[Node]| nodes.iter().find(|node| node.focused && node.class != "android.view.View").map(|node| (node.class.clone(), node.text.clone()));
        let focused = told_focus(&nodes);
        shell("input keyevent KEYCODE_TAB");
        settle(2);
        let nodes = dump(&out, "tabbed");
        let now = told_focus(&nodes);
        if now == focused {
            wrong.push(format!("Tab left focus on {now:?}"));
        }
        // A tap on a tab shows its page.
        let parts = find(&nodes, "android.widget.Button", "Parts").cloned().expect("the tab Parts");
        tap(&parts);
        let nodes = dump(&out, "parts");
        if find(&nodes, "android.widget.Switch", "Light colours").is_none() {
            wrong.push("the tap on the tab Parts did not show its page".into());
        }
        let _ = screen(&out, "parts");
        // The screen turns: the window is told its new size.
        shell("settings put system user_rotation 1");
        settle(5);
        let turned = window_said().expect("the window after turning");
        let picture = screen(&out, "turned");
        if turned.size[0] <= turned.size[1] || picture.0 != turned.size[0] {
            wrong.push(format!("after turning the window is {:?}", turned.size));
        }
        let _ = dump(&out, "turned");
        shell("settings put system user_rotation 0");
        settle(5);
        // Away to the home screen and back: the interface is as it was.
        shell("input keyevent KEYCODE_HOME");
        settle(3);
        let away = shell("dumpsys activity activities");
        if away.lines().any(|line| line.contains("mResumedActivity") && line.contains("taut.example.gallery")) {
            wrong.push("the gallery stayed in front of the home screen".into());
        }
        shell("am start -W -n taut.example.gallery/android.app.NativeActivity");
        settle(4);
        let nodes = dump(&out, "back");
        if find(&nodes, "android.widget.Switch", "Light colours").is_none() || !nodes.iter().any(|node| node.class == "android.widget.EditText" && node.text == "item") {
            wrong.push("the interface was not as it was after coming back".into());
        }
        let _ = screen(&out, "back");
        // The back button: put away the keyboard if it shows, then close the app.
        shell("input keyevent KEYCODE_BACK");
        settle(2);
        shell("input keyevent KEYCODE_BACK");
        settle(3);
        let left = shell("dumpsys activity activities");
        if left.lines().any(|line| line.contains("mResumedActivity") && line.contains("taut.example.gallery")) {
            wrong.push("the back button did not close the gallery".into());
        }
        let log = String::from_utf8_lossy(&adb(&["logcat", "-d"])).into_owned();
        let _ = std::fs::write(out.join("logcat.txt"), &log);
        assert!(!log.contains("FATAL EXCEPTION") && !log.contains("Fatal signal"), "the app crashed: see logcat.txt");
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }
}
