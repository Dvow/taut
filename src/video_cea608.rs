//! CEA-608 captions carried in a video stream (ATSC A/53 `cc_data`, as in H.264's and HEVC's
//! SEI) or in a QuickTime closed-caption track, made into cues (see `video_stream_captions`).
//!
//! Channel 1 of field 1 (`CC1`) is read: pop-on, roll-up and paint-on captions, the standard,
//! special and extended characters. Colours, italics and underlines are not kept: text only,
//! as the app's captions are.

use crate::video_captions::{Cue, CueRecorder};
use std::time::Duration;

/// Rows and columns of the caption screen.
const ROWS: usize = 15;
const COLUMNS: usize = 32;

/// How captions come on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Loaded off screen, then swapped on at once.
    PopOn,
    /// Rows rising from the base row, this many on show.
    RollUp(usize),
    /// Written straight onto the screen.
    PaintOn,
}

/// A caption screen: rows of characters, spaces where none.
type Screen = [[char; COLUMNS]; ROWS];

/// Reads CC1's byte pairs in order and keeps the cues they show.
pub(crate) struct Decoder {
    mode: Mode,
    shown: Screen,
    hidden: Screen,
    /// Where the next character goes: row and column.
    row: usize,
    column: usize,
    /// The last control pair, seen once: its repeat (sent twice for safety) is skipped.
    last_control: Option<[u8; 2]>,
    /// The channel of the last control pair (data of channel 2 is skipped until channel 1 is
    /// named again).
    on_channel_one: bool,
    /// The text on show over time.
    recorder: CueRecorder,
    /// Whether any caption data came.
    found: bool,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder { mode: Mode::PopOn, shown: blank(), hidden: blank(), row: ROWS - 1, column: 0, last_control: None, on_channel_one: true, recorder: CueRecorder::default(), found: false }
    }
}

fn blank() -> Screen {
    [[' '; COLUMNS]; ROWS]
}

/// A screen's text: its rows with anything on them, trimmed, one a line.
fn text_of(screen: &Screen) -> String {
    let rows = screen.iter().map(|row| row.iter().collect::<String>().trim().to_owned()).filter(|row| !row.is_empty());
    rows.collect::<Vec<_>>().join("\n")
}

impl Decoder {
    /// Reads `cc_data`'s triplets (the bytes after `cc_count` in A/53: marker and validity, then
    /// a pair), all at time `at`.
    pub(crate) fn read_cc_data(&mut self, triplets: &[u8], at: Duration) {
        for &[marks, first, second] in triplets.as_chunks::<3>().0 {
            // (Valid, and of field 1: NTSC line 21's first field.)
            if marks & 0x07 == 0x04 {
                self.read_pair([first, second], at);
            }
        }
    }

    /// Reads one byte pair of field 1, at time `at`.
    pub(crate) fn read_pair(&mut self, pair: [u8; 2], at: Duration) {
        let [first, second] = pair.map(|byte| byte & 0x7F);
        if first == 0 && second == 0 {
            return;
        }
        self.found = true;
        let control = (0x10..0x20).contains(&first);
        if !control {
            self.last_control = None;
            if self.on_channel_one {
                [first, second].into_iter().filter(|byte| *byte >= 0x20).for_each(|byte| self.put(standard(byte)));
                self.shown_changed(at);
            }
            return;
        }
        if self.last_control == Some([first, second]) {
            self.last_control = None;
            return;
        }
        self.last_control = Some([first, second]);
        self.on_channel_one = first & 0x08 == 0;
        if self.on_channel_one {
            self.control(first & !0x08, second);
            self.shown_changed(at);
        }
    }

    /// Starts again at `to` after a seek: what was read from there on is read again as it
    /// plays, the caption on show then ends there (608 says what shows only as it changes).
    /// Data from before `to` (from the key frame before it) changes the screen from `to`.
    pub(crate) fn sought(&mut self, to: Duration) {
        let mut recorder = std::mem::take(&mut self.recorder);
        recorder.sought(to);
        *self = Decoder { recorder, found: self.found, ..Decoder::default() };
    }

    /// Whether any caption data came (not only padding): the stream has captions.
    pub(crate) fn found(&self) -> bool {
        self.found
    }

    /// The cues read so far: each caption that showed, and when; the last one ends when the
    /// text after it starts (or never, while it shows).
    pub(crate) fn cues(&self) -> &[Cue] {
        self.recorder.cues()
    }

    /// A control pair of channel 1 (its first byte as channel 1 names it).
    fn control(&mut self, first: u8, second: u8) {
        match (first, second) {
            (0x14, 0x20..=0x2F) => self.command(second),
            (0x17, 0x21..=0x23) => self.column = (self.column + (second - 0x20) as usize).min(COLUMNS - 1),
            (0x11, 0x30..=0x3F) => self.put(special(second)),
            (0x12 | 0x13, 0x20..=0x3F) => {
                // An extended character takes the place of the standard one sent before it.
                self.column = self.column.saturating_sub(1);
                self.put(extended(first, second));
            }
            // A mid-row change of colour or style: shown as a space.
            (0x11, 0x20..=0x2F) => self.put(' '),
            (0x10..=0x17, 0x40..=0x7F) => self.preamble(first, second),
            _ => {}
        }
    }

    /// A preamble address code: the row and column the next text starts at.
    fn preamble(&mut self, first: u8, second: u8) {
        const ROW_OF: [usize; 8] = [10, 0, 2, 11, 13, 4, 6, 8];
        let row = ROW_OF[(first & 0x07) as usize] + ((second & 0x20) != 0) as usize;
        let indent = if second & 0x10 != 0 { ((second & 0x0E) >> 1) as usize * 4 } else { 0 };
        let row = row.min(ROWS - 1);
        if let Mode::RollUp(rows) = self.mode {
            // (Roll-up text keeps its rows together: they move to where the base row is now.)
            self.move_rows(self.row, row, rows);
        }
        (self.row, self.column) = (row, indent);
    }

    /// The commands of `0x14 0x20` to `0x14 0x2F`.
    fn command(&mut self, second: u8) {
        match second {
            0x20 => self.mode = Mode::PopOn,
            0x21 => {
                let (row, column) = (self.row, self.column.saturating_sub(1));
                (self.writing()[row][column], self.column) = (' ', column);
            }
            0x24 => {
                let (row, column) = (self.row, self.column);
                self.writing()[row][column..].fill(' ');
            }
            0x25..=0x27 => self.roll_up(second as usize - 0x23),
            0x29 => self.mode = Mode::PaintOn,
            0x2C => self.shown = blank(),
            0x2D => self.carriage_return(),
            0x2E => self.hidden = blank(),
            0x2F => {
                std::mem::swap(&mut self.shown, &mut self.hidden);
                self.mode = Mode::PopOn;
            }
            _ => {}
        }
    }

    /// Roll-up with this many rows: from pop-on or paint-on, the screen is cleared first.
    fn roll_up(&mut self, rows: usize) {
        if !matches!(self.mode, Mode::RollUp(_)) {
            (self.shown, self.hidden, self.row, self.column) = (blank(), blank(), ROWS - 1, 0);
        }
        self.mode = Mode::RollUp(rows);
    }

    /// A new line: in roll-up the rows move up one, the base row emptied; else the column
    /// goes back to the start.
    fn carriage_return(&mut self) {
        let Mode::RollUp(rows) = self.mode else { return self.column = 0 };
        let top = (self.row + 1).saturating_sub(rows);
        for row in top..self.row {
            self.shown[row] = self.shown[row + 1];
        }
        self.shown[..top].iter_mut().for_each(|row| *row = [' '; COLUMNS]);
        (self.shown[self.row], self.column) = ([' '; COLUMNS], 0);
    }

    /// Moves the `rows` roll-up rows ending at `from` to end at `to`.
    fn move_rows(&mut self, from: usize, to: usize, rows: usize) {
        if from == to {
            return;
        }
        let kept: Vec<[char; COLUMNS]> = (0..rows).filter_map(|up| from.checked_sub(up)).map(|row| self.shown[row]).collect();
        self.shown = blank();
        for (up, row) in kept.into_iter().enumerate().filter(|(up, _)| *up <= to) {
            self.shown[to - up] = row;
        }
    }

    /// The memory characters go into: off screen in pop-on, else the screen.
    fn writing(&mut self) -> &mut Screen {
        match self.mode {
            Mode::PopOn => &mut self.hidden,
            _ => &mut self.shown,
        }
    }

    /// Writes a character where the cursor is and moves it on (it stays in the last column).
    fn put(&mut self, letter: char) {
        let (row, column) = (self.row, self.column);
        self.writing()[row][column] = letter;
        self.column = (column + 1).min(COLUMNS - 1);
    }

    /// The text on the screen may have changed at `at`.
    fn shown_changed(&mut self, at: Duration) {
        self.recorder.show(text_of(&self.shown), at);
    }
}

/// A standard character: ASCII but for ten places.
fn standard(byte: u8) -> char {
    match byte {
        0x2A => 'á',
        0x5C => 'é',
        0x5E => 'í',
        0x5F => 'ó',
        0x60 => 'ú',
        0x7B => 'ç',
        0x7C => '÷',
        0x7D => 'Ñ',
        0x7E => 'ñ',
        0x7F => '█',
        _ => byte as char,
    }
}

/// A special character (`0x11 0x30` to `0x11 0x3F`).
fn special(second: u8) -> char {
    ['®', '°', '½', '¿', '™', '¢', '£', '♪', 'à', ' ', 'è', 'â', 'ê', 'î', 'ô', 'û'][(second - 0x30) as usize]
}

/// An extended character (`0x12` or `0x13`, then `0x20` to `0x3F`): Spanish, French,
/// Portuguese, German and Danish letters, and signs.
fn extended(first: u8, second: u8) -> char {
    const SPANISH_FRENCH: [char; 32] =
        ['Á', 'É', 'Ó', 'Ú', 'Ü', 'ü', '‘', '¡', '*', '\'', '—', '©', '℠', '•', '“', '”', 'À', 'Â', 'Ç', 'È', 'Ê', 'Ë', 'ë', 'Î', 'Ï', 'ï', 'Ô', 'Ù', 'ù', 'Û', '«', '»'];
    const PORTUGUESE_GERMAN: [char; 32] =
        ['Ã', 'ã', 'Í', 'Ì', 'ì', 'Ò', 'ò', 'Õ', 'õ', '{', '}', '\\', '^', '_', '|', '~', 'Ä', 'ä', 'Ö', 'ö', 'ß', '¥', '¤', '│', 'Å', 'å', 'Ø', 'ø', '┌', '┐', '└', '┘'];
    let table = if first == 0x12 { SPANISH_FRENCH } else { PORTUGUESE_GERMAN };
    table[(second - 0x20) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::odd;

    fn at(tenths: u64) -> Duration {
        Duration::from_millis(tenths * 100)
    }

    /// Feeds pairs, a tenth of a second apart from 0.
    fn decoded(pairs: &[[u8; 2]]) -> Vec<Cue> {
        let mut decoder = Decoder::default();
        (0..).zip(pairs).for_each(|(tenth, pair)| decoder.read_pair(*pair, at(tenth)));
        decoder.cues().to_vec()
    }

    /// A control pair of channel 1, as sent: twice.
    fn twice(second: u8) -> [[u8; 2]; 2] {
        [[0x94, odd(second)]; 2]
    }

    /// Text as pairs.
    fn letters(text: &str) -> Vec<[u8; 2]> {
        let bytes: Vec<u8> = text.bytes().map(odd).collect();
        bytes.chunks(2).map(|two| [two[0], two.get(1).copied().unwrap_or(0x80)]).collect()
    }

    #[test]
    fn a_pop_on_caption_shows_from_its_end_of_caption_until_it_is_erased() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x20), twice(0x2E), twice(0x70)].concat();
        pairs.extend(letters("Hello there"));
        let shown = pairs.len() as u64;
        pairs.extend(twice(0x2F));
        pairs.extend([[0x80, 0x80]; 5]);
        let erased = pairs.len() as u64;
        pairs.extend(twice(0x2C));
        assert_eq!(decoded(&pairs), [Cue { from: at(shown), to: at(erased), text: "Hello there".into() }]);
    }

    #[test]
    fn roll_up_rows_rise_and_each_new_line_is_a_new_cue() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x25), twice(0x2D)].concat();
        pairs.extend(letters("one"));
        pairs.extend(twice(0x2D));
        pairs.extend(letters("two"));
        pairs.extend(twice(0x2D));
        pairs.extend(letters("three"));
        let texts: Vec<String> = decoded(&pairs).into_iter().map(|cue| cue.text).collect();
        // (Two rows on show: the oldest goes up and off.)
        assert_eq!(texts.last().map(String::as_str), Some("two\nthree"));
        assert!(texts.contains(&"one\ntwo".to_owned()), "{texts:?}");
        assert!(texts.iter().all(|text| text.lines().count() <= 2), "{texts:?}");
    }

    #[test]
    fn paint_on_text_shows_as_it_is_written_and_backspace_takes_a_letter_back() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Caps"));
        pairs.extend(twice(0x21));
        pairs.extend(letters("t"));
        let cues = decoded(&pairs);
        assert_eq!(cues.first().map(|cue| cue.text.as_str()), Some("Ca"));
        assert_eq!(cues.last().map(|cue| cue.text.as_str()), Some("Capt"));
    }

    #[test]
    fn special_and_extended_characters_are_read() {
        // "Ole" + extended Á over the O, then a note sign, and a standard ñ.
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Ole"));
        pairs.push([0x92, odd(0x20)]);
        pairs.push([0x91, odd(0x37)]);
        pairs.push([odd(0x7E), 0x80]);
        assert_eq!(decoded(&pairs).last().map(|cue| cue.text.clone()), Some("OlÁ♪ñ".into()));
    }

    #[test]
    fn padding_alone_is_no_captions() {
        let mut decoder = Decoder::default();
        decoder.read_cc_data(&[0xFC, 0x80, 0x80, 0xF8, 0x94, 0x20], at(0));
        assert!(!decoder.found() && decoder.cues().is_empty());
        decoder.read_pair([0x94, 0x2C], at(1));
        assert!(decoder.found());
    }

    #[test]
    fn after_a_seek_the_caption_on_show_ends_there_and_what_follows_is_read_again() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Painted"));
        let mut decoder = Decoder::default();
        (0..).zip(&pairs).for_each(|(tenth, pair)| decoder.read_pair(*pair, at(tenth)));
        let first = decoder.cues()[0].from;
        decoder.sought(at(7));
        assert!(decoder.cues().iter().all(|cue| cue.from < at(7) && cue.to <= at(7)), "{:?}", decoder.cues());
        assert_eq!(decoder.cues()[0].from, first);
        // (Read again from the seek: nothing shows until a command says what.)
        decoder.read_pair([odd(b'x'), 0x80], at(8));
        assert_eq!(decoder.cues().last().map(|cue| cue.to), Some(at(7)));
    }

    #[test]
    fn data_read_again_from_the_key_frame_before_a_seek_changes_the_captions_from_the_seek() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Painted"));
        let mut decoder = Decoder::default();
        (0..).zip(&pairs).for_each(|(tenth, pair)| decoder.read_pair(*pair, at(tenth)));
        let played = decoder.cues().to_vec();
        decoder.sought(at(5));
        (0..).zip(&pairs).for_each(|(tenth, pair)| decoder.read_pair(*pair, at(tenth)));
        assert_eq!(decoder.cues(), played);
    }

    #[test]
    fn changes_at_one_moment_make_one_cue() {
        let mut decoder = Decoder::default();
        let pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat().into_iter().chain(letters("one two")).collect();
        pairs.iter().for_each(|pair| decoder.read_pair(*pair, at(3)));
        assert_eq!(decoder.cues(), [Cue { from: at(3), to: Duration::MAX, text: "one two".into() }]);
    }

    #[test]
    fn channel_two_is_skipped() {
        let mut pairs: Vec<[u8; 2]> = vec![[0x9C, odd(0x29)], [0x9C, odd(0x70)]];
        pairs.extend(letters("other"));
        assert!(decoded(&pairs).is_empty());
    }
}
