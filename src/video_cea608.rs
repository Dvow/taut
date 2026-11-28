//! CEA-608 captions carried in a video stream (ATSC A/53 `cc_data`, as in H.264's and HEVC's
//! SEI) or in a QuickTime closed-caption track, made into cues (see `video_stream_captions`).
//!
//! Both fields are read, each with two data channels, each channel a caption service (`CC1` to
//! `CC4`) and a text service (`T1` to `T4`). Captions come pop-on, roll-up or paint-on, each row
//! placed at its row and indent on the caption screen (the middle 80% of the frame, 15 rows by
//! 32 columns); text services fill rows from the top, scrolling. The standard, special and
//! extended characters are read, and italics; colours and underlines are not kept. XDS data
//! (in field 2) is skipped.

use crate::layout::Align;
use crate::video_captions::{Caption, Cue, CueRecorder, Down, Place};
use std::time::Duration;

/// Rows and columns of the caption screen.
const ROWS: usize = 15;
const COLUMNS: usize = 32;
/// Where the caption screen starts, from the frame's edges, and how much of the frame it is.
const MARGIN: f32 = 0.1;
const SCREEN: f32 = 0.8;

/// The services' names: field 1's channels' captions and text, then field 2's.
pub(crate) const NAMES: [&str; 8] = ["CC1", "CC2", "T1", "T2", "CC3", "CC4", "T3", "T4"];

/// How captions come on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Loaded off screen, then swapped on at once.
    PopOn,
    /// Rows rising from the base row, this many on show.
    RollUp(usize),
    /// Written straight onto the screen.
    PaintOn,
    /// A text service: rows from the top, scrolling up at the bottom.
    Text,
}

/// A place on the caption screen: a character, and whether it is italic.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    letter: char,
    italic: bool,
}

const BLANK: Cell = Cell { letter: ' ', italic: false };

/// A caption screen: rows of cells.
type Screen = [[Cell; COLUMNS]; ROWS];

/// Reads both fields' byte pairs in order and keeps the cues each service shows.
#[derive(Default)]
pub(crate) struct Decoder {
    fields: [Field; 2],
}

impl Decoder {
    /// Reads `cc_data`'s triplets (the bytes after `cc_count` in A/53: marker, validity and
    /// type, then a pair), all at time `at`: those of field 1 and field 2 (types 0 and 1).
    pub(crate) fn read_cc_data(&mut self, triplets: &[u8], at: Duration) {
        for &[marks, first, second] in triplets.as_chunks::<3>().0 {
            match marks & 0x07 {
                0x04 => self.fields[0].read_pair([first, second], at),
                0x05 => self.fields[1].read_pair([first, second], at),
                _ => {}
            }
        }
    }

    /// Reads one byte pair of field 1 or 2 (`field` 0 or 1), at time `at`.
    #[cfg(any(test, windows, target_vendor = "apple", all(unix, not(target_os = "android"))))]
    pub(crate) fn read_pair(&mut self, field: usize, pair: [u8; 2], at: Duration) {
        self.fields[field.min(1)].read_pair(pair, at);
    }

    /// Starts again at `to` after a seek: what was read from there on is read again as it
    /// plays, the captions on show then end there (608 says what shows only as it changes).
    /// Data from before `to` (from the key frame before it) changes the screen from `to`.
    pub(crate) fn sought(&mut self, to: Duration) {
        self.fields.iter_mut().for_each(|field| field.sought(to));
    }

    /// Each service (by its place in `NAMES`): whether any data came for it, and its cues so
    /// far (each caption that showed, and when; the last ones end when the captions after them
    /// start, or never while they show).
    pub(crate) fn services(&self) -> impl Iterator<Item = (bool, &[Cue])> {
        self.fields.iter().flat_map(|field| field.services.iter().map(|service| (service.found, service.recorder.cues())))
    }
}

/// One field's data: two channels, each with a caption service and a text service.
#[derive(Default)]
struct Field {
    /// Channel 1's captions, channel 2's, channel 1's text, channel 2's.
    services: [Service; 4],
    /// The data channel the last control pair named (0 or 1), and whether each channel's data
    /// goes to its text service.
    channel: usize,
    text: [bool; 2],
    /// The last control pair, seen once: its repeat (sent twice for safety) is skipped.
    last_control: Option<[u8; 2]>,
    /// Within an XDS packet (its data is skipped until it ends or a control pair comes).
    xds: bool,
}

impl Field {
    /// Reads one byte pair at time `at`.
    fn read_pair(&mut self, pair: [u8; 2], at: Duration) {
        let [first, second] = pair.map(|byte| byte & 0x7F);
        if first == 0 && second == 0 {
            return;
        }
        if (0x01..0x10).contains(&first) {
            // (XDS: 0x01 to 0x0E start or go on with a packet, 0x0F and its checksum end it.)
            self.xds = first != 0x0F;
            return;
        }
        if !(0x10..0x20).contains(&first) {
            self.last_control = None;
            if !self.xds {
                let service = self.service();
                [first, second].into_iter().filter(|byte| *byte >= 0x20).for_each(|byte| service.put(standard(byte)));
                service.shown_changed(at);
            }
            return;
        }
        self.xds = false;
        if self.last_control == Some([first, second]) {
            self.last_control = None;
            return;
        }
        self.last_control = Some([first, second]);
        self.channel = usize::from(first & 0x08 != 0);
        self.control(first & !0x08, second, at);
    }

    /// The service the current channel's data goes to.
    fn service(&mut self) -> &mut Service {
        let at = self.channel + if self.text[self.channel] { 2 } else { 0 };
        &mut self.services[at]
    }

    /// A control pair (its first byte as channel 1 names it): a command naming a service, or
    /// what goes to the current one.
    fn control(&mut self, first: u8, second: u8, at: Duration) {
        if matches!(first, 0x14 | 0x15) && (0x20..=0x2F).contains(&second) {
            // (Resume caption loading, roll-up, resume direct captioning; text restart, resume
            // text display.)
            match second {
                0x20 | 0x25..=0x27 | 0x29 => self.text[self.channel] = false,
                0x2A | 0x2B => self.text[self.channel] = true,
                _ => {}
            }
        }
        let service = self.service();
        service.control(first, second);
        service.shown_changed(at);
    }

    /// Starts again at `to` after a seek.
    fn sought(&mut self, to: Duration) {
        for service in &mut self.services {
            let mut recorder = std::mem::take(&mut service.recorder);
            recorder.sought(to);
            *service = Service { recorder, found: service.found, ..Service::default() };
        }
        (self.channel, self.text, self.last_control, self.xds) = (0, [false; 2], None, false);
    }
}

/// A caption or text service: its screens, where the next character goes, and its cues.
struct Service {
    mode: Mode,
    shown: Screen,
    hidden: Screen,
    row: usize,
    column: usize,
    /// Characters from now on are italic.
    italic: bool,
    recorder: CueRecorder,
    /// Whether any data came for it.
    found: bool,
}

impl Default for Service {
    fn default() -> Self {
        Service { mode: Mode::PopOn, shown: blank(), hidden: blank(), row: ROWS - 1, column: 0, italic: false, recorder: CueRecorder::default(), found: false }
    }
}

fn blank() -> Screen {
    [[BLANK; COLUMNS]; ROWS]
}

impl Service {
    /// A control pair for this service (its first byte as channel 1 names it).
    fn control(&mut self, first: u8, second: u8) {
        self.found = true;
        match (first, second) {
            (0x14 | 0x15, 0x20..=0x2F) => self.command(second),
            (0x17, 0x21..=0x23) => self.column = (self.column + (second - 0x20) as usize).min(COLUMNS - 1),
            (0x11, 0x30..=0x3F) => self.put(special(second)),
            (0x12 | 0x13, 0x20..=0x3F) => {
                // An extended character takes the place of the standard one sent before it.
                self.column = self.column.saturating_sub(1);
                self.put(extended(first, second));
            }
            // A mid-row change of colour or style: shown as a space, italics on or off after.
            (0x11, 0x20..=0x2F) => {
                self.put(' ');
                self.italic = (second >> 1) & 7 == 7;
            }
            (0x10..=0x17, 0x40..=0x7F) => self.preamble(first, second),
            _ => {}
        }
    }

    /// A preamble address code: the row and column the next text starts at (a text service
    /// keeps its row), and whether it is italic.
    fn preamble(&mut self, first: u8, second: u8) {
        const ROW_OF: [usize; 8] = [10, 0, 2, 11, 13, 4, 6, 8];
        let row = (ROW_OF[(first & 0x07) as usize] + ((second & 0x20) != 0) as usize).min(ROWS - 1);
        let indented = second & 0x10 != 0;
        let indent = if indented { ((second & 0x0E) >> 1) as usize * 4 } else { 0 };
        self.italic = !indented && (second >> 1) & 7 == 7;
        match self.mode {
            Mode::Text => return self.column = indent,
            // (Roll-up text keeps its rows together: they move to where the base row is now.)
            Mode::RollUp(rows) => self.move_rows(self.row, row, rows),
            _ => {}
        }
        (self.row, self.column) = (row, indent);
    }

    /// The commands of `0x14 0x20` to `0x14 0x2F` (`0x15` in field 2).
    fn command(&mut self, second: u8) {
        match second {
            0x20 => self.mode = Mode::PopOn,
            0x21 => {
                let (row, column) = (self.row, self.column.saturating_sub(1));
                (self.writing()[row][column], self.column) = (BLANK, column);
            }
            0x24 => {
                let (row, column) = (self.row, self.column);
                self.writing()[row][column..].fill(BLANK);
            }
            0x25..=0x27 => self.roll_up(second as usize - 0x23),
            0x29 => self.mode = Mode::PaintOn,
            0x2A => (self.mode, self.shown, self.row, self.column) = (Mode::Text, blank(), 0, 0),
            0x2B if self.mode != Mode::Text => (self.mode, self.row, self.column) = (Mode::Text, 0, 0),
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

    /// A new line: in roll-up the rows move up one, the base row emptied; in a text service
    /// the next row (the rows scroll up from the last); else the column goes back to the start.
    fn carriage_return(&mut self) {
        let (top, base) = match self.mode {
            Mode::RollUp(rows) => ((self.row + 1).saturating_sub(rows), self.row),
            Mode::Text if self.row + 1 < ROWS => return (self.row, self.column) = (self.row + 1, 0),
            Mode::Text => (0, ROWS - 1),
            _ => return self.column = 0,
        };
        for row in top..base {
            self.shown[row] = self.shown[row + 1];
        }
        self.shown[..top].iter_mut().for_each(|row| *row = [BLANK; COLUMNS]);
        (self.shown[base], self.column) = ([BLANK; COLUMNS], 0);
    }

    /// Moves the `rows` roll-up rows ending at `from` to end at `to`.
    fn move_rows(&mut self, from: usize, to: usize, rows: usize) {
        if from == to {
            return;
        }
        let kept: Vec<[Cell; COLUMNS]> = (0..rows).filter_map(|up| from.checked_sub(up)).map(|row| self.shown[row]).collect();
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
        let (row, column, italic) = (self.row, self.column, self.italic);
        self.found = true;
        self.writing()[row][column] = Cell { letter, italic };
        self.column = (column + 1).min(COLUMNS - 1);
    }

    /// What shows may have changed at `at`.
    fn shown_changed(&mut self, at: Duration) {
        self.recorder.show(captions_of(&self.shown), at);
    }
}

/// A row's text where it has any: its first column, the column after its last, and its cells.
type Written<'a> = (usize, (usize, usize), &'a [Cell]);

/// What a screen shows: its rows with text, those that line up (all centred, or starting or
/// ending in the same column) on rows one after another as one caption, from the top.
fn captions_of(screen: &Screen) -> Vec<Caption> {
    let written = screen.iter().enumerate().filter_map(|(row, cells)| {
        let start = cells.iter().position(|cell| cell.letter != ' ')?;
        let end = cells.iter().rposition(|cell| cell.letter != ' ')? + 1;
        Some((row, (start, end), &cells[start..end]))
    });
    let mut groups: Vec<(Vec<Written>, Align)> = Vec::new();
    for row in written {
        let joins = groups.last().and_then(|(rows, _)| joined_alignment(rows, row));
        match (joins, groups.last_mut()) {
            (Some(align), Some(group)) => {
                group.0.push(row);
                group.1 = align;
            }
            _ => groups.push((vec![row], if centred(row.1) { Align::Center } else { Align::Start })),
        }
    }
    groups.into_iter().map(|(rows, align)| caption_of(&rows, align)).collect()
}

/// How `row` lines up with the rows before it, where it comes right after them and lines up
/// with all of them: centred first, then by its start, then by its end.
fn joined_alignment(rows: &[Written], row: Written) -> Option<Align> {
    let next = rows.last().is_some_and(|last| last.0 + 1 == row.0);
    let align = [Align::Center, Align::Start, Align::End].into_iter().find(|align| rows.iter().all(|other| line_up(other.1, row.1, *align)))?;
    next.then_some(align)
}

/// Whether two rows' text (first column, column after the last) lines up this way.
fn line_up(one: (usize, usize), other: (usize, usize), align: Align) -> bool {
    match align {
        Align::Center => centred(one) && centred(other),
        Align::End => one.1 == other.1,
        _ => one.0 == other.0,
    }
}

/// Whether a row's text is centred on the screen (to a column).
fn centred((start, end): (usize, usize)) -> bool {
    (start + end).abs_diff(COLUMNS) <= 1
}

/// Rows one after another as a caption lined up this way: placed where its first row's text
/// starts, its middle or end (as `align`) on the screen; italic where all of it is.
fn caption_of(rows: &[Written], align: Align) -> Caption {
    let column = |(start, end): (usize, usize)| match align {
        Align::Center => (start + end) as f32 / 2.0,
        Align::End => end as f32,
        _ => start as f32,
    };
    let (first_row, first_columns, _) = rows[0];
    let x = MARGIN + SCREEN * column(first_columns) / COLUMNS as f32;
    let place = Place { x, across: align, down: Down::At(MARGIN + SCREEN * first_row as f32 / ROWS as f32, Align::Start), width: SCREEN, lines: align };
    let lines: Vec<String> = rows.iter().map(|(_, _, cells)| cells.iter().map(|cell| cell.letter).collect()).collect();
    let italic = rows.iter().flat_map(|(_, _, cells)| cells.iter()).filter(|cell| cell.letter != ' ').all(|cell| cell.italic);
    Caption { text: lines.join("\n"), place: Some(place), italic }
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

    /// Feeds pairs of field 1, a tenth of a second apart from 0.
    fn fed(pairs: &[[u8; 2]]) -> Decoder {
        let mut decoder = Decoder::default();
        (0..).zip(pairs).for_each(|(tenth, pair)| decoder.read_pair(0, *pair, at(tenth)));
        decoder
    }

    /// The cues of a service, by its name.
    fn cues_of<'a>(decoder: &'a Decoder, name: &str) -> &'a [Cue] {
        let nth = NAMES.iter().position(|known| *known == name).unwrap();
        decoder.services().nth(nth).unwrap().1
    }

    /// CC1's cues.
    fn decoded(pairs: &[[u8; 2]]) -> Vec<Cue> {
        cues_of(&fed(pairs), "CC1").to_vec()
    }

    fn texts(cues: &[Cue]) -> Vec<&str> {
        cues.iter().map(|cue| cue.caption.text.as_str()).collect()
    }

    /// A control pair of channel 1 (`0x14`, or another first byte of it), as sent: twice.
    fn twice(second: u8) -> [[u8; 2]; 2] {
        [[0x94, odd(second)]; 2]
    }
    fn twice_with(first: u8, second: u8) -> [[u8; 2]; 2] {
        [[odd(first), odd(second)]; 2]
    }

    /// Text as pairs.
    fn letters(text: &str) -> Vec<[u8; 2]> {
        let bytes: Vec<u8> = text.bytes().map(odd).collect();
        bytes.chunks(2).map(|two| [two[0], two.get(1).copied().unwrap_or(0x80)]).collect()
    }

    /// A pop-on caption of channel 1 (`first`: 0x14) or 2 (0x1C) at a preamble's row and
    /// indent, shown at once.
    fn pop_on(first: u8, preamble: [u8; 2], text: &str) -> Vec<[u8; 2]> {
        let mut pairs: Vec<[u8; 2]> = [twice_with(first, 0x20), twice_with(first, 0x2E), twice_with(preamble[0] | (first & 0x08), preamble[1])].concat();
        pairs.extend(letters(text));
        pairs.extend(twice_with(first, 0x2F));
        pairs
    }

    fn near(one: f32, other: f32) -> bool {
        (one - other).abs() < 1e-4
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
        let cues = decoded(&pairs);
        assert_eq!(texts(&cues), ["Hello there"]);
        assert_eq!((cues[0].from, cues[0].to), (at(shown), at(erased)));
    }

    #[test]
    fn a_caption_is_placed_at_its_row_and_indent() {
        // (Row 1, indent 8: 0x11 0x54; row 15, indent 0: 0x14 0x70.)
        let place = decoded(&pop_on(0x14, [0x11, 0x54], "Up here"))[0].caption.place.unwrap();
        assert!(near(place.x, 0.1 + 0.8 * 8.0 / 32.0) && place.across == Align::Start, "{place:?}");
        assert_eq!(place.down, Down::At(0.1, Align::Start));
        let place = decoded(&pop_on(0x14, [0x14, 0x70], "Down here"))[0].caption.place.unwrap();
        assert_eq!(place.down, Down::At(0.1 + 0.8 * 14.0 / 15.0, Align::Start));
    }

    #[test]
    fn centred_rows_are_one_centred_caption_and_rows_apart_are_two() {
        // (Row 14 "Hi you" at indent 12 + 1 column (13..19: centred), row 15 "and you" at
        // indent 12 + 1 (13..20: centred to a column).)
        let mut pairs = [twice(0x20), twice(0x2E), twice_with(0x14, 0x56), [[0x97, odd(0x21)]; 2]].concat();
        pairs.extend(letters("Hi you"));
        pairs.extend([twice_with(0x14, 0x76), [[0x97, odd(0x21)]; 2]].concat());
        pairs.extend(letters("and you"));
        pairs.extend(twice(0x2F));
        let cues = decoded(&pairs);
        assert_eq!(texts(&cues), ["Hi you\nand you"]);
        let place = cues[0].caption.place.unwrap();
        assert!(near(place.x, 0.5) && place.across == Align::Center && place.lines == Align::Center, "{place:?}");
        // (Rows 1 and 15 of one caption: two captions, the higher first.)
        let mut pairs = [twice(0x20), twice(0x2E), twice_with(0x11, 0x40)].concat();
        pairs.extend(letters("top"));
        pairs.extend(twice_with(0x14, 0x70));
        pairs.extend(letters("bottom"));
        pairs.extend(twice(0x2F));
        let cues = decoded(&pairs);
        assert_eq!(texts(&cues), ["top", "bottom"]);
        assert_eq!(cues[0].caption.place.unwrap().down, Down::At(0.1, Align::Start));
        assert_eq!(cues[1].caption.place.unwrap().down, Down::At(0.1 + 0.8 * 14.0 / 15.0, Align::Start));
    }

    #[test]
    fn rows_starting_in_one_column_are_one_caption_at_that_column() {
        let mut pairs = [twice(0x29), twice_with(0x14, 0x52)].concat();
        pairs.extend(letters("first row"));
        pairs.extend(twice_with(0x14, 0x72));
        pairs.extend(letters("second"));
        let cues = decoded(&pairs);
        let last = cues.last().unwrap();
        assert_eq!(last.caption.text, "first row\nsecond");
        assert_eq!((last.caption.place.unwrap().across, last.caption.place.unwrap().down), (Align::Start, Down::At(0.1 + 0.8 * 13.0 / 15.0, Align::Start)));
    }

    #[test]
    fn italics_from_a_preamble_or_a_mid_row_code_make_a_caption_italic() {
        // (Row 15 in italics: 0x14 0x6E.)
        let mut pairs = [twice(0x29), twice_with(0x14, 0x6E)].concat();
        pairs.extend(letters("Leaning"));
        assert!(decoded(&pairs).last().unwrap().caption.italic);
        // (A mid-row code back to white: the rest upright, so not all of it italic.)
        pairs.extend(twice_with(0x11, 0x20));
        pairs.extend(letters("up"));
        let last = decoded(&pairs).last().cloned().unwrap();
        assert_eq!((last.caption.text.as_str(), last.caption.italic), ("Leaning up", false));
    }

    #[test]
    fn roll_up_rows_rise_and_each_new_line_is_a_new_cue() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x25), twice(0x2D)].concat();
        pairs.extend(letters("one"));
        pairs.extend(twice(0x2D));
        pairs.extend(letters("two"));
        pairs.extend(twice(0x2D));
        pairs.extend(letters("three"));
        let cues = decoded(&pairs);
        let texts = texts(&cues);
        // (Two rows on show: the oldest goes up and off.)
        assert_eq!(texts.last(), Some(&"two\nthree"));
        assert!(texts.contains(&"one\ntwo"), "{texts:?}");
        assert!(texts.iter().all(|text| text.lines().count() <= 2), "{texts:?}");
        assert_eq!(cues.last().unwrap().caption.place.unwrap().down, Down::At(0.1 + 0.8 * 13.0 / 15.0, Align::Start));
    }

    #[test]
    fn paint_on_text_shows_as_it_is_written_and_backspace_takes_a_letter_back() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Caps"));
        pairs.extend(twice(0x21));
        pairs.extend(letters("t"));
        let cues = decoded(&pairs);
        assert_eq!(texts(&cues).first(), Some(&"Ca"));
        assert_eq!(texts(&cues).last(), Some(&"Capt"));
    }

    #[test]
    fn special_and_extended_characters_are_read() {
        // "Ole" + extended Á over the O, then a note sign, and a standard ñ.
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Ole"));
        pairs.push([0x92, odd(0x20)]);
        pairs.push([0x91, odd(0x37)]);
        pairs.push([odd(0x7E), 0x80]);
        assert_eq!(texts(&decoded(&pairs)).last(), Some(&"OlÁ♪ñ"));
    }

    #[test]
    fn padding_alone_is_no_captions() {
        let mut decoder = Decoder::default();
        decoder.read_cc_data(&[0xFC, 0x80, 0x80, 0xF8, 0x94, 0x20], at(0));
        assert!(decoder.services().all(|(found, cues)| !found && cues.is_empty()));
        decoder.read_pair(0, [0x94, 0x2C], at(1));
        assert_eq!(decoder.services().map(|(found, _)| found).collect::<Vec<_>>(), [true, false, false, false, false, false, false, false]);
    }

    #[test]
    fn after_a_seek_the_caption_on_show_ends_there_and_what_follows_is_read_again() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Painted"));
        let mut decoder = fed(&pairs);
        let first = cues_of(&decoder, "CC1")[0].from;
        decoder.sought(at(7));
        assert!(cues_of(&decoder, "CC1").iter().all(|cue| cue.from < at(7) && cue.to <= at(7)), "{:?}", cues_of(&decoder, "CC1"));
        assert_eq!(cues_of(&decoder, "CC1")[0].from, first);
        // (Read again from the seek: nothing shows until a command says what.)
        decoder.read_pair(0, [odd(b'x'), 0x80], at(8));
        assert_eq!(cues_of(&decoder, "CC1").last().map(|cue| cue.to), Some(at(7)));
    }

    #[test]
    fn data_read_again_from_the_key_frame_before_a_seek_changes_the_captions_from_the_seek() {
        let mut pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat();
        pairs.extend(letters("Painted"));
        let mut decoder = fed(&pairs);
        let played = cues_of(&decoder, "CC1").to_vec();
        decoder.sought(at(5));
        (0..).zip(&pairs).for_each(|(tenth, pair)| decoder.read_pair(0, *pair, at(tenth)));
        assert_eq!(cues_of(&decoder, "CC1"), played);
    }

    #[test]
    fn changes_at_one_moment_make_one_cue() {
        let mut decoder = Decoder::default();
        let pairs: Vec<[u8; 2]> = [twice(0x29), twice(0x70)].concat().into_iter().chain(letters("one two")).collect();
        pairs.iter().for_each(|pair| decoder.read_pair(0, *pair, at(3)));
        let cues = cues_of(&decoder, "CC1");
        assert_eq!((texts(cues), cues[0].from, cues[0].to), (vec!["one two"], at(3), Duration::MAX));
    }

    #[test]
    fn channel_two_is_cc2_and_its_data_leaves_cc1_alone() {
        let decoder = fed(&pop_on(0x1C, [0x14, 0x70], "other"));
        assert!(cues_of(&decoder, "CC1").is_empty());
        assert_eq!(texts(cues_of(&decoder, "CC2")), ["other"]);
        assert_eq!(decoder.services().map(|(found, _)| found).collect::<Vec<_>>(), [false, true, false, false, false, false, false, false]);
    }

    #[test]
    fn field_two_holds_cc3_and_cc4_and_its_commands_use_0x15() {
        let mut decoder = Decoder::default();
        let (cc3, cc4) = (pop_on(0x15, [0x14, 0x70], "third"), pop_on(0x1D, [0x14, 0x70], "fourth"));
        for (tenth, pair) in (0..).zip(cc3.iter().chain(&cc4)) {
            decoder.read_cc_data(&[0xFD, pair[0], pair[1]], at(tenth));
        }
        assert_eq!((texts(cues_of(&decoder, "CC3")), texts(cues_of(&decoder, "CC4"))), (vec!["third"], vec!["fourth"]));
        assert!(cues_of(&decoder, "CC1").is_empty());
    }

    #[test]
    fn a_text_service_fills_rows_from_the_top_and_scrolls() {
        // (Text restart, then lines; resume caption loading goes back to CC1.)
        let mut pairs: Vec<[u8; 2]> = twice(0x2A).to_vec();
        for line in 0..16 {
            pairs.extend(letters(&format!("line {line}")));
            pairs.extend(twice(0x2D));
        }
        pairs.extend(pop_on(0x14, [0x14, 0x70], "caption"));
        let decoder = fed(&pairs);
        let text = cues_of(&decoder, "T1");
        assert_eq!(text[0].caption.text, "li");
        assert_eq!(text[0].caption.place.unwrap().down, Down::At(0.1, Align::Start));
        // (Fifteen rows: the first line has scrolled off by the last.)
        let last = &text.last().unwrap().caption.text;
        assert!(last.starts_with("line 2\n") && last.ends_with("line 15"), "{last:?}");
        assert_eq!(texts(cues_of(&decoder, "CC1")), ["caption"]);
    }

    #[test]
    fn xds_data_in_field_two_is_skipped() {
        let mut decoder = Decoder::default();
        // (An XDS packet: start, data that looks like text, end and checksum; then CC3 text.)
        let mut pairs: Vec<[u8; 2]> = vec![[0x01, 0x03], [odd(b'N'), odd(b'o')], [0x8F, 0x20]];
        pairs.extend(pop_on(0x15, [0x14, 0x70], "Yes"));
        for (tenth, pair) in (0..).zip(&pairs) {
            decoder.read_pair(1, *pair, at(tenth));
        }
        assert_eq!(texts(cues_of(&decoder, "CC3")), ["Yes"]);
    }
}
