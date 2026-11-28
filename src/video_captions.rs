//! Captions read by taut: an app's WebVTT, SRT or SSA text made into cues, the text on show over
//! time kept as cues (for the decoders of captions in a video stream), and the captions on show
//! at a time (also for cues a system's player hands over). Each caption keeps its text (tags
//! dropped, entities read, its lines kept), where its source puts it on the frame, and whether
//! it is italic; colours and fonts are not kept.

use crate::layout::Align;
use std::time::Duration;

/// A line of captions as a part of the frame's height: a 15th, as CEA-608 and CEA-708 count
/// rows (and as the usual caption size, an 18th, is with its line gap).
pub(crate) const LINE: f32 = 1.0 / 15.0;

/// Where a caption sits on the frame, as its source says. Places are parts of the frame's width
/// and height from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Place {
    /// A place across the frame, and which side of the caption's box lies at it.
    pub x: f32,
    pub across: Align,
    pub down: Down,
    /// The widest its box may be.
    pub width: f32,
    /// How its lines line up in its box.
    pub lines: Align,
}

/// Where a caption sits down the frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Down {
    /// A place down the frame, and which side of the caption's box lies at it.
    At(f32, Align),
    /// Lines from the top (0: the first) or, below 0, from the bottom (-1: the last), as
    /// WebVTT counts them.
    Line(i32),
}

impl Place {
    /// SSA's alignment `keypad` (1 to 9 as on a keypad: 1 bottom left, 5 the middle, 9 top
    /// right) inside margins: left, right, and top or bottom, each at most half the frame (as
    /// parts of it).
    pub(crate) fn keypad(keypad: u8, left: f32, right: f32, vertical: f32) -> Place {
        let [left, right, vertical] = [left, right, vertical].map(|margin| if margin.is_finite() { margin.clamp(0.0, 0.5) } else { 0.0 });
        let row = (keypad.clamp(1, 9) - 1) / 3;
        let (x, across) = match (keypad.clamp(1, 9) - 1) % 3 {
            0 => (left, Align::Start),
            1 => ((left + 1.0 - right) / 2.0, Align::Center),
            _ => (1.0 - right, Align::End),
        };
        let down = match row {
            0 => Down::At(1.0 - vertical, Align::End),
            1 => Down::At(0.5, Align::Center),
            _ => Down::At(vertical, Align::Start),
        };
        Place { x, across, down, width: (1.0 - left - right).max(0.0), lines: across }
    }
}

/// A caption: its text, where its source puts it (none: the usual place, the bottom middle),
/// and whether it is italic throughout.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Caption {
    pub text: String,
    pub place: Option<Place>,
    pub italic: bool,
}

impl Caption {
    /// Text at the usual place, upright.
    #[cfg(any(test, windows, target_vendor = "apple", target_os = "android"))]
    pub(crate) fn plain(text: impl Into<String>) -> Caption {
        Caption { text: text.into(), ..Caption::default() }
    }
}

/// A caption and its time span.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cue {
    pub from: Duration,
    pub to: Duration,
    pub caption: Caption,
}

/// Why captions were not read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CaptionsError {
    /// No cue in it: not WebVTT, SRT or SSA, or empty.
    NoCues,
    /// A cue's times, on this line (from 1), could not be read.
    BadTime(usize),
}

impl std::fmt::Display for CaptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            CaptionsError::NoCues => write!(f, "no captions in it (WebVTT, SRT or SSA)"),
            CaptionsError::BadTime(line) => write!(f, "a caption's times on line {line} can't be read"),
        }
    }
}

impl std::error::Error for CaptionsError {}

/// The cues of WebVTT, SRT or SSA text, by their start.
pub(crate) fn read(text: &str) -> Result<Vec<Cue>, CaptionsError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut cues = match text.trim_start().starts_with("[Script Info]") {
        true => crate::video_ssa::read(text)?,
        false => read_timed_blocks(text)?,
    };
    if cues.is_empty() {
        return Err(CaptionsError::NoCues);
    }
    cues.sort_by_key(|cue| cue.from);
    Ok(cues)
}

/// The cues of WebVTT or SRT text. Each block with a `-->` line is a cue: the lines after it are
/// its text, the settings after its times its place; WebVTT's `REGION` blocks name places for
/// cues. Anything else (WebVTT's header, notes, styles; SRT's numbers) is skipped.
fn read_timed_blocks(text: &str) -> Result<Vec<Cue>, CaptionsError> {
    let lines: Vec<(usize, &str)> = text.lines().map(|line| line.trim_end_matches('\r')).enumerate().collect();
    let (mut cues, mut regions) = (Vec::new(), Vec::new());
    for block in lines.split(|(_, line)| line.trim().is_empty()) {
        let Some(timed) = block.iter().position(|(_, line)| line.contains("-->")) else {
            regions.extend(region_of(block));
            continue;
        };
        let (number, times) = block[timed];
        let (from, to) = times_of(times).ok_or(CaptionsError::BadTime(number + 1))?;
        let settings = times.split_once("-->").map_or("", |(_, rest)| rest.trim_start().split_once(char::is_whitespace).map_or("", |(_, settings)| settings));
        let text: Vec<&str> = block[timed + 1..].iter().map(|(_, line)| *line).collect();
        cues.push(Cue { from, to, caption: placed_caption(&text.join("\n"), settings, &regions) });
    }
    Ok(cues)
}

/// A timing line's start and end: `00:01.000 --> 00:02.500 align:start` (WebVTT) or
/// `00:00:01,000 --> 00:00:02,500` (SRT).
fn times_of(line: &str) -> Option<(Duration, Duration)> {
    let (from, rest) = line.split_once("-->")?;
    let to = rest.split_whitespace().next()?;
    Some((time_of(from.trim())?, time_of(to)?))
}

/// `hh:mm:ss.ttt`, `mm:ss.ttt`, with `.` or `,` before the thousandths.
pub(crate) fn time_of(text: &str) -> Option<Duration> {
    let (whole, thousandths) = text.split_once(['.', ',']).unwrap_or((text, "0"));
    let mut seconds = 0u64;
    for part in whole.split(':') {
        seconds = seconds.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    let millis: u64 = thousandths.get(..3.min(thousandths.len()))?.parse().ok()?;
    let millis = millis * 10u64.pow(3 - thousandths.len().min(3) as u32);
    Some(Duration::from_millis(seconds.checked_mul(1000)?.checked_add(millis)?))
}

/// A WebVTT region: its name and the box it is, as parts of the frame.
#[derive(Clone, Debug, PartialEq)]
struct Region {
    id: String,
    left: f32,
    /// Where its last line ends.
    bottom: f32,
    width: f32,
}

/// The region a `REGION` block defines: `id`, `width`, `lines`, `regionanchor` (the point of
/// the region, in parts of its width and height) and `viewportanchor` (where that point lies
/// on the frame). Its lines scroll up: cues sit at its bottom.
fn region_of(block: &[(usize, &str)]) -> Option<Region> {
    let (first, rest) = block.split_first()?;
    if first.1.trim() != "REGION" {
        return None;
    }
    let (mut id, mut width, mut lines, mut anchor, mut at) = (String::new(), 1.0, 3.0, (0.0, 1.0), (0.0, 1.0));
    for (name, value) in rest.iter().flat_map(|(_, line)| line.split_whitespace()).filter_map(|setting| setting.split_once(':')) {
        match name {
            "id" => id = value.to_owned(),
            "width" => width = percent(value).unwrap_or(width),
            "lines" => lines = value.parse::<u8>().map_or(lines, f32::from),
            "regionanchor" => anchor = percents(value).unwrap_or(anchor),
            "viewportanchor" => at = percents(value).unwrap_or(at),
            _ => {}
        }
    }
    let height = lines * LINE;
    let (left, top) = (at.0 - anchor.0 * width, at.1 - anchor.1 * height);
    Some(Region { id, left, bottom: top + height, width })
}

/// `40%` as 0.4.
fn percent(text: &str) -> Option<f32> {
    let number: f32 = text.strip_suffix('%')?.parse().ok()?;
    (0.0..=100.0).contains(&number).then_some(number / 100.0)
}

/// `10%,90%` as (0.1, 0.9).
fn percents(text: &str) -> Option<(f32, f32)> {
    let (x, y) = text.split_once(',')?;
    Some((percent(x)?, percent(y)?))
}

/// The place WebVTT cue settings give (`line`, `position`, `size`, `align`, `region`), for
/// text written left to right; none where they keep the usual place.
fn settings_place(settings: &str, regions: &[Region]) -> Option<Place> {
    let (mut align, mut line, mut position, mut size, mut region) = (None, None, None, 1.0, None);
    for (name, value) in settings.split_whitespace().filter_map(|setting| setting.split_once(':')) {
        match name {
            "align" => align = Some(value),
            "line" => line = Some(value),
            "position" => position = Some(value),
            "size" => size = percent(value).unwrap_or(size),
            "region" => region = regions.iter().find(|region| region.id == value),
            _ => {}
        }
    }
    if (align, line, position, region) == (None, None, None, None) && size == 1.0 {
        return None;
    }
    let lines = match align.unwrap_or("center") {
        "start" | "left" => Align::Start,
        "end" | "right" => Align::End,
        _ => Align::Center,
    };
    let (left, width) = box_across(lines, position, size);
    let across = |left: f32, width: f32| match lines {
        Align::Start => left,
        Align::End => left + width,
        _ => left + width / 2.0,
    };
    if let Some(region) = region {
        let (left, width) = (region.left + left * region.width, width * region.width);
        return Some(Place { x: across(left, width), across: lines, down: Down::At(region.bottom, Align::End), width, lines });
    }
    Some(Place { x: across(left, width), across: lines, down: line.and_then(line_place).unwrap_or(Down::Line(-1)), width, lines })
}

/// The left side and width of a cue's box: its `position` (with its own alignment, or one
/// following the text's) and `size`, cut to the frame (WebVTT's rules for its position).
fn box_across(lines: Align, position: Option<&str>, size: f32) -> (f32, f32) {
    let (at, side) = position.map_or((None, None), |value| match value.split_once(',') {
        Some((at, side)) => (percent(at), Some(side)),
        None => (percent(value), None),
    });
    let side = match side {
        Some("line-left") => Align::Start,
        Some("line-right") => Align::End,
        Some("center") => Align::Center,
        _ => lines,
    };
    let at = at.unwrap_or(match lines {
        Align::Start => 0.0,
        Align::End => 1.0,
        _ => 0.5,
    });
    match side {
        Align::Start => (at, size.min(1.0 - at)),
        Align::End => (at - size.min(at), size.min(at)),
        _ => {
            let width = size.min(2.0 * at.min(1.0 - at));
            (at - width / 2.0, width)
        }
    }
}

/// A cue's `line`: a line number, or a part of the frame's height with the side of its box
/// that lies there (`start`, `center`, `end`).
fn line_place(value: &str) -> Option<Down> {
    let (at, side) = value.split_once(',').unwrap_or((value, "start"));
    let side = match side {
        "center" => Align::Center,
        "end" => Align::End,
        _ => Align::Start,
    };
    match percent(at) {
        Some(part) => Some(Down::At(part, side)),
        None => at.parse().ok().map(Down::Line),
    }
}

/// A 3GPP timed text sample's text (TS 26.245: its length, then UTF-8, or UTF-16 after a byte
/// order mark; the style boxes after it dropped), trimmed.
#[cfg(any(test, windows, target_vendor = "apple", target_os = "android"))]
pub(crate) fn timed_text(sample: &[u8]) -> String {
    let [high, low, rest @ ..] = sample else { return String::new() };
    let text = &rest[..(usize::from(*high) << 8 | usize::from(*low)).min(rest.len())];
    let utf16 = |bytes: &[u8], read: fn([u8; 2]) -> u16| String::from_utf16_lossy(&bytes.as_chunks::<2>().0.iter().map(|two| read(*two)).collect::<Vec<u16>>());
    let text = match text {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, u16::from_be_bytes),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, u16::from_le_bytes),
        _ => String::from_utf8_lossy(text).into_owned(),
    };
    text.trim_end_matches('\0').trim().to_owned()
}

/// A WebVTT cue's text as a caption placed as its settings say (outside any region).
#[cfg(any(test, windows, target_vendor = "apple", target_family = "wasm"))]
pub(crate) fn caption_with_settings(text: &str, settings: &str) -> Caption {
    placed_caption(text, settings, &[])
}

/// A WebVTT cue's text as a caption placed as its settings say (in one of `regions`, where they
/// name it), else by SRT's `{\anN}`.
fn placed_caption(text: &str, settings: &str, regions: &[Region]) -> Caption {
    let caption = caption_of(text);
    Caption { place: settings_place(settings, regions).or(caption.place), ..caption }
}

/// A WebVTT or SRT cue's text as a caption: its lines without markup, at SRT's `{\anN}` place
/// if it has one, italic where all of it is in `<i>`.
pub(crate) fn caption_of(text: &str) -> Caption {
    let place = text.find("{\\an").and_then(|at| text[at + 4..].bytes().next()).filter(u8::is_ascii_digit).map(|digit| digit - b'0');
    let place = place.filter(|keypad| (1..=9).contains(keypad) && *keypad != 2).map(|keypad| Place::keypad(keypad, 0.05, 0.05, 0.05));
    let (text, italic) = styled(text);
    Caption { text, place, italic }
}

/// Text without its markup (`<i>`, `<c.red>`, `<v Ann>`, `<00:01.000>`, SRT's `{\an8}`), its
/// entities read; and whether all of it (but spaces) is in `<i>`.
pub(crate) fn styled(line: &str) -> (String, bool) {
    let mut out = String::with_capacity(line.len());
    let (mut rest, mut italic, mut all_italic, mut any) = (line, 0usize, true, false);
    while let Some(at) = rest.find(['<', '&', '{']) {
        let shown = &rest[..at];
        out.push_str(shown);
        (any, all_italic) = (any || !shown.trim().is_empty(), all_italic && (italic > 0 || shown.trim().is_empty()));
        rest = &rest[at..];
        italic = match rest.get(..3) {
            Some("<i>") => italic + 1,
            Some("</i") => italic.saturating_sub(1),
            _ => italic,
        };
        // (Each of the three is one byte.)
        let (shown, length) = markup(rest).unwrap_or((&rest[..1], 1));
        out.push_str(shown);
        (any, all_italic) = (any || !shown.trim().is_empty(), all_italic && (italic > 0 || shown.trim().is_empty()));
        rest = &rest[length..];
    }
    out.push_str(rest);
    (any, all_italic) = (any || !rest.trim().is_empty(), all_italic && (italic > 0 || rest.trim().is_empty()));
    (out, any && all_italic)
}

/// Text without its markup, its entities read (see `styled`).
#[cfg(test)]
pub(crate) fn plain(line: &str) -> String {
    styled(line).0
}

/// Markup at the start of `text`: what it shows, and its length; none where it is no markup.
fn markup(text: &str) -> Option<(&'static str, usize)> {
    if let Some(close) = [("<", '>'), ("{\\", '}')].into_iter().find_map(|(open, close)| text.starts_with(open).then_some(close)) {
        return text.find(close).map(|end| ("", end + 1));
    }
    let end = text.find(';').filter(|end| *end <= 6)?;
    let shown = match &text[..=end] {
        "&amp;" => "&",
        "&lt;" => "<",
        "&gt;" => ">",
        "&quot;" => "\"",
        "&apos;" => "'",
        "&nbsp;" => "\u{A0}",
        "&lrm;" => "\u{200E}",
        "&rlm;" => "\u{200F}",
        _ => return None,
    };
    Some((shown, end + 1))
}

/// The captions on show over time, kept as cues: each change ends the cues on show and starts
/// the next. For decoders that say only what changes (CEA-608, CEA-708).
#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub(crate) struct CueRecorder {
    /// The cues so far, by their start; the last `open` of them on show (their end not yet
    /// known).
    cues: Vec<Cue>,
    open: usize,
    /// The earliest time the next change can take: the last change's, or where a seek went
    /// (data read again from the key frame before it changes the text from there).
    not_before: Duration,
}

#[cfg(not(target_family = "wasm"))]
impl CueRecorder {
    /// The captions on show are `shown` from `at` (or `not_before`, if later). Cues changed again
    /// at the moment they started are replaced.
    pub(crate) fn show(&mut self, shown: Vec<Caption>, at: Duration) {
        let open = self.cues.len() - self.open;
        if self.cues[open..].iter().map(|cue| &cue.caption).eq(shown.iter()) {
            return;
        }
        let at = at.max(self.not_before);
        let replaced = self.cues[open..].iter().all(|cue| cue.from == at);
        match replaced {
            true => self.cues.truncate(open),
            false => self.cues[open..].iter_mut().for_each(|cue| cue.to = at),
        }
        (self.open, self.not_before) = (shown.len(), at);
        self.cues.extend(shown.into_iter().map(|caption| Cue { from: at, to: Duration::MAX, caption }));
    }

    /// Starts again at `to` after a seek: cues from there on are dropped (read again as it
    /// plays), those on show end there.
    pub(crate) fn sought(&mut self, to: Duration) {
        self.cues.truncate(self.cues.partition_point(|cue| cue.from < to));
        self.cues.iter_mut().for_each(|cue| cue.to = cue.to.min(to));
        (self.open, self.not_before) = (0, to);
    }

    /// The cues so far: each caption that showed, and when; the last ones end when the
    /// captions after them start (or never, while they show).
    pub(crate) fn cues(&self) -> &[Cue] {
        &self.cues
    }
}

/// The captions on show at `at` of cues sorted by their start: each cue whose span holds it.
pub(crate) fn captions_at(cues: &[Cue], at: Duration) -> Vec<Caption> {
    let started = cues.partition_point(|cue| cue.from <= at);
    cues[..started].iter().filter(|cue| cue.to > at).map(|cue| cue.caption.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn texts(cues: &[Cue]) -> Vec<&str> {
        cues.iter().map(|cue| cue.caption.text.as_str()).collect()
    }

    fn place_of(text: &str) -> Option<Place> {
        read(text).unwrap()[0].caption.place
    }

    fn near(one: f32, other: f32) -> bool {
        (one - other).abs() < 1e-4
    }

    #[test]
    fn webvtt_cues_are_read_without_their_markup() {
        let text = "\u{FEFF}WEBVTT - a test\n\nSTYLE\n::cue { color: red }\n\nNOTE a note\n\nfirst\n00:01.000 --> 00:02.500\n<v Ann>Hello <i>there</i></v>\n&lt;two&gt; &amp; lines\n\n01:00:00.5 --> 01:00:01.25\nlate";
        let cues = read(text).unwrap();
        assert_eq!(cues[0], Cue { from: ms(1000), to: ms(2500), caption: Caption::plain("Hello there\n<two> & lines") });
        assert_eq!((cues[1].from, cues[1].to, cues[1].caption.text.as_str()), (ms(3_600_500), ms(3_601_250), "late"));
    }

    #[test]
    fn srt_cues_are_read_and_sorted() {
        let text = "2\r\n00:00:03,000 --> 00:00:04,000\r\n{\\an2}<font color=\"red\">Second</font>\r\n\r\n1\r\n00:00:01,000 --> 00:00:02,000\r\nFirst\r\n";
        let cues = read(text).unwrap();
        assert_eq!(texts(&cues), ["First", "Second"]);
        assert_eq!((cues[1].from, cues[1].to, cues[1].caption.place), (ms(3000), ms(4000), None));
    }

    #[test]
    fn text_with_no_cues_or_bad_times_is_refused() {
        assert_eq!(read("WEBVTT\n\nno cue here"), Err(CaptionsError::NoCues));
        assert_eq!(read("WEBVTT\n\n00:0x.000 --> 00:02.000\nbad"), Err(CaptionsError::BadTime(3)));
        assert_eq!(read("99999999999999999999:00:00.000 --> 00:02.000\nlong"), Err(CaptionsError::BadTime(1)));
    }

    #[test]
    fn the_captions_on_show_are_each_cue_holding_the_time() {
        let cue = |from, to, text: &str| Cue { from: ms(from), to: ms(to), caption: Caption::plain(text) };
        let cues = [cue(0, 1000, "a"), cue(500, 2000, "b"), cue(3000, 4000, "c")];
        let at = |millis| captions_at(&cues, ms(millis)).into_iter().map(|caption| caption.text).collect::<Vec<_>>().join("\n");
        assert_eq!([at(0), at(700), at(1000), at(2500), at(3000), at(4000)], ["a", "a\nb", "b", "", "c", ""]);
    }

    #[test]
    fn a_lone_ampersand_or_brace_stays() {
        assert_eq!(plain("Tom & Jerry {laughs} a<b"), "Tom & Jerry {laughs} a<b");
    }

    #[test]
    fn a_cue_all_in_italics_is_italic_and_one_partly_so_is_not() {
        assert_eq!(styled("<i>Whispered</i>"), ("Whispered".into(), true));
        assert_eq!(styled(" <i>two</i>  <i>words</i> "), (" two  words ".into(), true));
        assert_eq!(styled("Said <i>softly</i>"), ("Said softly".into(), false));
        assert_eq!(styled("<i></i>"), (String::new(), false));
        let cues = read("WEBVTT\n\n00:01.000 --> 00:02.000\n<i>one</i>\n<i>two</i>").unwrap();
        assert!(cues[0].caption.italic);
    }

    #[test]
    fn webvtt_settings_place_a_cue_across_and_down() {
        // (Left aligned at 10% in a box of 40%; its top at 20% down.)
        let place = place_of("WEBVTT\n\n00:01.000 --> 00:02.000 align:start position:10% size:40% line:20%\nleft").unwrap();
        assert!(near(place.x, 0.1) && place.across == Align::Start && near(place.width, 0.4), "{place:?}");
        assert_eq!((place.down, place.lines), (Down::At(0.2, Align::Start), Align::Start));
        // (Right aligned at the right edge; its bottom at 90%.)
        let place = place_of("WEBVTT\n\n00:01.000 --> 00:02.000 align:end line:90%,end\nright").unwrap();
        assert!(near(place.x, 1.0) && place.across == Align::End && near(place.width, 1.0), "{place:?}");
        assert_eq!(place.down, Down::At(0.9, Align::End));
        // (Line numbers count from the top, or from the bottom below 0.)
        assert_eq!(place_of("WEBVTT\n\n00:01.000 --> 00:02.000 line:0\ntop").unwrap().down, Down::Line(0));
        assert_eq!(place_of("WEBVTT\n\n00:01.000 --> 00:02.000 line:-3\nhigher").unwrap().down, Down::Line(-3));
        // (No settings, or ones that keep the usual: the usual place.)
        assert_eq!(place_of("WEBVTT\n\n00:01.000 --> 00:02.000\nusual"), None);
        assert_eq!(place_of("WEBVTT\n\n00:01.000 --> 00:02.000 vertical:rl\nusual"), None);
    }

    #[test]
    fn a_webvtt_position_keeps_the_box_on_the_frame() {
        // (Centred at 80% with a size of 60%: cut to 40% so it ends at the right edge.)
        let place = place_of("WEBVTT\n\n00:01.000 --> 00:02.000 position:80% size:60%\ncut").unwrap();
        assert!(near(place.x, 0.8) && near(place.width, 0.4) && place.across == Align::Center, "{place:?}");
        // (Its own alignment: the box's right side at 30%, the text centred in it.)
        let place = place_of("WEBVTT\n\n00:01.000 --> 00:02.000 position:30%,line-right size:20%\nsided").unwrap();
        assert!(near(place.x, 0.2) && near(place.width, 0.2) && place.across == Align::Center, "{place:?}");
    }

    #[test]
    fn a_cue_in_a_webvtt_region_sits_at_the_regions_bottom() {
        let text = "WEBVTT\n\nREGION\nid:fred width:40% lines:3\nregionanchor:0%,100% viewportanchor:10%,90%\n\n00:01.000 --> 00:02.000 region:fred align:left\nin it\n\n00:03.000 --> 00:04.000 region:other\nnot";
        let cues = read(text).unwrap();
        let place = cues[0].caption.place.unwrap();
        assert!(near(place.x, 0.1) && near(place.width, 0.4) && place.across == Align::Start, "{place:?}");
        assert_eq!(place.down, Down::At(0.9, Align::End));
        assert_eq!(cues[1].caption.place, None);
    }

    #[test]
    fn timed_text_is_read_as_utf8_or_as_utf16_after_its_byte_order_mark() {
        assert_eq!(timed_text(b"\0\x05Hello styles after"), "Hello");
        assert_eq!(timed_text(&[0, 6, 0xFE, 0xFF, 0, b'H', 0, b'i']), "Hi");
        assert_eq!(timed_text(&[0, 6, 0xFF, 0xFE, b'H', 0, b'i', 0]), "Hi");
        assert_eq!(timed_text(&[0]), "");
    }

    #[test]
    fn srt_keypad_tags_place_a_cue() {
        let cues = read("1\n00:00:01,000 --> 00:00:02,000\n{\\an8}On top\n\n2\n00:00:03,000 --> 00:00:04,000\n{\\an1}Low left\n").unwrap();
        let top = cues[0].caption.place.unwrap();
        assert_eq!((cues[0].caption.text.as_str(), top.across, top.down), ("On top", Align::Center, Down::At(0.05, Align::Start)));
        let low = cues[1].caption.place.unwrap();
        assert!(near(low.x, 0.05) && low.across == Align::Start && low.down == Down::At(0.95, Align::End), "{low:?}");
    }

    #[test]
    fn the_keypad_puts_a_box_at_its_corner_or_middle_inside_the_margins() {
        let middle = Place::keypad(5, 0.1, 0.2, 0.05);
        assert!(near(middle.x, 0.45) && middle.across == Align::Center && middle.down == Down::At(0.5, Align::Center) && near(middle.width, 0.7));
        let top_right = Place::keypad(9, 0.1, 0.2, 0.05);
        assert!(near(top_right.x, 0.8) && top_right.across == Align::End && top_right.down == Down::At(0.05, Align::Start));
        // (Margins bigger than the frame, or below nothing, keep the box on it.)
        let off = Place::keypad(9, -1.0, 4.0, 2.5);
        assert!(near(off.x, 0.5) && off.down == Down::At(0.5, Align::Start) && near(off.width, 0.5), "{off:?}");
    }

    #[test]
    #[cfg(not(target_family = "wasm"))]
    fn the_recorder_keeps_each_change_of_several_captions_as_cues() {
        let mut recorder = CueRecorder::default();
        let (one, two) = (Caption::plain("one"), Caption::plain("two"));
        recorder.show(vec![one.clone(), two.clone()], ms(100));
        recorder.show(vec![one.clone(), two.clone()], ms(200));
        recorder.show(vec![one.clone()], ms(300));
        recorder.show(Vec::new(), ms(400));
        let spans: Vec<(u64, u64, &str)> = recorder.cues().iter().map(|cue| (cue.from.as_millis() as u64, cue.to.as_millis() as u64, cue.caption.text.as_str())).collect();
        assert_eq!(spans, [(100, 300, "one"), (100, 300, "two"), (300, 400, "one")]);
        // (Changed again at the moment they started: replaced.)
        recorder.show(vec![two.clone()], ms(500));
        recorder.show(vec![one.clone()], ms(500));
        assert_eq!(texts(&recorder.cues()[3..]), ["one"]);
    }
}
