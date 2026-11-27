//! Captions read by taut: an app's WebVTT or SRT text made into cues, the text on show over time
//! kept as cues (for the decoders of captions in a video stream), and the cue on show at a time
//! (also for cues a system's player hands over). Text only: tags are dropped, entities read, a
//! cue's lines kept; positions, regions and styles are not.

use std::time::Duration;

/// A caption and its time span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cue {
    pub from: Duration,
    pub to: Duration,
    pub text: String,
}

/// Why captions were not read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CaptionsError {
    /// No cue in it: not WebVTT or SRT, or empty.
    NoCues,
    /// A cue's times, on this line (from 1), could not be read.
    BadTime(usize),
}

impl std::fmt::Display for CaptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            CaptionsError::NoCues => write!(f, "no captions in it (WebVTT or SRT)"),
            CaptionsError::BadTime(line) => write!(f, "a caption's times on line {line} can't be read"),
        }
    }
}

impl std::error::Error for CaptionsError {}

/// The cues of WebVTT or SRT text, by their start. Each block with a `-->` line is a cue: the
/// lines after it are its text; anything else (WebVTT's header, notes, styles, regions; SRT's
/// numbers) is skipped.
pub(crate) fn read(text: &str) -> Result<Vec<Cue>, CaptionsError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let lines: Vec<(usize, &str)> = text.lines().map(|line| line.trim_end_matches('\r')).enumerate().collect();
    let mut cues = Vec::new();
    for block in lines.split(|(_, line)| line.trim().is_empty()) {
        let Some(timed) = block.iter().position(|(_, line)| line.contains("-->")) else { continue };
        let (number, times) = block[timed];
        let (from, to) = times_of(times).ok_or(CaptionsError::BadTime(number + 1))?;
        let words: Vec<String> = block[timed + 1..].iter().map(|(_, line)| plain(line)).collect();
        cues.push(Cue { from, to, text: words.join("\n") });
    }
    if cues.is_empty() {
        return Err(CaptionsError::NoCues);
    }
    cues.sort_by_key(|cue| cue.from);
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
fn time_of(text: &str) -> Option<Duration> {
    let (whole, thousandths) = text.split_once(['.', ',']).unwrap_or((text, "0"));
    let mut seconds = 0u64;
    for part in whole.split(':') {
        seconds = seconds.checked_mul(60)?.checked_add(part.parse().ok()?)?;
    }
    let millis: u64 = thousandths.get(..3.min(thousandths.len()))?.parse().ok()?;
    let millis = millis * 10u64.pow(3 - thousandths.len().min(3) as u32);
    Some(Duration::from_millis(seconds.checked_mul(1000)?.checked_add(millis)?))
}

/// A caption line as plain text: tags (`<i>`, `<c.red>`, `<v Ann>`, `<00:01.000>`, SRT's
/// `{\an8}`) dropped, entities read.
pub(crate) fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(at) = rest.find(['<', '&', '{']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // (Each of the three is one byte.)
        let (shown, length) = markup(rest).unwrap_or((&rest[..1], 1));
        out.push_str(shown);
        rest = &rest[length..];
    }
    out.push_str(rest);
    out
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

/// The text on show over time, kept as cues: each change ends the cue on show and starts the
/// next. For decoders that say only what changes (CEA-608, CEA-708).
#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub(crate) struct CueRecorder {
    /// The cues so far, in time order, the last one open (its end not yet known) while its text
    /// shows.
    cues: Vec<Cue>,
    open: bool,
    /// The earliest time the next change can take: the last change's, or where a seek went
    /// (data read again from the key frame before it changes the text from there).
    not_before: Duration,
}

#[cfg(not(target_family = "wasm"))]
impl CueRecorder {
    /// The text on show is `text` from `at` (or `not_before`, if later), none where empty. A cue
    /// changed again at the moment it started is replaced.
    pub(crate) fn show(&mut self, text: String, at: Duration) {
        let open = self.cues.last_mut().filter(|_| self.open);
        if open.as_ref().is_some_and(|cue| cue.text == text) || (!self.open && text.is_empty()) {
            return;
        }
        let at = at.max(self.not_before);
        match open {
            Some(cue) if cue.from == at => _ = self.cues.pop(),
            Some(cue) => cue.to = at,
            None => {}
        }
        (self.open, self.not_before) = (!text.is_empty(), at);
        if self.open {
            self.cues.push(Cue { from: at, to: Duration::MAX, text });
        }
    }

    /// Starts again at `to` after a seek: cues from there on are dropped (read again as it
    /// plays), the one on show ends there.
    pub(crate) fn sought(&mut self, to: Duration) {
        self.cues.truncate(self.cues.partition_point(|cue| cue.from < to));
        self.cues.iter_mut().for_each(|cue| cue.to = cue.to.min(to));
        (self.open, self.not_before) = (false, to);
    }

    /// The cues so far: each text that showed, and when; the last one ends when the text after
    /// it starts (or never, while it shows).
    pub(crate) fn cues(&self) -> &[Cue] {
        &self.cues
    }
}

/// The text on show at `at` of cues sorted by their start: each cue whose span holds it, one
/// to a line.
pub(crate) fn text_at(cues: &[Cue], at: Duration) -> Option<String> {
    let started = cues.partition_point(|cue| cue.from <= at);
    let mut shown = cues[..started].iter().filter(|cue| cue.to > at).map(|cue| cue.text.as_str());
    let first = shown.next()?;
    Some(shown.fold(first.to_owned(), |all, more| all + "\n" + more))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn webvtt_cues_are_read_without_their_markup() {
        let text = "\u{FEFF}WEBVTT - a test\n\nSTYLE\n::cue { color: red }\n\nNOTE a note\n\nfirst\n00:01.000 --> 00:02.500 align:start\n<v Ann>Hello <i>there</i></v>\n&lt;two&gt; &amp; lines\n\n01:00:00.5 --> 01:00:01.25\nlate";
        let cues = read(text).unwrap();
        assert_eq!(cues[0], Cue { from: ms(1000), to: ms(2500), text: "Hello there\n<two> & lines".into() });
        assert_eq!((cues[1].from, cues[1].to, cues[1].text.as_str()), (ms(3_600_500), ms(3_601_250), "late"));
    }

    #[test]
    fn srt_cues_are_read_and_sorted() {
        let text = "2\r\n00:00:03,000 --> 00:00:04,000\r\n{\\an8}<font color=\"red\">Second</font>\r\n\r\n1\r\n00:00:01,000 --> 00:00:02,000\r\nFirst\r\n";
        let cues = read(text).unwrap();
        assert_eq!(cues.iter().map(|cue| cue.text.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
        assert_eq!((cues[1].from, cues[1].to), (ms(3000), ms(4000)));
    }

    #[test]
    fn text_with_no_cues_or_bad_times_is_refused() {
        assert_eq!(read("WEBVTT\n\nno cue here"), Err(CaptionsError::NoCues));
        assert_eq!(read("WEBVTT\n\n00:0x.000 --> 00:02.000\nbad"), Err(CaptionsError::BadTime(3)));
        assert_eq!(read("99999999999999999999:00:00.000 --> 00:02.000\nlong"), Err(CaptionsError::BadTime(1)));
    }

    #[test]
    fn the_text_on_show_is_each_cue_holding_the_time() {
        let cue = |from, to, text: &str| Cue { from: ms(from), to: ms(to), text: text.into() };
        let cues = [cue(0, 1000, "a"), cue(500, 2000, "b"), cue(3000, 4000, "c")];
        let at = |millis| text_at(&cues, ms(millis));
        assert_eq!([at(0), at(700), at(1000), at(2500), at(3000), at(4000)], [Some("a".into()), Some("a\nb".into()), Some("b".into()), None, Some("c".into()), None]);
    }

    #[test]
    fn a_lone_ampersand_or_brace_stays() {
        assert_eq!(plain("Tom & Jerry {laughs} a<b"), "Tom & Jerry {laughs} a<b");
    }
}
