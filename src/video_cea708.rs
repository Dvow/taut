//! CEA-708 (DTVCC) captions carried in a video stream's `cc_data` (its triplets of type 2 and
//! 3), made into cues for each caption service (see `video_stream_captions`).
//!
//! Each service has eight windows: defined, shown, hidden, cleared and deleted by its commands,
//! written into where the pen is, in the window's print direction, scrolling and wrapping as it
//! says. Delays hold the commands after them. What shows is each shown window's text, the
//! highest window first, placed at the window's anchor on the caption screen (the middle 80% of
//! the frame), its lines justified as the window says, italic where the pen wrote all of it so.
//! Colours, fonts, sizes, borders and effects are not kept.

use crate::layout::Align;
use crate::video_captions::{Caption, Cue, CueRecorder, Down, Place};
use std::time::Duration;

/// Where the caption screen starts, from the frame's edges, and how much of the frame it is.
const MARGIN: f32 = 0.1;
const SCREEN: f32 = 0.8;

/// A service's input buffer (CEA-708 8.4.2): a delay ends early once this much waits.
const BUFFER: usize = 128;

/// Gathers `cc_data`'s DTVCC packets and hands each service its blocks.
#[derive(Default)]
pub(crate) struct Decoder {
    /// The packet being gathered, its header first, and its whole size.
    packet: Vec<u8>,
    size: usize,
    /// The services, in the order their first data came.
    services: Vec<Service>,
}

impl Decoder {
    /// Reads `cc_data`'s triplets (marker and validity, then two bytes), all at time `at`:
    /// those of DTVCC packets (type 3 starts one, type 2 goes on with it).
    pub(crate) fn read_cc_data(&mut self, triplets: &[u8], at: Duration) {
        self.services.iter_mut().for_each(|service| service.run(at));
        for &[marks, first, second] in triplets.as_chunks::<3>().0 {
            match marks & 0x07 {
                0x07 => self.start_packet([first, second], at),
                0x06 if self.size > 0 => self.packet.extend([first, second]),
                _ => continue,
            }
            if self.size > 0 && self.packet.len() >= self.size {
                self.end_packet(at);
            }
        }
    }

    /// Starts again at `to` after a seek: each service's windows are gone, its cues from there
    /// on read again as it plays.
    pub(crate) fn sought(&mut self, to: Duration) {
        (self.packet, self.size) = (Vec::new(), 0);
        self.services.iter_mut().for_each(|service| service.sought(to));
    }

    /// Each service found, by its number, and its cues so far, in the order found.
    pub(crate) fn services(&self) -> impl Iterator<Item = (u8, &[Cue])> {
        self.services.iter().map(|service| (service.number, service.recorder.cues()))
    }

    /// A packet starts with this header (sequence number, and its size in pairs: 0 for 64); one
    /// cut short before it is read as far as it came.
    fn start_packet(&mut self, [header, first]: [u8; 2], at: Duration) {
        if self.size > 0 {
            self.end_packet(at);
        }
        let pairs = match header & 0x3F {
            0 => 64,
            pairs => pairs as usize,
        };
        (self.packet, self.size) = (vec![header, first], pairs * 2);
    }

    /// Hands each service block of the packet gathered to its service.
    fn end_packet(&mut self, at: Duration) {
        let packet = std::mem::take(&mut self.packet);
        let mut rest = packet.get(1..self.size.min(packet.len())).unwrap_or_default();
        self.size = 0;
        while let Some((number, block, after)) = service_block(rest) {
            self.service(number).read(block, at);
            rest = after;
        }
    }

    /// The service of this number, found now if not before.
    fn service(&mut self, number: u8) -> &mut Service {
        let at = match self.services.iter().position(|service| service.number == number) {
            Some(at) => at,
            None => {
                self.services.push(Service::new(number));
                self.services.len() - 1
            }
        };
        &mut self.services[at]
    }
}

/// The first service block of a packet's data: its service's number, its bytes, and what
/// follows; none at a null block (the padding that ends a packet) or the data's end.
fn service_block(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&header, rest) = data.split_first()?;
    let (number, size) = (header >> 5, (header & 0x1F) as usize);
    let (number, rest) = match number {
        0 => return None,
        7 => rest.split_first().map(|(extended, rest)| (extended & 0x3F, rest))?,
        number => (number, rest),
    };
    let (block, after) = rest.split_at_checked(size).unwrap_or((rest, &[]));
    Some((number, block, after))
}

/// One caption service: its windows, its commands waiting, and its cues.
struct Service {
    number: u8,
    screen: Screen,
    /// Commands not acted on yet: those a delay holds, and any cut off at a block's end.
    input: Vec<u8>,
    /// A delay holds `input` until then (or a DelayCancel or Reset comes, or the buffer fills).
    delay_until: Option<Duration>,
    recorder: CueRecorder,
}

impl Service {
    fn new(number: u8) -> Service {
        Service { number, screen: Screen::default(), input: Vec::new(), delay_until: None, recorder: CueRecorder::default() }
    }

    /// Takes in a service block at `at`, and acts on what it can.
    fn read(&mut self, block: &[u8], at: Duration) {
        self.input.extend_from_slice(block);
        self.run(at);
    }

    /// Acts on the commands waiting, at `at`, as far as a delay lets: those a delay held from
    /// its end (or now, where it is cancelled).
    fn run(&mut self, at: Duration) {
        loop {
            let from = match self.delay_until {
                // (Nothing to act on: the text stays as it is.)
                None if self.input.is_empty() => return,
                None => at,
                Some(until) if until <= at => until,
                Some(_) if self.delay_cancelled() => at,
                Some(_) => return,
            };
            self.delay_until = None;
            self.act(from);
            self.recorder.show(self.screen.captions(), from);
            if self.delay_until.is_none() {
                return;
            }
        }
    }

    /// Acts on the whole commands waiting, at `at`, up to a Delay.
    fn act(&mut self, at: Duration) {
        let mut done = 0;
        while let Some(length) = command_length(&self.input[done..]) {
            let command = &self.input[done..done + length];
            done += length;
            if let [0x8D, tenths] = *command {
                self.delay_until = Some(at + Duration::from_millis(tenths as u64 * 100));
                break;
            }
            self.screen.act(command);
        }
        self.input.drain(..done);
    }

    /// Whether a DelayCancel or Reset waits behind the delay, or the buffer is full.
    fn delay_cancelled(&self) -> bool {
        let mut at = 0;
        while let Some(length) = command_length(&self.input[at..]) {
            if matches!(self.input[at], 0x8E | 0x8F) {
                return true;
            }
            at += length;
        }
        self.input.len() >= BUFFER
    }

    /// Starts again at `to` after a seek.
    fn sought(&mut self, to: Duration) {
        (self.screen, self.delay_until) = (Screen::default(), None);
        self.input.clear();
        self.recorder.sought(to);
    }
}

/// A command's length (code and parameters), where all of it is there.
fn command_length(bytes: &[u8]) -> Option<usize> {
    let length = match *bytes.first()? {
        0x10 => 1 + extended_length(&bytes[1..])?,
        0x11..=0x17 | 0x88..=0x8D => 2,
        0x18..=0x1F | 0x90 | 0x92 => 3,
        0x91 => 4,
        0x97 => 5,
        0x98..=0x9F => 7,
        _ => 1,
    };
    (bytes.len() >= length).then_some(length)
}

/// The length of what follows EXT1: an extended code and its parameters (C2, G2, C3, G3).
fn extended_length(bytes: &[u8]) -> Option<usize> {
    Some(match *bytes.first()? {
        0x08..=0x0F => 2,
        0x10..=0x17 => 3,
        0x18..=0x1F => 4,
        0x80..=0x87 => 5,
        0x88..=0x8F => 6,
        // (Variable length: a header byte, its low 5 bits the length, then that many.)
        0x90..=0x9F => 2 + (*bytes.get(1)? & 0x1F) as usize,
        _ => 1,
    })
}

/// A direction text is written or scrolled in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Direction {
    LeftToRight,
    RightToLeft,
    TopToBottom,
    BottomToTop,
}

impl Direction {
    fn of(bits: u8) -> Direction {
        [Direction::LeftToRight, Direction::RightToLeft, Direction::TopToBottom, Direction::BottomToTop][(bits & 3) as usize]
    }
    /// One step this way, in rows and columns.
    fn step(self) -> (isize, isize) {
        match self {
            Direction::LeftToRight => (0, 1),
            Direction::RightToLeft => (0, -1),
            Direction::TopToBottom => (1, 0),
            Direction::BottomToTop => (-1, 0),
        }
    }
    fn across(self) -> bool {
        matches!(self, Direction::LeftToRight | Direction::RightToLeft)
    }
}

/// A service's windows, and the one written into.
#[derive(Default)]
struct Screen {
    windows: [Window; 8],
    current: Option<usize>,
}

impl Screen {
    /// Acts on one whole command (not a Delay).
    fn act(&mut self, command: &[u8]) {
        match *command {
            [0x10, code, ..] => self.extended(code),
            [0x08] => self.window(Window::backspace),
            [0x0C] => self.window(Window::form_feed),
            [0x0D] => self.window(Window::new_line),
            [0x0E] => self.window(Window::clear_line),
            [0x18, high, low] => self.put(char::from_u32(u32::from(high) << 8 | u32::from(low)).unwrap_or('\u{FFFD}')),
            [0x7F] => self.put('♪'),
            [code @ (0x20..=0x7E | 0xA0..=0xFF)] => self.put(char::from(code)),
            [code @ 0x80..=0x87] => self.current = Some((code & 7) as usize).filter(|at| self.windows[*at].defined),
            [code @ 0x88..=0x8C, chosen] => self.windows_command(code, chosen),
            [0x8F] => *self = Screen::default(),
            [0x90, tag_offset_size, italic_underline_edge_font] => self.window(|window| (window.hides_text, window.italic) = (tag_offset_size >> 4 == 15, italic_underline_edge_font & 0x80 != 0)),
            [0x92, row, column] => self.window(|window| window.move_pen((row & 0x0F) as usize, (column & 0x3F) as usize)),
            [0x97, _, _, layout, _] => self.window(|window| window.set_layout(layout)),
            [code @ 0x98..=0x9F, ref parameters @ ..] => self.define((code & 7) as usize, parameters),
            _ => {}
        }
    }

    /// An extended code: a G2 character (G3's are icons, C2's and C3's do nothing yet).
    fn extended(&mut self, code: u8) {
        if let Some(letter) = g2(code) {
            self.put(letter);
        }
    }

    /// Does this to the window written into, if any. (Only a defined window is ever written into
    /// or shown.)
    fn window(&mut self, change: impl FnOnce(&mut Window)) {
        if let Some(at) = self.current {
            change(&mut self.windows[at]);
        }
    }

    fn put(&mut self, letter: char) {
        self.window(|window| window.put(letter));
    }

    /// Clears, shows, hides, toggles or deletes the windows of `chosen`'s bits.
    fn windows_command(&mut self, code: u8, chosen: u8) {
        for (at, window) in self.windows.iter_mut().enumerate().filter(|(at, _)| chosen >> at & 1 != 0) {
            match code {
                0x88 => window.clear(),
                0x89 => window.visible = window.defined,
                0x8A => window.visible = false,
                0x8B => window.visible = window.defined && !window.visible,
                _ => *window = Window::default(),
            }
            if code == 0x8C && self.current == Some(at) {
                self.current = None;
            }
        }
    }

    /// DefineWindow: makes or changes window `at`, and writes into it from now.
    fn define(&mut self, at: usize, parameters: &[u8]) {
        let &[flags, vertical, horizontal, anchor_rows, columns, styles] = parameters else { return };
        let window = &mut self.windows[at];
        let new = !window.defined;
        window.resize((anchor_rows & 0x0F) as usize + 1, (columns & 0x3F) as usize + 1);
        window.visible = flags & 0x20 != 0;
        window.place(vertical, horizontal, anchor_rows >> 4);
        let style = (styles >> 3) & 7;
        if new || style != 0 {
            window.set_style(style);
        }
        if new || styles & 7 != 0 {
            (window.hides_text, window.italic) = (false, false);
        }
        if new {
            window.pen = window.origin();
        }
        self.current = Some(at);
    }

    /// What shows: each shown window with text, the highest (then leftmost) first.
    fn captions(&self) -> Vec<Caption> {
        let mut shown: Vec<&Window> = self.windows.iter().filter(|window| window.visible).collect();
        shown.sort_by(|one, other| one.top.total_cmp(&other.top).then(one.left.total_cmp(&other.left)));
        shown.iter().map(|window| window.caption()).filter(|caption| !caption.text.is_empty()).collect()
    }
}

/// A G2 character (after EXT1): the signs and letters CEA-708 gives; none where it gives none.
fn g2(code: u8) -> Option<char> {
    Some(match code {
        0x20 => ' ',
        0x21 => '\u{A0}',
        0x25 => '…',
        0x2A => 'Š',
        0x2C => 'Œ',
        0x30 => '█',
        0x31 => '‘',
        0x32 => '’',
        0x33 => '“',
        0x34 => '”',
        0x35 => '•',
        0x39 => '™',
        0x3A => 'š',
        0x3C => 'œ',
        0x3D => '℠',
        0x3F => 'Ÿ',
        0x76 => '⅛',
        0x77 => '⅜',
        0x78 => '⅝',
        0x79 => '⅞',
        0x7A => '│',
        0x7B => '┐',
        0x7C => '└',
        0x7D => '─',
        0x7E => '┘',
        0x7F => '┌',
        _ => return None,
    })
}

/// A place in a window: a character, and whether the pen wrote it italic.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cell {
    letter: char,
    italic: bool,
}

const BLANK: Cell = Cell { letter: ' ', italic: false };

/// A window: its text, where the pen is, how text runs in it, and where it is on the screen.
struct Window {
    defined: bool,
    visible: bool,
    rows: usize,
    columns: usize,
    /// Its cells, row by row.
    cells: Vec<Cell>,
    /// The pen: row and column.
    pen: (usize, usize),
    /// The pen wrote the last cell of its line: the next letter starts a new line (with word
    /// wrap) or writes over it.
    at_line_end: bool,
    print: Direction,
    scroll: Direction,
    word_wrap: bool,
    /// How its lines line up.
    justify: Align,
    /// The pen's text tag says its text is not to be shown; the pen writes italic.
    hides_text: bool,
    italic: bool,
    /// Where its anchor is, as parts of the caption screen (across, down), and which of its
    /// points that is (0 top left to 8 bottom right).
    anchor: (f32, f32),
    anchor_point: u8,
    /// Where its top and left are, as parts of the caption screen (for the order of windows).
    top: f32,
    left: f32,
}

impl Default for Window {
    fn default() -> Self {
        Window {
            defined: false,
            visible: false,
            rows: 0,
            columns: 0,
            cells: Vec::new(),
            pen: (0, 0),
            at_line_end: false,
            print: Direction::LeftToRight,
            scroll: Direction::BottomToTop,
            word_wrap: false,
            justify: Align::Start,
            hides_text: false,
            italic: false,
            anchor: (0.0, 0.0),
            anchor_point: 0,
            top: 0.0,
            left: 0.0,
        }
    }
}

impl Window {
    /// Gives it this many rows and columns (1 to 16, 1 to 64), keeping the text that fits and
    /// the pen in it.
    fn resize(&mut self, rows: usize, columns: usize) {
        let mut cells = vec![BLANK; rows * columns];
        for row in 0..rows.min(self.rows) {
            let kept = columns.min(self.columns);
            cells[row * columns..row * columns + kept].copy_from_slice(&self.cells[row * self.columns..row * self.columns + kept]);
        }
        (self.defined, self.rows, self.columns, self.cells) = (true, rows, columns, cells);
        self.pen = (self.pen.0.min(rows - 1), self.pen.1.min(columns - 1));
        self.at_line_end = false;
    }

    /// Where it is: its anchor (in 75 rows by 210 columns, or in hundredths where relative)
    /// and which of its points that is (0 top left to 8 bottom right).
    fn place(&mut self, vertical: u8, horizontal: u8, anchor_point: u8) {
        let relative = vertical & 0x80 != 0;
        let (down, across) = ((vertical & 0x7F) as f32, horizontal as f32);
        let (down, across) = if relative { (down / 100.0, across / 100.0) } else { (down / 75.0, across / 210.0) };
        let anchor_point = anchor_point.min(8);
        let (above, beside) = ((anchor_point / 3) as f32 / 2.0, (anchor_point % 3) as f32 / 2.0);
        (self.anchor, self.anchor_point) = ((across.min(1.0), down.min(1.0)), anchor_point);
        // (A row is a 15th of the screen's height, a column a 42nd of its width.)
        (self.top, self.left) = (down - above * self.rows as f32 / 15.0, across - beside * self.columns as f32 / 42.0);
    }

    /// A predefined window style (1 to 7; 0 is 1): 1 to 6 write left to right and scroll up,
    /// 4 to 6 wrapping words; 7, a ticker, writes top to bottom and scrolls right to left. 2
    /// and 5 centre their lines.
    fn set_style(&mut self, style: u8) {
        let ticker = style == 7;
        self.print = if ticker { Direction::TopToBottom } else { Direction::LeftToRight };
        self.scroll = if ticker { Direction::RightToLeft } else { Direction::BottomToTop };
        self.word_wrap = (4..=6).contains(&style);
        self.justify = if matches!(style, 2 | 5) { Align::Center } else { Align::Start };
    }

    /// SetWindowAttributes' third byte: word wrap, print and scroll direction, justification
    /// (left, right, centre, full: full as left).
    fn set_layout(&mut self, layout: u8) {
        (self.word_wrap, self.print, self.scroll) = (layout & 0x40 != 0, Direction::of(layout >> 4), Direction::of(layout >> 2));
        self.justify = [Align::Start, Align::End, Align::Center, Align::Start][usize::from(layout & 3)];
        self.at_line_end = false;
    }

    /// The way from one line to the next: across the print direction, against the scroll
    /// (where the two lie the same way, as print left to right and scroll up).
    fn line_step(&self) -> (isize, isize) {
        match (self.print.across(), self.scroll) {
            (true, Direction::TopToBottom) => (-1, 0),
            (true, _) => (1, 0),
            (false, Direction::LeftToRight) => (0, -1),
            (false, _) => (0, 1),
        }
    }

    /// The cell after `cell` going `step`, if in the window.
    fn after(&self, (row, column): (usize, usize), (down, across): (isize, isize)) -> Option<(usize, usize)> {
        let (row, column) = (row.checked_add_signed(down)?, column.checked_add_signed(across)?);
        (row < self.rows && column < self.columns).then_some((row, column))
    }

    /// The first cell of the line `cell` is on, in the print direction.
    fn line_start(&self, (row, column): (usize, usize)) -> (usize, usize) {
        match self.print {
            Direction::LeftToRight => (row, 0),
            Direction::RightToLeft => (row, self.columns - 1),
            Direction::TopToBottom => (0, column),
            Direction::BottomToTop => (self.rows - 1, column),
        }
    }

    /// Where writing starts in a cleared window: the start of its first line.
    fn origin(&self) -> (usize, usize) {
        let (down, across) = self.line_step();
        let first = (if down < 0 { self.rows - 1 } else { 0 }, if across < 0 { self.columns - 1 } else { 0 });
        self.line_start(first)
    }

    /// The cells of the line `cell` is on, in the print direction.
    fn line(&self, cell: (usize, usize)) -> Vec<(usize, usize)> {
        let start = self.line_start(cell);
        std::iter::successors(Some(start), |cell| self.after(*cell, self.print.step())).collect()
    }

    fn cell(&mut self, (row, column): (usize, usize)) -> &mut Cell {
        &mut self.cells[row * self.columns + column]
    }

    fn clear(&mut self) {
        self.cells.fill(BLANK);
        self.at_line_end = false;
    }

    /// SetPenLocation, kept in the window.
    fn move_pen(&mut self, row: usize, column: usize) {
        (self.pen, self.at_line_end) = ((row.min(self.rows - 1), column.min(self.columns - 1)), false);
    }

    /// Writes a letter where the pen is and moves it on; at the line's end, a word that does
    /// not fit goes to the next line where words wrap.
    fn put(&mut self, letter: char) {
        if std::mem::take(&mut self.at_line_end) && self.word_wrap {
            self.wrap(letter);
            if letter == ' ' {
                return;
            }
        }
        *self.cell(self.pen) = if self.hides_text { BLANK } else { Cell { letter, italic: self.italic } };
        match self.after(self.pen, self.print.step()) {
            Some(next) => self.pen = next,
            None => self.at_line_end = true,
        }
    }

    /// Goes on to the next line before writing `letter`: the word the line ends in comes
    /// along (unless it fills the line, or a space comes next).
    fn wrap(&mut self, letter: char) {
        let line = self.line(self.pen);
        let word = line.iter().rev().take_while(|cell| self.cells[cell.0 * self.columns + cell.1].letter != ' ').count();
        let moved = if letter == ' ' || word == line.len() { Vec::new() } else { line[line.len() - word..].to_vec() };
        let letters: Vec<Cell> = moved.iter().map(|cell| std::mem::replace(self.cell(*cell), BLANK)).collect();
        self.new_line();
        for letter in letters {
            *self.cell(self.pen) = letter;
            self.pen = self.after(self.pen, self.print.step()).unwrap_or(self.pen);
        }
    }

    /// Carriage return: the pen goes to the start of the next line, the text scrolling where it
    /// is on the last.
    fn new_line(&mut self) {
        self.at_line_end = false;
        let step = self.line_step();
        if let Some(next) = self.after(self.pen, step) {
            self.pen = self.line_start(next);
            return;
        }
        self.scroll(step);
        self.pen = self.line_start(self.pen);
    }

    /// Moves every line one back against `step` (the first goes), the last one left empty.
    fn scroll(&mut self, (down, across): (isize, isize)) {
        let old = std::mem::replace(&mut self.cells, vec![BLANK; self.rows * self.columns]);
        for (row, column) in (0..self.rows).flat_map(|row| (0..self.columns).map(move |column| (row, column))) {
            if let Some((from_row, from_column)) = self.after((row, column), (down, across)) {
                self.cells[row * self.columns + column] = old[from_row * self.columns + from_column];
            }
        }
    }

    /// Horizontal carriage return: the pen's line emptied, the pen at its start.
    fn clear_line(&mut self) {
        for cell in self.line(self.pen) {
            *self.cell(cell) = BLANK;
        }
        (self.pen, self.at_line_end) = (self.line_start(self.pen), false);
    }

    /// Backspace: the letter before the pen taken back (the last one, at the line's end).
    fn backspace(&mut self) {
        let back = (-self.print.step().0, -self.print.step().1);
        if !std::mem::take(&mut self.at_line_end) {
            let Some(before) = self.after(self.pen, back) else { return };
            self.pen = before;
        }
        *self.cell(self.pen) = BLANK;
    }

    /// Form feed: the window emptied, the pen at its start.
    fn form_feed(&mut self) {
        self.clear();
        self.pen = self.origin();
    }

    /// What it shows: each line with anything on it, in the order lines are written, each read
    /// in the print direction and trimmed; placed at its anchor on the caption screen, lined up
    /// as it says; italic where all of it is.
    fn caption(&self) -> Caption {
        let step = self.line_step();
        let first = self.line_start(self.origin());
        let lines: Vec<Vec<Cell>> = std::iter::successors(Some(first), |cell| self.after(*cell, step).map(|next| self.line_start(next)))
            .map(|start| self.line(start).iter().map(|(row, column)| self.cells[row * self.columns + column]).collect())
            .collect();
        let read = lines.iter().map(|line| line.iter().map(|cell| cell.letter).collect::<String>().trim().to_owned());
        let text = read.filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n");
        let italic = lines.iter().flatten().filter(|cell| cell.letter != ' ').all(|cell| cell.italic);
        let sides = [Align::Start, Align::Center, Align::End];
        let (across, down) = (sides[usize::from(self.anchor_point % 3)], sides[usize::from(self.anchor_point / 3)]);
        let place = Place { x: MARGIN + SCREEN * self.anchor.0, across, down: Down::At(MARGIN + SCREEN * self.anchor.1, down), width: SCREEN, lines: self.justify };
        Caption { text, place: Some(place), italic: italic && lines.iter().flatten().any(|cell| cell.letter != ' ') }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }

    fn at(tenths: u64) -> Duration {
        Duration::from_millis(tenths * 100)
    }

    /// A packet's `cc_data` triplets: the first of type 3, the rest of type 2.
    fn triplets(packet: &[u8]) -> Vec<u8> {
        packet.chunks(2).enumerate().flat_map(|(nth, pair)| [if nth == 0 { 0xFF } else { 0xFE }, pair[0], pair[1]]).collect()
    }

    /// A decoder fed one block of service 1 a tenth of a second, from 0.
    fn fed(blocks: &[&[u8]]) -> Decoder {
        let mut decoder = Decoder::default();
        for (tenth, block) in (0..).zip(blocks) {
            decoder.read_cc_data(&triplets(&files::cea708_packet(tenth as u8, 1, block)), at(tenth));
        }
        decoder
    }

    fn cues(decoder: &Decoder, service: u8) -> Vec<(Duration, Duration, String)> {
        let (_, cues) = decoder.services().find(|(number, _)| *number == service).expect("the service");
        cues.iter().map(|cue| (cue.from, cue.to, cue.caption.text.clone())).collect()
    }

    /// Service 1's text on show at each change: its windows' cues starting then, a line each.
    fn texts(decoder: &Decoder) -> Vec<String> {
        let mut texts: Vec<(Duration, String)> = Vec::new();
        for (from, _, text) in cues(decoder, 1) {
            match texts.last_mut() {
                Some((at, all)) if *at == from => *all = format!("{all}\n{text}"),
                _ => texts.push((from, text)),
            }
        }
        texts.into_iter().map(|(_, text)| text).collect()
    }

    /// Service 1's last captions.
    fn last(decoder: &Decoder) -> Vec<Caption> {
        let (_, cues) = decoder.services().find(|(number, _)| *number == 1).expect("the service");
        cues.iter().filter(|cue| cue.to == Duration::MAX).map(|cue| cue.caption.clone()).collect()
    }

    fn near(one: f32, other: f32) -> bool {
        (one - other).abs() < 1e-4
    }

    #[test]
    fn a_window_is_placed_at_its_anchor_on_the_caption_screen() {
        // (Anchored by its bottom middle (point 7) at 90% down, 50% across.)
        let window = [0x98, 0x20, 0x80 | 90, 50, 0x70, 31, 1 << 3 | 1];
        let place = last(&fed(&[&[&window[..], b"Low"].concat()]))[0].place.unwrap();
        assert!(near(place.x, 0.1 + 0.8 * 0.5) && place.across == Align::Center, "{place:?}");
        assert_eq!(place.down, Down::At(0.1 + 0.8 * 0.9, Align::End));
        // (Anchored by its top left in the grid of 75 rows by 210 columns.)
        let window = [0x98, 0x20, 15, 42, 0x00, 31, 1 << 3 | 1];
        let place = last(&fed(&[&[&window[..], b"High"].concat()]))[0].place.unwrap();
        assert!(near(place.x, 0.1 + 0.8 * 0.2) && place.across == Align::Start, "{place:?}");
        assert_eq!(place.down, Down::At(0.1 + 0.8 * 0.2, Align::Start));
    }

    #[test]
    fn a_windows_justification_and_style_line_up_its_lines() {
        let centred = last(&fed(&[&[&define(0, true, 1, 80, 2)[..], b"mid"].concat()]))[0].place.unwrap();
        assert_eq!(centred.lines, Align::Center);
        // (SetWindowAttributes: right justified.)
        let right = last(&fed(&[&define(0, true, 1, 80, 1), &[0x97, 0, 0, 0x0D, 0], b"right"]))[0].place.unwrap();
        assert_eq!(right.lines, Align::End);
    }

    #[test]
    fn text_the_pen_writes_italic_makes_an_italic_caption() {
        let italic = last(&fed(&[&define(0, true, 1, 80, 1), &[0x90, 0x05, 0x80], b"lean"]));
        assert!(italic[0].italic, "{italic:?}");
        let mixed = last(&fed(&[&define(0, true, 1, 80, 1), &[0x90, 0x05, 0x80], b"lean", &[0x90, 0x05, 0x00], b" up"]));
        assert!(!mixed[0].italic && mixed[0].text == "lean up", "{mixed:?}");
    }

    /// DefineWindow 0 to 7 (`at`): shown or not, `rows` by 32, anchored at its top left this
    /// far down (hundredths), in window style `style`.
    fn define(at: u8, shown: bool, rows: u8, down: u8, style: u8) -> [u8; 7] {
        [0x98 | at, if shown { 0x20 } else { 0 }, 0x80 | down, 0, rows - 1, 31, style << 3 | 1]
    }

    #[test]
    fn a_hidden_window_written_then_shown_is_a_cue_until_it_is_deleted() {
        let loading = [&define(0, false, 1, 90, 1)[..], b"Hello there"].concat();
        let decoder = fed(&[&loading, &[], &[0x89, 0x01], &[], &[], &[0x8C, 0x01]]);
        assert_eq!(cues(&decoder, 1), [(at(2), at(5), "Hello there".into())]);
    }

    #[test]
    fn text_written_into_a_shown_window_shows_as_it_is_written() {
        let decoder = fed(&[&define(0, true, 1, 90, 1), b"Cap", b"tion", &[0x88, 0x01]]);
        assert_eq!(cues(&decoder, 1), [(at(1), at(2), "Cap".into()), (at(2), at(3), "Caption".into())]);
    }

    #[test]
    fn a_delay_holds_the_commands_after_it_until_its_time() {
        let mut decoder = fed(&[&[&define(0, false, 1, 90, 1)[..], b"Later", &[0x8D, 5, 0x89, 0x01]].concat()]);
        assert!(cues(&decoder, 1).is_empty());
        // (Time passes with other data: the delay ends at 0.5 s, not when the data came.)
        decoder.read_cc_data(&[0xFC, 0x80, 0x80], at(9));
        assert_eq!(cues(&decoder, 1), [(at(5), Duration::MAX, "Later".into())]);
    }

    #[test]
    fn a_delay_cancel_or_a_reset_ends_a_delay_at_once() {
        let held = [&define(0, false, 1, 90, 1)[..], b"Now", &[0x8D, 50, 0x89, 0x01]].concat();
        let cancelled = fed(&[&held, &[0x8E]]);
        assert_eq!(cues(&cancelled, 1), [(at(1), Duration::MAX, "Now".into())]);
        let reset = fed(&[&held, &[0x8F], b"lost"]);
        assert!(cues(&reset, 1).is_empty(), "{:?}", cues(&reset, 1));
    }

    #[test]
    fn carriage_returns_scroll_a_window_of_two_rows_up() {
        let decoder = fed(&[&define(0, true, 2, 80, 4), b"one\rtwo", b"\rthree"]);
        assert_eq!(texts(&decoder), ["one\ntwo", "two\nthree"]);
    }

    #[test]
    fn a_word_that_does_not_fit_wraps_whole_where_words_wrap() {
        let narrow = [0x98, 0x20, 0x80 | 80, 0, 1, 9, 4 << 3 | 1];
        let decoder = fed(&[&[&narrow[..], b"one two three"].concat()]);
        assert_eq!(texts(&decoder), ["one two\nthree"]);
        // (Without wrapping, the last column is written over.)
        let decoder = fed(&[&[&[narrow[0], narrow[1], narrow[2], 0, 1, 9, 1 << 3 | 1][..], b"one two three"].concat()]);
        assert_eq!(texts(&decoder), ["one two te"]);
    }

    #[test]
    fn backspace_form_feed_and_horizontal_carriage_return_take_text_back() {
        let decoder = fed(&[&define(0, true, 2, 80, 1), b"abc\x08d", b"\rgone", b"\x0Ekept", b"\x0Cnew"]);
        assert_eq!(texts(&decoder), ["abd", "abd\ngone", "abd\nkept", "new"]);
    }

    #[test]
    fn the_pen_moves_where_it_is_set_and_its_hidden_text_is_not_shown() {
        let decoder = fed(&[&define(0, true, 3, 50, 1), &[0x92, 2, 4], b"low", &[0x90, 0xF0, 0], b"secret", &[0x90, 0x00, 0, 0x92, 0, 0], b"top"]);
        assert_eq!(texts(&decoder).last().map(String::as_str), Some("top\nlow"));
    }

    #[test]
    fn the_text_of_shown_windows_is_read_from_the_highest() {
        let decoder = fed(&[&[&define(1, true, 1, 80, 1)[..], b"lower"].concat(), &[&define(2, true, 1, 10, 1)[..], b"upper"].concat(), &[0x8A, 0x04]]);
        assert_eq!(texts(&decoder), ["lower", "upper\nlower", "lower"]);
    }

    #[test]
    fn right_to_left_and_ticker_windows_are_read_as_written() {
        // (SetWindowAttributes: print right to left, scroll up; then a form feed takes the pen
        // to the right.)
        let right_to_left = [0x97, 0, 0, 0x1C, 0, 0x0C];
        let decoder = fed(&[&define(0, true, 1, 80, 1), &right_to_left, b"abc"]);
        assert_eq!(texts(&decoder), ["abc"]);
        let decoder = fed(&[&define(0, true, 4, 80, 7), b"ab\rcd"]);
        assert_eq!(texts(&decoder).last().map(String::as_str), Some("ab\ncd"));
    }

    #[test]
    fn extended_and_sixteen_bit_characters_and_the_music_note_are_read() {
        let decoder = fed(&[&define(0, true, 1, 80, 1), &[0x10, 0x39, 0x7F, 0xE9, 0x18, 0xAC, 0x00, 0x10, 0x90, 0x02, 1, 2, 0x10, 0x3C]]);
        assert_eq!(texts(&decoder).last().map(String::as_str), Some("™♪é가œ"));
    }

    #[test]
    fn services_are_listed_as_found_and_numbers_past_six_are_read() {
        let mut decoder = Decoder::default();
        let window = [&define(0, true, 1, 80, 1)[..], b"x"].concat();
        for (tenth, service) in [(0, 9), (1, 1), (2, 9)] {
            decoder.read_cc_data(&triplets(&files::cea708_packet(tenth as u8, service, &window)), at(tenth));
        }
        assert_eq!(decoder.services().map(|(number, _)| number).collect::<Vec<_>>(), [9, 1]);
        assert_eq!(cues(&decoder, 9)[0].2, "x");
    }

    #[test]
    fn a_packet_spread_over_several_reads_or_cut_short_is_read() {
        let packet = files::cea708_packet(0, 1, &[&define(0, true, 1, 80, 1)[..], b"spread"].concat());
        let all = triplets(&packet);
        let mut decoder = Decoder::default();
        for (tenth, part) in (0..).zip(all.chunks(6)) {
            decoder.read_cc_data(part, at(tenth));
        }
        assert_eq!(texts(&decoder), ["spread"]);
        // (Cut short by the next packet's start: read as far as it came.)
        let mut decoder = Decoder::default();
        decoder.read_cc_data(&all[..all.len() - 3], at(0));
        decoder.read_cc_data(&triplets(&files::cea708_packet(1, 2, b"other")), at(1));
        assert_eq!(cues(&decoder, 1), [(at(1), Duration::MAX, "sprea".into())]);
    }

    #[test]
    fn after_a_seek_the_cue_on_show_ends_there_and_what_follows_is_read_again() {
        let blocks: [&[u8]; 4] = [&define(0, true, 1, 80, 1), b"one", &[0x88, 0x01], b"two"];
        let mut decoder = fed(&blocks);
        decoder.sought(at(2));
        assert_eq!(cues(&decoder, 1), [(at(1), at(2), "one".into())]);
        // (Read again from the key frame before the seek: the window is defined again.)
        for (tenth, block) in (0..).zip(&blocks) {
            decoder.read_cc_data(&triplets(&files::cea708_packet(tenth as u8, 1, block)), at(tenth));
        }
        assert_eq!(cues(&decoder, 1), [(at(1), at(2), "one".into()), (at(3), Duration::MAX, "two".into())]);
    }
}
