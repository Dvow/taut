//! SSA and ASS (SubStation Alpha) captions: a whole script an app gives, or a player's events
//! with the script's header (as Matroska keeps them). Each caption is placed as the script says,
//! by its style's alignment and margins or by `\pos`, and is italic by its style or `\i1`.
//! Fonts, colours, borders, motion and karaoke are not kept.

use crate::layout::Align;
use crate::video_captions::{Caption, CaptionsError, Cue, Down, Place, time_of};

/// A script's header: its size (the units of margins and `\pos`), its styles, and whether `\n`
/// breaks lines (`WrapStyle: 2`).
#[derive(Debug)]
pub(crate) struct Script {
    width: f32,
    height: f32,
    styles: Vec<Style>,
    breaks_at_n: bool,
}

/// A style: its name, alignment (as on a keypad), margins (left, right, top or bottom) and
/// whether it is italic.
#[derive(Clone, Debug)]
struct Style {
    name: String,
    keypad: u8,
    margins: [f32; 3],
    italic: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style { name: "Default".into(), keypad: 2, margins: [10.0; 3], italic: false }
    }
}

/// A script's sections, as their headings name them.
#[derive(Clone, Copy, PartialEq)]
enum Section {
    Info,
    Styles,
    LegacyStyles,
    Events,
    Other,
}

impl Section {
    fn of(heading: &str) -> Section {
        match heading.to_ascii_lowercase().as_str() {
            "[script info]" => Section::Info,
            "[v4+ styles]" => Section::Styles,
            "[v4 styles]" => Section::LegacyStyles,
            "[events]" => Section::Events,
            _ => Section::Other,
        }
    }
}

/// The cues of a whole script, by their start: its `Dialogue` lines.
pub(crate) fn read(text: &str) -> Result<Vec<Cue>, CaptionsError> {
    let script = Script::read(text);
    let mut cues = Vec::new();
    for (number, (format, line)) in events(text) {
        let field = |name: &str| format.iter().position(|field| field.eq_ignore_ascii_case(name)).and_then(|at| line.get(at)).map(|value| value.trim());
        let times = (field("Start").and_then(time_of), field("End").and_then(time_of));
        let (Some(from), Some(to)) = times else { return Err(CaptionsError::BadTime(number + 1)) };
        let margins = ["MarginL", "MarginR", "MarginV"].map(|name| field(name).and_then(|value| value.parse().ok()));
        let text = line.last().copied().filter(|_| line.len() == format.len()).unwrap_or_default();
        cues.push(Cue { from, to, caption: script.caption(field("Style").unwrap_or_default(), margins, text) });
    }
    Ok(cues)
}

/// Each `Dialogue` line of the events, by its line's number (from 0): the events' format and
/// the line's fields (its text last, commas and all).
fn events(text: &str) -> impl Iterator<Item = (usize, (Vec<&str>, Vec<&str>))> {
    let (mut section, mut format) = (Section::Other, Vec::new());
    text.lines().enumerate().filter_map(move |(number, line)| {
        let line = line.trim_end_matches('\r');
        if line.starts_with('[') {
            section = Section::of(line.trim());
            return None;
        }
        let (kind, rest) = line.split_once(':').filter(|_| section == Section::Events)?;
        match kind.trim() {
            "Format" => format = rest.split(',').map(str::trim).collect(),
            "Dialogue" if !format.is_empty() => return Some((number, (format.clone(), rest.trim_start().splitn(format.len(), ',').collect()))),
            _ => {}
        }
        None
    })
}

/// A player's event as Matroska keeps it (`ReadOrder, Layer, Style, Name, MarginL, MarginR,
/// MarginV, Effect, Text`) as a caption of the script with this header.
#[cfg(any(test, all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
pub(crate) fn event_caption(script: &Script, event: &str) -> Caption {
    let fields: Vec<&str> = event.splitn(9, ',').collect();
    let [_, _, style, _, left, right, vertical, _, text] = fields[..] else { return script.caption("", [None; 3], event) };
    let margins = [left, right, vertical].map(|margin| margin.trim().parse().ok());
    script.caption(style.trim(), margins, text)
}

impl Script {
    /// The header of a script: its size and styles. Without a size, 384 by 288 (or 4:3 to the
    /// side it gives).
    pub(crate) fn read(header: &str) -> Script {
        let (mut section, mut format, mut styles) = (Section::Other, Vec::new(), Vec::new());
        let (mut width, mut height, mut breaks_at_n) = (None, None, false);
        for line in header.lines().map(|line| line.trim_end_matches('\r')) {
            if line.starts_with('[') {
                section = Section::of(line.trim());
                continue;
            }
            let Some((name, value)) = line.split_once(':').map(|(name, value)| (name.trim(), value.trim())) else { continue };
            match (section, name) {
                (Section::Info, "PlayResX") => width = value.parse::<f32>().ok().filter(|side| *side > 0.0),
                (Section::Info, "PlayResY") => height = value.parse::<f32>().ok().filter(|side| *side > 0.0),
                (Section::Info, "WrapStyle") => breaks_at_n = value == "2",
                (Section::Styles | Section::LegacyStyles, "Format") => format = value.split(',').map(str::trim).collect(),
                (Section::Styles | Section::LegacyStyles, "Style") => styles.push(style_of(&format, value, section == Section::LegacyStyles)),
                _ => {}
            }
        }
        let (width, height) = match (width, height) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, width * 3.0 / 4.0),
            (None, Some(height)) => (height * 4.0 / 3.0, height),
            (None, None) => (384.0, 288.0),
        };
        Script { width, height, styles, breaks_at_n }
    }

    /// A caption of style `style`, with an event's own margins (0 or none: the style's), of
    /// event text `text`.
    fn caption(&self, style: &str, margins: [Option<f32>; 3], text: &str) -> Caption {
        let style = self.styles.iter().find(|known| known.name == style.trim_start_matches('*')).cloned().unwrap_or_default();
        let margins: [f32; 3] = std::array::from_fn(|at| margins[at].filter(|margin| *margin != 0.0).unwrap_or(style.margins[at]));
        let tags = Tags::of(text, style.keypad);
        let place = match tags.at {
            Some((x, y)) => Some(self.placed_at(tags.keypad, x, y)),
            None if tags.keypad == 2 => None,
            None => Some(Place::keypad(tags.keypad, margins[0] / self.width, margins[1] / self.width, margins[2] / self.height)),
        };
        Caption { text: self.shown_text(text), place, italic: tags.italic.unwrap_or(style.italic) }
    }

    /// The place of `\pos(x,y)`: the point of the box the alignment names lies there.
    fn placed_at(&self, keypad: u8, x: f32, y: f32) -> Place {
        let column = [Align::Start, Align::Center, Align::End][usize::from((keypad.clamp(1, 9) - 1) % 3)];
        let row = [Align::End, Align::Center, Align::Start][usize::from((keypad.clamp(1, 9) - 1) / 3)];
        let (x, y) = ((x / self.width).clamp(0.0, 1.0), (y / self.height).clamp(0.0, 1.0));
        let width = match column {
            Align::Start => 1.0 - x,
            Align::End => x,
            _ => 2.0 * x.min(1.0 - x),
        };
        Place { x, across: column, down: Down::At(y, row), width, lines: column }
    }

    /// An event's text as shown: override blocks (`{...}`) dropped, `\N` (and `\n` where it
    /// breaks lines) a new line, `\h` a space that does not break.
    fn shown_text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find(['{', '\\']) {
            out.push_str(&rest[..at]);
            rest = &rest[at..];
            let (shown, length) = match rest.as_bytes() {
                [b'{', ..] => ("", rest.find('}').map_or(rest.len(), |end| end + 1)),
                [b'\\', b'N', ..] => ("\n", 2),
                [b'\\', b'n', ..] => (if self.breaks_at_n { "\n" } else { " " }, 2),
                [b'\\', b'h', ..] => ("\u{A0}", 2),
                _ => ("\\", 1),
            };
            out.push_str(shown);
            rest = &rest[length..];
        }
        out.push_str(rest);
        out
    }
}

/// A style line, by the styles' format: SSA's alignments (1 to 3 at the bottom, 4 more at the
/// top, 8 more in the middle) made a keypad's.
fn style_of(format: &[&str], line: &str, legacy: bool) -> Style {
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let field = |name: &str| format.iter().position(|field| field.eq_ignore_ascii_case(name)).and_then(|at| fields.get(at).copied());
    let number = |name: &str| field(name).and_then(|value| value.parse::<f32>().ok());
    let usual = Style::default();
    let alignment = field("Alignment").and_then(|value| value.parse::<u8>().ok()).unwrap_or(2);
    let keypad = if legacy { legacy_keypad(alignment) } else { alignment.clamp(1, 9) };
    let margins = [number("MarginL"), number("MarginR"), number("MarginV")];
    Style {
        name: field("Name").unwrap_or_default().trim_start_matches('*').to_owned(),
        keypad,
        margins: std::array::from_fn(|at| margins[at].unwrap_or(usual.margins[at])),
        italic: field("Italic").is_some_and(|value| value != "0"),
    }
}

/// SSA's alignment (`\a`, v4 styles) as a keypad's.
fn legacy_keypad(alignment: u8) -> u8 {
    let column = (alignment & 3).clamp(1, 3);
    match alignment {
        _ if alignment & 8 != 0 => column + 3,
        _ if alignment & 4 != 0 => column + 6,
        _ => column,
    }
}

/// What an event's override tags change: its alignment, `\pos` (or where `\move` starts) and
/// italics; the first of each counts, as renderers take them for the whole line.
#[derive(Debug, PartialEq)]
struct Tags {
    keypad: u8,
    at: Option<(f32, f32)>,
    italic: Option<bool>,
}

impl Tags {
    fn of(text: &str, keypad: u8) -> Tags {
        let mut tags = Tags { keypad, at: None, italic: None };
        let (mut aligned, mut rest) = (false, text);
        while let Some((block, after)) = rest.split_once('{').and_then(|(_, after)| after.split_once('}')) {
            for tag in block.split('\\').skip(1) {
                aligned |= tags.take(tag, aligned);
            }
            rest = after;
        }
        tags
    }

    /// Takes one tag (the text after its backslash): whether it set the alignment.
    fn take(&mut self, tag: &str, aligned: bool) -> bool {
        let number = |name: &str| tag.strip_prefix(name).and_then(|value| value.trim().parse::<u8>().ok());
        if let Some(keypad) = number("an").filter(|keypad| (1..=9).contains(keypad) && !aligned) {
            self.keypad = keypad;
            return true;
        }
        if let Some(alignment) = number("a").filter(|_| !aligned) {
            self.keypad = legacy_keypad(alignment);
            return true;
        }
        if let Some(on) = number("i").filter(|_| self.italic.is_none()) {
            self.italic = Some(on != 0);
        }
        let point = tag.strip_prefix("pos(").or_else(|| tag.strip_prefix("move(")).and_then(|values| point_of(values.trim_end_matches(')')));
        if self.at.is_none() {
            self.at = point;
        }
        false
    }
}

/// The first two numbers of `x,y,...`.
fn point_of(values: &str) -> Option<(f32, f32)> {
    let mut numbers = values.split(',').map(|value| value.trim().parse::<f32>().ok().filter(|number| number.is_finite()));
    Some((numbers.next()??, numbers.next()??))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const HEADER: &str = "[Script Info]\r\nScriptType: v4.00+\r\nPlayResX: 640\r\nPlayResY: 480\r\n\r\n[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\r\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\r\nStyle: Top,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,-1,0,0,100,100,0,0,1,2,2,8,64,64,48,1\r\n\r\n[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n";

    fn script(events: &str) -> Vec<Cue> {
        read(&format!("{HEADER}{events}")).unwrap()
    }

    fn near(one: f32, other: f32) -> bool {
        (one - other).abs() < 1e-4
    }

    #[test]
    fn dialogue_lines_are_cues_with_their_text_and_times() {
        let cues = script("Comment: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,not shown\r\nDialogue: 0,0:00:01.50,0:00:04.00,Default,,0,0,0,,Hello, there\\Nsecond line\r\n");
        assert_eq!(cues.len(), 1);
        assert_eq!((cues[0].from, cues[0].to), (Duration::from_millis(1500), Duration::from_secs(4)));
        assert_eq!(cues[0].caption, Caption::plain("Hello, there\nsecond line"));
    }

    #[test]
    fn a_style_places_its_captions_inside_its_margins_and_may_make_them_italic() {
        let cues = script("Dialogue: 0,0:00:01.00,0:00:02.00,Top,,0,0,0,,Title\r\n");
        let place = cues[0].caption.place.unwrap();
        assert!(cues[0].caption.italic);
        assert!(near(place.x, 0.5) && place.across == Align::Center && near(place.width, 0.8), "{place:?}");
        assert_eq!(place.down, Down::At(0.1, Align::Start));
        // (The event's own margin over the style's.)
        let cues = script("Dialogue: 0,0:00:01.00,0:00:02.00,Top,,0,0,96,,Lower\r\n");
        assert_eq!(cues[0].caption.place.unwrap().down, Down::At(0.2, Align::Start));
    }

    #[test]
    fn pos_puts_the_point_its_alignment_names_where_it_says() {
        let cues = script("Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\an7\\pos(160,120)\\i1}Placed\r\n");
        let caption = &cues[0].caption;
        let place = caption.place.unwrap();
        assert!(caption.italic && caption.text == "Placed");
        assert!(near(place.x, 0.25) && place.across == Align::Start && near(place.width, 0.75), "{place:?}");
        assert_eq!(place.down, Down::At(0.25, Align::Start));
        // (The first alignment counts; \move starts where its first point is.)
        let cues = script("Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\an3}{\\an9\\move(320,240,0,0)}Moving\r\n");
        let place = cues[0].caption.place.unwrap();
        assert_eq!((place.across, place.down), (Align::End, Down::At(0.5, Align::End)));
    }

    #[test]
    fn the_usual_alignment_keeps_the_usual_place() {
        assert_eq!(script("Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Low\r\n")[0].caption.place, None);
    }

    #[test]
    fn legacy_ssa_alignments_and_tags_are_read() {
        assert_eq!([1, 2, 3, 5, 6, 7, 9, 10, 11].map(legacy_keypad), [1, 2, 3, 7, 8, 9, 4, 5, 6]);
        let text = "[Script Info]\nScriptType: v4.00\n\n[V4 Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, TertiaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding\nStyle: Default,Arial,20,16777215,65535,65535,0,0,0,1,2,2,6,30,30,12,0,0\n\n[Events]\nFormat: Marked, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: Marked=0,0:00:01.00,0:00:02.00,Default,,0000,0000,0000,,Up top\nDialogue: Marked=0,0:00:03.00,0:00:04.00,Default,,0000,0000,0000,,{\\a9}Middle left\n";
        let cues = read(text).unwrap();
        let top = cues[0].caption.place.unwrap();
        // (Size 384 by 288 where none is given.)
        assert!(near(top.x, 0.5) && top.down == Down::At(12.0 / 288.0, Align::Start), "{top:?}");
        assert_eq!(cues[1].caption.place.unwrap().down, Down::At(0.5, Align::Center));
        assert!(near(cues[1].caption.place.unwrap().x, 30.0 / 384.0));
    }

    #[test]
    fn a_matroska_event_is_read_with_its_scripts_header() {
        let header = Script::read(HEADER);
        let caption = event_caption(&header, "3,0,Top,Ann,0,0,0,,Said\\hthis{\\i0}");
        assert_eq!((caption.text.as_str(), caption.italic), ("Said\u{A0}this", false));
        assert_eq!(caption.place.unwrap().down, Down::At(0.1, Align::Start));
        assert_eq!(event_caption(&header, "only text").text, "only text");
    }

    #[test]
    fn bad_times_and_scripts_without_events_are_refused() {
        assert_eq!(read(&format!("{HEADER}Dialogue: 0,0:00:xx.00,0:00:02.00,Default,,0,0,0,,Bad\r\n")), Err(CaptionsError::BadTime(13)));
        assert_eq!(crate::video_captions::read(HEADER), Err(CaptionsError::NoCues));
    }
}
