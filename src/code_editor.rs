//! Code editor: source code, config files, anything line-based.
//! [`ui.code_editor(&mut code, "Source")`](Ui::code_editor), where `code` is a [`CodeEditor`]
//! the app keeps: the text and the editing state (cursors, undo, folds).
//!
//! Fast at any file size. The text is one `String`, with each line's start offset kept beside
//! it: an edit re-finds the lines it touched and shifts the rest. Only visible rows are
//! described (as [`Ui::rows`] does for a large list), a box per run of one colour. Colouring a
//! line needs what was left open before it (a block comment, a multi-line string): that is kept
//! per line, computed only up to the visible lines, and after an edit recomputed only until it
//! matches the old value again.
//!
//! A language's syntax is plain data ([`Syntax`]): keywords, type names, comment and string
//! syntax. A few languages are built in; an app writes rules for others the same way.
use crate::color::Color;
use crate::event::{Cursor, Key, Mods};
use crate::id::Id;
use crate::layout::{Align, Flow, Length};
use crate::platform::Clipboard;
use crate::text::{Font, cluster};
use crate::text_edit::{in_word, step, word, word_edge};
use crate::theme::Theme;
use crate::ui::{Icon, Response, Stroke, Ui};
use crate::widgets::{CROSS, DOWN, RIGHT};
use std::ops::Range;
use std::time::Duration;

/// Undo depth.
const HISTORY: usize = 500;
/// An edit at more places than this re-indexes every line, not line by line.
const MANY: usize = 64;
/// How many lines away a matching bracket is searched for.
const PAIRED_WITHIN: usize = 2000;
/// Line-end state: inside a block comment.
const IN_COMMENT: u8 = 1;
/// Opening brackets, each with its closing one.
const BRACKETS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];
/// The up arrow, for the find-previous button.
const UP: Icon = Icon::line("M4 9.5 8 5.5 12 9.5");

/// A code token's kind, for its colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Identifiers, punctuation, and anything else.
    Plain,
    /// A keyword: `fn`, `if`, `return`.
    Keyword,
    /// A type name: `u32`, `String`.
    Type,
    /// A number: `42`, `0xff`, `1.5`.
    Number,
    /// A string literal.
    String,
    /// A comment.
    Comment,
}

impl TokenKind {
    /// Its colour in a theme.
    pub fn color(self, theme: &Theme) -> Color {
        match self {
            TokenKind::Plain => theme.text,
            TokenKind::Keyword => theme.accent,
            TokenKind::Type => theme.blue,
            TokenKind::Number => theme.amber,
            TokenKind::String => theme.green,
            TokenKind::Comment => theme.text_faint,
        }
    }
}

/// A string syntax of a language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StringSyntax {
    /// Opening and closing delimiters. (A backslash escapes the next character.)
    pub delimiters: [&'static str; 2],
    /// It may span lines. (Else a line end ends it, closed or not.)
    pub multiline: bool,
    /// It holds one character, or one escape: `'a'`, `'\n'`. A delimiter not closed right after
    /// is no literal (Rust's `'a`, a lifetime).
    pub char_literal: bool,
}

/// A language's syntax for highlighting, and what indents the next line: plain data, so an app
/// writes rules for its own language the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// Keywords.
    pub keywords: &'static [&'static str],
    /// Built-in type names.
    pub types: &'static [&'static str],
    /// A capitalized word is a type name too.
    pub capitalized_types: bool,
    /// Line comment start; "" for none.
    pub line_comment: &'static str,
    /// Block comment start and end; "" for none.
    pub block_comment: [&'static str; 2],
    /// String syntaxes. Where two start alike, the longer first.
    pub strings: &'static [StringSyntax],
    /// Line-end characters after which Enter indents one more step.
    pub indent_after: &'static str,
}

impl Syntax {
    /// No highlighting: every token is plain.
    pub const PLAIN: Syntax = Syntax { keywords: &[], types: &[], capitalized_types: false, line_comment: "", block_comment: ["", ""], strings: &[], indent_after: "([{" };
    /// Rust.
    pub const RUST: Syntax = Syntax {
        keywords: &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
            "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        types: &["bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize", "f32", "f64"],
        capitalized_types: true,
        line_comment: "//",
        block_comment: ["/*", "*/"],
        strings: &[StringSyntax { delimiters: ["\"", "\""], multiline: true, char_literal: false }, StringSyntax { delimiters: ["'", "'"], multiline: false, char_literal: true }],
        indent_after: "([{",
    };
    /// C and C++.
    pub const C: Syntax = Syntax {
        keywords: &[
            "auto",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "enum",
            "extern",
            "false",
            "for",
            "goto",
            "if",
            "inline",
            "namespace",
            "new",
            "nullptr",
            "private",
            "protected",
            "public",
            "return",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "union",
            "using",
            "virtual",
            "volatile",
            "while",
        ],
        types: &["bool", "char", "double", "float", "int", "long", "short", "signed", "size_t", "unsigned", "void"],
        capitalized_types: false,
        line_comment: "//",
        block_comment: ["/*", "*/"],
        strings: &[StringSyntax { delimiters: ["\"", "\""], multiline: false, char_literal: false }, StringSyntax { delimiters: ["'", "'"], multiline: false, char_literal: true }],
        indent_after: "([{",
    };
    /// Python.
    pub const PYTHON: Syntax = Syntax {
        keywords: &[
            "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except", "False", "finally", "for", "from", "global", "if", "import", "in", "is",
            "lambda", "None", "nonlocal", "not", "or", "pass", "raise", "return", "True", "try", "while", "with", "yield",
        ],
        types: &["bool", "bytes", "dict", "float", "int", "list", "object", "set", "str", "tuple"],
        capitalized_types: true,
        line_comment: "#",
        block_comment: ["", ""],
        strings: &[
            StringSyntax { delimiters: ["\"\"\"", "\"\"\""], multiline: true, char_literal: false },
            StringSyntax { delimiters: ["'''", "'''"], multiline: true, char_literal: false },
            StringSyntax { delimiters: ["\"", "\""], multiline: false, char_literal: false },
            StringSyntax { delimiters: ["'", "'"], multiline: false, char_literal: false },
        ],
        indent_after: "([{:",
    };
    /// JavaScript, TypeScript and JSON.
    pub const JAVASCRIPT: Syntax = Syntax {
        keywords: &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "debugger",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "null",
            "of",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "undefined",
            "var",
            "void",
            "while",
            "yield",
        ],
        types: &["any", "boolean", "never", "number", "object", "string", "unknown"],
        capitalized_types: true,
        line_comment: "//",
        block_comment: ["/*", "*/"],
        strings: &[
            StringSyntax { delimiters: ["`", "`"], multiline: true, char_literal: false },
            StringSyntax { delimiters: ["\"", "\""], multiline: false, char_literal: false },
            StringSyntax { delimiters: ["'", "'"], multiline: false, char_literal: false },
        ],
        indent_after: "([{",
    };

    /// Highlights one line. Pushes onto `pieces` each run's end (bytes from the line start) and
    /// kind. `open` is the state before the line (0 at the text's start); returns the state at
    /// its end, for the next line. (This is all a code block shown elsewhere needs:
    /// [`TokenKind::color`] colours each piece.)
    pub fn line(&self, line: &str, open: u8, pieces: &mut Vec<(usize, TokenKind)>) -> u8 {
        self.read(line, open, Some(pieces))
    }

    /// Scans a line: returns its end state, and its pieces if asked.
    fn read(&self, line: &str, mut open: u8, mut pieces: Option<&mut Vec<(usize, TokenKind)>>) -> u8 {
        let mut at = 0;
        while at < line.len() {
            let (end, sort, after) = self.piece(line, at, open, pieces.is_some());
            if let Some(pieces) = pieces.as_deref_mut() {
                join(pieces, end, sort);
            }
            (at, open) = (end, after);
        }
        open
    }

    /// The token of `line` starting at byte `at` in state `open`: its end, kind, and the state
    /// after it. (`named`: words are looked up; else only skipped.)
    fn piece(&self, line: &str, at: usize, open: u8, named: bool) -> (usize, TokenKind, u8) {
        let rest = &line[at..];
        if open == IN_COMMENT {
            return self.comment_end(line, at);
        }
        if let Some(quote) = self.strings.get((open as usize).wrapping_sub(2)) {
            return close(rest, quote.delimiters[1]).map_or((line.len(), TokenKind::String, if quote.multiline { open } else { 0 }), |end| (at + end, TokenKind::String, 0));
        }
        let first = rest.as_bytes()[0];
        if first.is_ascii_digit() {
            return (at + number(rest), TokenKind::Number, 0);
        }
        if first == b'_' || first.is_ascii_alphabetic() || first >= 0x80 {
            let end = at + word_end(rest);
            return (end, if named { self.sort_of(&line[at..end]) } else { TokenKind::Plain }, 0);
        }
        if !self.line_comment.is_empty() && rest.starts_with(self.line_comment) {
            return (line.len(), TokenKind::Comment, 0);
        }
        if !self.block_comment[0].is_empty() && rest.starts_with(self.block_comment[0]) {
            return self.comment_end(line, at + self.block_comment[0].len());
        }
        match self.strings.iter().position(|quote| rest.starts_with(quote.delimiters[0])) {
            Some(found) => self.quoted(line, at, found),
            None => (at + self.plain_end(rest), TokenKind::Plain, 0),
        }
    }

    /// The token a block comment makes from byte `from` of `line`: its end, and whether it is
    /// still open after the line.
    fn comment_end(&self, line: &str, from: usize) -> (usize, TokenKind, u8) {
        let mark = self.block_comment[1];
        match line[from..].find(mark).filter(|_| !mark.is_empty()) {
            Some(end) => (from + end + mark.len(), TokenKind::Comment, 0),
            None => (line.len(), TokenKind::Comment, IN_COMMENT),
        }
    }

    /// The token string syntax `quotes[found]` makes, opening at byte `at` of `line`.
    fn quoted(&self, line: &str, at: usize, found: usize) -> (usize, TokenKind, u8) {
        let quote = self.strings[found];
        let from = at + quote.delimiters[0].len();
        let rest = &line[from..];
        if quote.char_literal {
            // One character, or a backslash escape (up to `\u{10ffff}`), then the closing
            // delimiter: else it was no literal.
            let kept = rest.strip_prefix('\\').map_or(0, |after| 1 + cluster(after));
            let inside = if kept > 0 { rest[kept..].find(quote.delimiters[1]).filter(|end| *end <= 9).map(|end| kept + end) } else { Some(cluster(rest)) };
            let closed = inside.filter(|inside| *inside > 0 && rest[*inside..].starts_with(quote.delimiters[1]));
            return closed.map_or((from, TokenKind::Plain, 0), |inside| (from + inside + quote.delimiters[1].len(), TokenKind::String, 0));
        }
        match close(rest, quote.delimiters[1]) {
            Some(end) => (from + end, TokenKind::String, 0),
            None => (line.len(), TokenKind::String, if quote.multiline { found as u8 + 2 } else { 0 }),
        }
    }

    /// A word's kind: keyword, type name, or neither.
    fn sort_of(&self, word: &str) -> TokenKind {
        if self.keywords.contains(&word) {
            TokenKind::Keyword
        } else if self.types.contains(&word) || self.capitalized_types && word.starts_with(char::is_uppercase) {
            TokenKind::Type
        } else {
            TokenKind::Plain
        }
    }

    /// The end of the plain run at the start of `rest`: past its first character, up to
    /// anything that could start a word, number, comment or string.
    fn plain_end(&self, rest: &str) -> usize {
        let marks = [self.line_comment, self.block_comment[0]].into_iter().chain(self.strings.iter().map(|quote| quote.delimiters[0]));
        let mut firsts = [0u8; 8];
        let mut count = 0;
        for mark in marks.filter_map(|mark| mark.as_bytes().first()).take(8) {
            (firsts[count], count) = (*mark, count + 1);
        }
        let starts = |byte: &u8| byte.is_ascii_alphanumeric() || *byte == b'_' || *byte >= 0x80 || firsts[..count].contains(byte);
        1 + rest.as_bytes()[1..].iter().position(starts).unwrap_or(rest.len() - 1)
    }
}

/// Appends a piece; merges it with the last if of the same kind.
fn join(pieces: &mut Vec<(usize, TokenKind)>, end: usize, sort: TokenKind) {
    match pieces.last_mut() {
        Some(last) if last.1 == sort => last.0 = end,
        _ => pieces.push((end, sort)),
    }
}

/// The end of the first `mark` in `rest`, skipping escaped ones; `None` if there is none.
fn close(rest: &str, mark: &str) -> Option<usize> {
    let (bytes, mark) = (rest.as_bytes(), mark.as_bytes());
    let mut at = 0;
    while at < bytes.len() && !mark.is_empty() {
        if bytes[at..].starts_with(mark) {
            return Some(at + mark.len());
        }
        at += if bytes[at] == b'\\' { 2 } else { 1 };
    }
    None
}

/// The length of the number at the start of `rest`: digits, letters and `_` (`0x1f`, `10u8`,
/// `1_000`), and a point followed by a digit (`1.5`; `1..5` is two).
fn number(rest: &str) -> usize {
    let bytes = rest.as_bytes();
    let goes_on = |at: usize| bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] == b'.' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit);
    (1..bytes.len()).find(|at| !goes_on(*at)).unwrap_or(bytes.len())
}

/// The length of the word at the start of `rest`: letters, digits and `_`, any script.
fn word_end(rest: &str) -> usize {
    rest.bytes().position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80)).unwrap_or(rest.len())
}

/// The column width of a piece starting at column `from`: one per character, a tab to the next
/// multiple of `tab`.
fn columns(text: &str, from: u32, tab: u32) -> u32 {
    text.chars().fold(from, |column, letter| if letter == '\t' { (column / tab + 1) * tab } else { column + 1 }) - from
}

/// The byte length of a line's leading indent.
fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

/// The closing bracket for an opening one.
fn closer(open: char) -> Option<char> {
    BRACKETS.iter().find(|(opens, _)| *opens == open).map(|(_, closes)| *closes)
}

/// Where a text position moves when `edits` are made (each a replaced range and its new text,
/// in order, in pre-edit offsets): along with the text before it. Inside a replaced range, it
/// goes to the end of the replacement.
fn moved(at: usize, edits: &[(Range<usize>, String)]) -> usize {
    let mut shift = 0isize;
    for (range, with) in edits {
        if at < range.start || at == range.start && range.start < range.end {
            break;
        }
        if at < range.end {
            return range.start.saturating_add_signed(shift) + with.len();
        }
        shift += with.len() as isize - range.len() as isize;
    }
    at.saturating_add_signed(shift)
}

/// A stable insertion sort by `key`, for short lists (cursors: usually few).
fn in_order<T>(list: &mut [T], key: impl Fn(&T) -> usize) {
    for at in 1..list.len() {
        let mut to = at;
        while to > 0 && key(&list[to - 1]) > key(&list[to]) {
            list.swap(to - 1, to);
            to -= 1;
        }
    }
}

/// Where a line wraps to fit rows so many columns wide: for each row after the first, its start
/// byte and column. After the last space that fits; with none, before the first character that
/// doesn't.
struct Breaks<'a> {
    letters: std::str::CharIndices<'a>,
    width: u32,
    tab: u32,
    column: u32,
    /// The current row's start, and its last space: each as byte and column.
    from: (usize, u32),
    space: Option<(usize, u32)>,
}

impl<'a> Breaks<'a> {
    fn new(line: &'a str, width: u32, tab: u32) -> Self {
        Breaks { letters: line.char_indices(), width: width.max(1), tab, column: 0, from: (0, 0), space: None }
    }
}

impl Iterator for Breaks<'_> {
    type Item = (usize, u32);
    fn next(&mut self) -> Option<(usize, u32)> {
        for (at, letter) in self.letters.by_ref() {
            let after = if letter == '\t' { (self.column / self.tab + 1) * self.tab } else { self.column + 1 };
            // A space that doesn't fit hangs past the edge: the row breaks after it.
            let over = letter != ' ' && after - self.from.1 > self.width && at > self.from.0;
            let broken = over.then(|| self.space.filter(|(space, _)| *space > self.from.0).unwrap_or((at, self.column)));
            self.space = if letter == ' ' { Some((at + 1, after)) } else { self.space.filter(|_| broken.is_none()) };
            self.column = after;
            if let Some(start) = broken {
                self.from = start;
                return Some(start);
            }
        }
        None
    }
}

/// A cursor and its selection anchor, in bytes: the text between is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sel {
    cursor: usize,
    anchor: usize,
    /// The column it keeps while moving up and down rows.
    column: Option<u32>,
}

impl Sel {
    fn at(at: usize) -> Sel {
        Sel { cursor: at, anchor: at, column: None }
    }
    fn range(&self) -> Range<usize> {
        self.cursor.min(self.anchor)..self.cursor.max(self.anchor)
    }
}

/// One change, undone as one: at each of some places (in pre-change offsets) what went and what
/// came; and the cursors before and after.
struct Change {
    edits: Vec<(usize, String, String)>,
    before: Vec<Sel>,
    after: Vec<Sel>,
    /// It was typed, so typing right after it undoes with it.
    typed: bool,
}

/// The find bar: open, and whether replacing too; search and replacement text; match case, and
/// whole words.
#[derive(Default)]
struct Finding {
    open: bool,
    replacing: bool,
    words: String,
    with: String,
    case: bool,
    whole: bool,
    /// Focus goes to its box when next described.
    focus: bool,
    /// The total match count, as of the search and text (as one hash); and the selected match's
    /// index (0 for none), as of the selection's start.
    counted: (u64, usize, (usize, usize)),
}

/// A code editor's state ([`Ui::code_editor`]): the text and the editing state. Kept by the
/// app, one per editor, as a [`Table`](crate::Table) is.
///
/// ```
/// use taut::{CodeEditor, Syntax};
/// let mut code = CodeEditor::new("fn main() {}\n", &Syntax::RUST);
/// code.wrap = true; // long lines go on to the next row, in place of scrolling sideways
/// assert_eq!(code.text(), "fn main() {}\n");
/// assert_eq!(code.cursor(), (1, 1)); // line and column of the cursor, counted from 1
/// ```
pub struct CodeEditor {
    /// Highlighting: [`Syntax::RUST`], or the app's own; [`Syntax::PLAIN`] highlights nothing.
    pub syntax: &'static Syntax,
    /// Wraps long lines instead of scrolling sideways.
    pub wrap: bool,
    /// Tab width in columns; also the indent width.
    pub tab_width: usize,
    /// Tab indents with a tab character instead of spaces.
    pub hard_tabs: bool,
    text: String,
    /// Each line's start byte; each line's width in columns; and the widest, once computed
    /// since the last change.
    starts: Vec<u32>,
    widths: Vec<u32>,
    widest: Option<u32>,
    /// The text uses CRLF line ends.
    returns: bool,
    /// Each line's start state, for highlighting (see `open_at`): correct for lines before
    /// `sure`; and for those before `kept`, from the first line after `edited` whose recomputed
    /// start state matches the old one.
    states: Vec<u8>,
    sure: usize,
    kept: usize,
    edited: usize,
    /// For wrapping, each line's row count at `wrapped_at` columns (0: no wrapping). And where
    /// lines wrap or fold, each line's first row, and the total row count after the last line;
    /// empty when each line is one row.
    wraps: Vec<u16>,
    wrapped_at: u32,
    tops: Vec<u32>,
    tops_stale: bool,
    /// Folded line ranges: first and last line of each, in order. (The line before the first
    /// stands for them.)
    folds: Vec<[u32; 2]>,
    /// The cursors, the primary first.
    sels: Vec<Sel>,
    changes: Vec<Change>,
    /// How many of the last changes are undone now.
    undone: usize,
    /// Counts every change to the text.
    version: u64,
    /// The tab width and rules all of the above was computed with (the app may change them).
    made_for: (usize, Syntax),
    finding: Finding,
    /// The go-to-line bar, if open: its text, and whether focus goes to it when next described.
    going: Option<(String, bool)>,
    /// For the box: its id, and its measuring id; its room (width and height); when its cursor
    /// last moved (it blinks from then); whether to scroll the primary cursor into view; the
    /// pointer's click count; and that Escape had nothing to do, so Tab moves on.
    id: Id,
    measured: Id,
    view: [f32; 2],
    moved: Duration,
    reveal: bool,
    clicks: u8,
    escaped: bool,
    /// The bracket pair at the primary cursor, as of a version and position.
    paired: (u64, usize, Option<[usize; 2]>),
    /// The last highlighted line's pieces, which line, and as of which version.
    pieces: Vec<(usize, TokenKind)>,
    coloured: (u64, usize),
}

impl std::fmt::Debug for CodeEditor {
    /// Its length in bytes and lines, and its settings; not the text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines = self.starts.len().max(1);
        f.debug_struct("CodeEditor")
            .field("bytes", &self.text.len())
            .field("lines", &lines)
            .field("wrap", &self.wrap)
            .field("tab_width", &self.tab_width)
            .field("hard_tabs", &self.hard_tabs)
            .finish_non_exhaustive()
    }
}

impl Default for CodeEditor {
    fn default() -> Self {
        CodeEditor::new("", &Syntax::PLAIN)
    }
}

impl CodeEditor {
    /// The text to edit, and its highlighting rules.
    pub fn new(text: impl Into<String>, rules: &'static Syntax) -> CodeEditor {
        let mut code = CodeEditor {
            syntax: rules,
            wrap: false,
            tab_width: 4,
            hard_tabs: false,
            text: text.into(),
            starts: Vec::new(),
            widths: Vec::new(),
            widest: None,
            returns: false,
            states: Vec::new(),
            sure: 1,
            kept: 1,
            edited: 0,
            wraps: Vec::new(),
            wrapped_at: 0,
            tops: Vec::new(),
            tops_stale: false,
            folds: Vec::new(),
            sels: vec![Sel::at(0)],
            changes: Vec::new(),
            undone: 0,
            version: 0,
            made_for: (4, *rules),
            finding: Finding::default(),
            going: None,
            id: 0,
            measured: 0,
            view: [0.0; 2],
            moved: Duration::ZERO,
            reveal: false,
            clicks: 1,
            escaped: false,
            paired: (u64::MAX, 0, None),
            pieces: Vec::new(),
            coloured: (u64::MAX, 0),
        };
        code.index();
        code
    }

    /// The current text.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Replaces all the text: the undo history is cleared and the cursor goes to the start.
    pub fn set_text(&mut self, text: impl Into<String>) {
        (self.text, self.sels, self.undone, self.version) = (text.into(), vec![Sel::at(0)], 0, self.version + 1);
        self.changes.clear();
        self.index();
    }
    /// The primary cursor's line and column, both from 1 (column in characters).
    pub fn cursor(&self) -> (usize, usize) {
        let at = self.sels[0].cursor;
        let line = self.line_of(at);
        (line + 1, self.text[self.starts[line] as usize..at].chars().count() + 1)
    }
    /// Puts the cursor at the start of a line (from 1), to be scrolled into view.
    pub fn go_to_line(&mut self, line: usize) {
        let line = line.clamp(1, self.starts.len()) - 1;
        let (start, end) = self.line_ends(line);
        self.sels = vec![Sel::at(start + indent_len(&self.text[start..end]))];
        self.unfold_cursors();
        self.reveal = true;
    }

    // ---- the lines ----

    /// Indexes every line's start and width from scratch: for a new text, or after a change too
    /// large to patch line by line.
    fn index(&mut self) {
        self.starts.clear();
        self.widths.clear();
        let tab = self.tab_width.max(1) as u32;
        let mut from = 0;
        for (at, _) in self.text.match_indices('\n') {
            self.starts.push(from as u32);
            self.widths.push(columns(self.text[from..at].trim_end_matches('\r'), 0, tab));
            from = at + 1;
        }
        self.starts.push(from as u32);
        self.widths.push(columns(&self.text[from..], 0, tab));
        self.returns = self.text.find('\n').is_some_and(|at| self.text[..at].ends_with('\r'));
        self.states.clear();
        self.states.resize(self.starts.len(), 0);
        (self.sure, self.kept, self.edited, self.widest) = (1, 1, 0, None);
        (self.made_for, self.wrapped_at, self.tops_stale) = ((self.tab_width, *self.syntax), 0, true);
        self.folds.clear();
        self.wraps.clear();
    }

    /// The line containing byte `at`.
    fn line_of(&self, at: usize) -> usize {
        self.starts.partition_point(|start| *start as usize <= at) - 1
    }
    /// Line `line`'s start and end (before its line end).
    fn line_ends(&self, line: usize) -> (usize, usize) {
        let start = self.starts[line] as usize;
        let end = self.starts.get(line + 1).map_or(self.text.len(), |next| *next as usize - 1);
        (start, if self.text[start..end].ends_with('\r') { end - 1 } else { end })
    }
    /// Line `line`'s text, without its line end.
    fn line_text(&self, line: usize) -> &str {
        let (start, end) = self.line_ends(line);
        &self.text[start..end]
    }

    /// Re-indexes after `removed` bytes from `at` were replaced by `added`: the touched lines
    /// are re-found and the rest shifted, with everything kept per line.
    fn relines(&mut self, at: usize, removed: usize, added: usize) {
        let (first, last) = (self.line_of(at), self.line_of(at + removed));
        let shift = added as i64 - removed as i64;
        let end = self.starts.get(last + 1).map_or(self.text.len() as i64, |next| *next as i64 - 1 + shift) as usize;
        let (from, tab) = (self.starts[first] as usize, self.tab_width.max(1) as u32);
        // The lines now in place of those.
        let mut found: Vec<(u32, u32)> = Vec::new();
        let mut start = from;
        for (at, _) in self.text[from..end].match_indices('\n') {
            found.push((start as u32, columns(self.text[start..from + at].trim_end_matches('\r'), 0, tab)));
            start = from + at + 1;
        }
        found.push((start as u32, columns(self.text[start..end].trim_end_matches('\r'), 0, tab)));
        let new = found.len();
        self.starts.splice(first..=last, found.iter().map(|line| line.0));
        self.widths.splice(first..=last, found.iter().map(|line| line.1));
        for start in &mut self.starts[first + new..] {
            *start = (*start as i64 + shift) as u32;
        }
        self.relined(first, last, new);
    }

    /// Patches the per-line state after lines `first` to `last` became `new` lines.
    fn relined(&mut self, first: usize, last: usize, new: usize) {
        let shift = new as isize - (last + 1 - first) as isize;
        let along = |line: usize| if line > last { line.saturating_add_signed(shift) } else { line };
        self.states.splice(first + 1..=last, std::iter::repeat_n(0, new - 1));
        // Start states are correct up to the first touched line; after the touched lines they
        // may well match again.
        self.edited = if self.sure > self.edited { first + new - 1 } else { along(self.edited).max(first + new - 1) };
        self.kept = if self.kept > last + 1 { along(self.kept) } else { self.kept.min(first + 1) };
        self.sure = self.sure.min(first + 1);
        if self.wrapped_at > 0 {
            let rows: Vec<u16> = (first..first + new).map(|line| self.rows_of(line)).collect();
            self.wraps.splice(first..=last, rows);
        }
        // A fold whose lines, or whose standing line, were touched is unfolded.
        self.folds.retain(|fold| (fold[1] as usize) < first || fold[0] as usize > last + 1);
        for fold in &mut self.folds {
            *fold = fold.map(|line| along(line as usize) as u32);
        }
        (self.widest, self.tops_stale) = (None, true);
    }

    /// Line `line`'s row count when wrapping at `wrapped_at` columns.
    fn rows_of(&self, line: usize) -> u16 {
        Breaks::new(self.line_text(line), self.wrapped_at, self.tab_width.max(1) as u32).count().saturating_add(1).min(u16::MAX as usize) as u16
    }

    /// Syncs the per-line state with the app's settings, which may have changed: tab width,
    /// rules, and wrapping at `width` columns.
    fn fit(&mut self, width: u32) {
        if self.made_for.0 != self.tab_width {
            self.index();
        }
        if self.made_for.1 != *self.syntax {
            (self.made_for.1, self.sure, self.kept, self.coloured) = (*self.syntax, 1, 1, (u64::MAX, 0));
        }
        let wrap = if self.wrap { width.max(8) } else { 0 };
        if wrap == self.wrapped_at {
            return;
        }
        (self.wrapped_at, self.tops_stale) = (wrap, true);
        self.wraps.clear();
        if wrap > 0 {
            self.wraps = (0..self.starts.len()).map(|line| self.rows_of(line)).collect();
        }
    }

    /// The widest line, in columns.
    fn widest(&mut self) -> u32 {
        let widths = &self.widths;
        *self.widest.get_or_insert_with(|| widths.iter().copied().max().unwrap_or(0))
    }

    // ---- rows: what the box shows of the lines ----

    /// Recomputes each line's first row, where it differs from its line number.
    fn tops(&mut self) {
        if !std::mem::take(&mut self.tops_stale) {
            return;
        }
        self.tops.clear();
        if self.wraps.is_empty() && self.folds.is_empty() {
            return;
        }
        let (mut top, mut fold) = (0u32, 0);
        for line in 0..self.starts.len() {
            self.tops.push(top);
            fold += self.folds.get(fold).is_some_and(|run| run[1] < line as u32) as usize;
            let hidden = self.folds.get(fold).is_some_and(|run| run[0] <= line as u32);
            top += if hidden { 0 } else { self.wraps.get(line).map_or(1, |rows| *rows as u32) };
        }
        self.tops.push(top);
    }
    /// The row count.
    fn row_count(&self) -> usize {
        self.tops.last().map_or(self.starts.len(), |rows| *rows as usize)
    }
    /// The row line `line` starts at.
    fn top_of(&self, line: usize) -> usize {
        self.tops.get(line).map_or(line, |top| *top as usize)
    }
    /// The line of row `row`.
    fn line_at(&self, row: usize) -> usize {
        match self.tops.is_empty() {
            true => row.min(self.starts.len() - 1),
            false => self.tops.partition_point(|top| *top as usize <= row).saturating_sub(1).min(self.starts.len() - 1),
        }
    }
    /// Row `row`: its line, its byte range, and its start column.
    fn row_span(&self, row: usize) -> (usize, usize, usize, u32) {
        let line = self.line_at(row);
        let (start, end) = self.line_ends(line);
        if self.wraps.is_empty() {
            return (line, start, end, 0);
        }
        let part = row.saturating_sub(self.top_of(line));
        let mut breaks = Breaks::new(&self.text[start..end], self.wrapped_at, self.tab_width.max(1) as u32);
        let from = if part == 0 { (0, 0) } else { breaks.nth(part - 1).unwrap_or((end - start, 0)) };
        let to = breaks.next().map_or(end, |next| start + next.0);
        (line, start + from.0, to, from.1)
    }
    /// The row byte `at` shows in, and its column within that row.
    fn place(&self, at: usize) -> (usize, u32) {
        let line = self.line_of(at);
        let start = self.starts[line] as usize;
        let tab = self.tab_width.max(1) as u32;
        let column = columns(&self.text[start..at], 0, tab);
        if self.wraps.is_empty() {
            return (self.top_of(line), column);
        }
        let end = self.line_ends(line).1;
        let before = Breaks::new(&self.text[start..end], self.wrapped_at, tab).take_while(|(byte, _)| start + byte <= at);
        let (part, from) = before.fold((0, 0), |(part, _), (_, from)| (part + 1, from));
        (self.top_of(line) + part, column - from)
    }
    /// The position in row `row` at `column`, or the row's end. (A row a line wraps after ends
    /// before its last character: the position after it is the next row's.)
    fn at_column(&self, row: usize, column: u32) -> usize {
        let (line, from, to, start) = self.row_span(row);
        let last = to == self.line_ends(line).1;
        let (mut at, mut went, tab) = (from, start, self.tab_width.max(1) as u32);
        while at < to {
            let next = at + cluster(&self.text[at..to]);
            went += columns(&self.text[at..next], went, tab);
            if went > start + column || next == to && !last {
                break;
            }
            at = next;
        }
        at
    }

    // ---- folding ----

    /// A line's indent depth in columns; `None` for a blank line.
    fn depth(&self, line: usize) -> Option<u32> {
        let text = self.line_text(line);
        let indent = indent_len(text);
        (indent < text.len()).then(|| columns(&text[..indent], 0, self.tab_width.max(1) as u32))
    }
    /// The last line of the block line `line` starts, if it starts one: the following lines
    /// indented further, up to the last non-blank one. (The next non-blank line is searched
    /// only a few lines on.)
    fn block(&self, line: usize) -> Option<usize> {
        let depth = self.depth(line)?;
        let next = (line + 1..self.starts.len().min(line + 64)).find_map(|next| self.depth(next).map(|deeper| (next, deeper)))?;
        if next.1 <= depth {
            return None;
        }
        let after = (next.0..self.starts.len()).find(|later| self.depth(*later).is_some_and(|deeper| deeper <= depth)).unwrap_or(self.starts.len());
        (line + 1..after).rev().find(|inside| self.depth(*inside).is_some())
    }
    /// Folds the block line `line` starts (with any folds inside), or unfolds it if folded.
    /// Returns whether there was such a block.
    fn fold(&mut self, line: usize) -> bool {
        if let Some(run) = self.folds.iter().position(|run| run[0] as usize == line + 1) {
            self.folds.remove(run);
            self.tops_stale = true;
            return true;
        }
        let Some(last) = self.block(line) else { return false };
        let run = [line as u32 + 1, last as u32];
        self.folds.retain(|inner| inner[1] < run[0] || inner[0] > run[1]);
        let at = self.folds.partition_point(|before| before[0] < run[0]);
        self.folds.insert(at, run);
        self.tops_stale = true;
        // A cursor in the folded lines moves to the end of the standing line.
        let shows = self.line_ends(line).1;
        let inside: Vec<bool> = self.sels.iter().map(|sel| (line + 1..=last).contains(&self.line_of(sel.cursor))).collect();
        for (sel, inside) in self.sels.iter_mut().zip(inside) {
            if inside {
                *sel = Sel::at(shows);
            }
        }
        self.merge();
        true
    }
    /// Unfolds whatever a cursor entered.
    fn unfold_cursors(&mut self) {
        let lines: Vec<usize> = self.sels.iter().map(|sel| self.line_of(sel.cursor)).collect();
        let before = self.folds.len();
        self.folds.retain(|run| !lines.iter().any(|line| (run[0] as usize..=run[1] as usize).contains(line)));
        self.tops_stale |= self.folds.len() != before;
    }

    // ---- colours ----

    /// Line `line`'s start state (see [`Syntax::line`]): scanned on from the last known line,
    /// and after an edit only until it matches again.
    fn open_at(&mut self, line: usize) -> u8 {
        while self.sure <= line {
            let at = self.sure;
            let open = self.syntax.read(self.line_text(at - 1), self.states[at - 1], None);
            if at > self.edited && at < self.kept && self.states[at] == open {
                self.sure = self.kept;
                continue;
            }
            self.states[at] = open;
            self.sure += 1;
        }
        self.kept = self.kept.max(self.sure);
        self.states[line]
    }
    /// Highlights line `line` into `pieces`, unless it was the last highlighted.
    fn colour(&mut self, line: usize) {
        if self.coloured == (self.version, line) {
            return;
        }
        let open = self.open_at(line);
        self.pieces.clear();
        let (start, end) = self.line_ends(line);
        self.syntax.read(&self.text[start..end], open, Some(&mut self.pieces));
        self.coloured = (self.version, line);
    }

    /// The bracket matching the one at the primary cursor (after it, else before), where both
    /// are code, not in a comment or string: both positions. Searched up to `PAIRED_WITHIN`
    /// lines away.
    fn paired(&mut self) -> Option<[usize; 2]> {
        let at = self.sels[0].cursor;
        if self.paired.0 == self.version && self.paired.1 == at {
            return self.paired.2;
        }
        let before = self.text[..at].chars().next_back().map(|letter| (at - letter.len_utf8(), letter));
        let after = self.text[at..].chars().next().map(|letter| (at, letter));
        let found = [after, before].into_iter().flatten().find_map(|(at, letter)| self.partner(at, letter).map(|other| [at, other]));
        self.paired = (self.version, at, found);
        found
    }
    /// The position of the bracket matching `letter` at byte `at`.
    fn partner(&mut self, at: usize, letter: char) -> Option<usize> {
        let (open, close) = *BRACKETS.iter().find(|(open, close)| *open == letter || *close == letter)?;
        let (forward, home) = (letter == open, self.line_of(at));
        if !self.is_code(home, at) {
            return None;
        }
        let mut depth = 0usize;
        let lines: Vec<usize> = if forward { (home..self.starts.len().min(home + PAIRED_WITHIN)).collect() } else { (home.saturating_sub(PAIRED_WITHIN)..=home).rev().collect() };
        for line in lines {
            if let Some(found) = self.pair_in(line, at, [open, close], forward, &mut depth) {
                return Some(found);
            }
        }
        None
    }
    /// Whether byte `at` of line `line` is code: not in a comment or string.
    fn is_code(&mut self, line: usize, at: usize) -> bool {
        self.colour(line);
        let start = self.starts[line] as usize;
        let piece = self.pieces.partition_point(|(end, _)| *end <= at - start);
        self.pieces.get(piece).is_some_and(|(_, sort)| *sort == TokenKind::Plain)
    }
    /// Scans line `line`'s code for the bracket matching the one at `at`, forward or back,
    /// tracking nesting depth of its kind.
    fn pair_in(&mut self, line: usize, at: usize, [open, close]: [char; 2], forward: bool, depth: &mut usize) -> Option<usize> {
        self.colour(line);
        let start = self.starts[line] as usize;
        let mut letters: Vec<(usize, char)> = Vec::new();
        let mut from = 0;
        for (end, sort) in self.pieces.iter().copied() {
            let piece = &self.text[start + from..start + end];
            letters.extend(piece.char_indices().filter(|(_, letter)| *letter == open || *letter == close).filter(|_| sort == TokenKind::Plain).map(|(byte, letter)| (start + from + byte, letter)));
            from = end;
        }
        if !forward {
            letters.reverse();
        }
        for (byte, letter) in letters.into_iter().filter(|(byte, _)| if forward { *byte > at } else { *byte < at }) {
            match letter == open && forward || letter == close && !forward {
                true => *depth += 1,
                false if *depth == 0 => return Some(byte),
                false => *depth -= 1,
            }
        }
        None
    }

    // ---- changing the text ----

    /// Makes one change, undone as one: each of `edits` replaces a range of the text (in order,
    /// non-overlapping). With `carets`, a cursor goes into each replacement, that many bytes
    /// in; else cursors move with the text.
    fn change(&mut self, edits: Vec<(Range<usize>, String)>, carets: Option<Vec<usize>>, typed: bool) {
        if edits.iter().all(|(range, with)| range.is_empty() && with.is_empty()) {
            return;
        }
        let before = self.sels.clone();
        let written: Vec<(usize, String, String)> = edits.iter().map(|(range, with)| (range.start, self.text[range.clone()].to_string(), with.clone())).collect();
        self.apply(&edits);
        if let Some(carets) = carets {
            self.sels = self.carets(&edits, &carets);
            self.merge();
        }
        self.changes.truncate(self.changes.len() - self.undone);
        self.undone = 0;
        // Consecutive typed letters undo together, a word at a time.
        let after = self.sels.clone();
        match self.changes.last_mut().filter(|last| typed && last.typed && joins(last, &written)) {
            Some(last) => {
                last.edits[0].2.push_str(&written[0].2);
                last.after = after;
            }
            None => self.changes.push(Change { edits: written, before, after, typed }),
        }
        if self.changes.len() > HISTORY {
            self.changes.remove(0);
        }
    }

    /// The cursors after `edits`: one in each replacement, `carets` bytes into it.
    fn carets(&self, edits: &[(Range<usize>, String)], carets: &[usize]) -> Vec<Sel> {
        let mut shift = 0isize;
        let mut sels = Vec::with_capacity(edits.len());
        for ((range, with), caret) in edits.iter().zip(carets) {
            sels.push(Sel::at(range.start.saturating_add_signed(shift) + caret));
            shift += with.len() as isize - range.len() as isize;
        }
        sels
    }

    /// Applies each replacement (in order, non-overlapping, in current offsets), re-indexes the
    /// lines, and moves the cursors.
    fn apply(&mut self, edits: &[(Range<usize>, String)]) {
        let many = edits.len() > MANY;
        for (range, with) in edits.iter().rev() {
            self.text.replace_range(range.clone(), with);
            if !many {
                self.relines(range.start, range.len(), with.len());
            }
        }
        if many {
            self.index();
        }
        for sel in &mut self.sels {
            (sel.cursor, sel.anchor) = (moved(sel.cursor, edits), moved(sel.anchor, edits));
        }
        self.version += 1;
    }

    /// Undoes the last change, or (`again`) redoes the last undone one.
    fn undo(&mut self, again: bool) {
        let at = if again { self.changes.len() - self.undone } else { self.changes.len().wrapping_sub(self.undone + 1) };
        let Some(change) = self.changes.get(at).filter(|_| !again || self.undone > 0) else { return };
        let mut shift = 0isize;
        let mut edits = Vec::with_capacity(change.edits.len());
        for (place, removed, added) in &change.edits {
            let (from, to) = if again { (removed, added) } else { (added, removed) };
            let start = if again { *place } else { place.saturating_add_signed(shift) };
            edits.push((start..start + from.len(), to.clone()));
            shift += added.len() as isize - removed.len() as isize;
        }
        let sels = if again { change.after.clone() } else { change.before.clone() };
        self.apply(&edits);
        self.sels = sels;
        self.undone = if again { self.undone - 1 } else { self.undone + 1 };
        self.unfold_cursors();
    }

    /// Edits at every cursor as `make` says: the range to replace, its replacement, and the
    /// cursor's offset into it. (Where two cursors' ranges meet, the first's edit wins.)
    fn each(&mut self, typed: bool, make: impl Fn(&CodeEditor, &Sel) -> (Range<usize>, String, usize)) {
        let mut made: Vec<(Range<usize>, String, usize)> = self.sels.iter().map(|sel| make(self, sel)).collect();
        in_order(&mut made, |(range, ..)| range.start);
        let (mut edits, mut carets, mut end) = (Vec::with_capacity(made.len()), Vec::with_capacity(made.len()), 0);
        for (nth, (range, with, caret)) in made.into_iter().enumerate() {
            if nth == 0 || range.start >= end {
                end = range.end;
                edits.push((range, with));
                carets.push(caret);
            }
        }
        self.change(edits, Some(carets), typed);
    }

    /// Inserts typed or pasted text at every cursor, replacing each selection. (Its line ends
    /// become the text's own.)
    fn put(&mut self, text: &str, typed: bool) {
        let with: String = text.replace("\r\n", "\n").replace('\r', "\n");
        let with = if self.returns { with.replace('\n', "\r\n") } else { with };
        self.each(typed, |_, sel| (sel.range(), with.clone(), with.len()));
    }

    /// What Enter inserts for a cursor: a line end and the line's indent; after a block opener,
    /// one step more; with the matching closing bracket right after the cursor, that bracket on
    /// its own line below.
    fn line_break(&self, sel: &Sel) -> (Range<usize>, String, usize) {
        let range = sel.range();
        let start = self.starts[self.line_of(range.start)] as usize;
        let typed = &self.text[start..range.start];
        let indent = &typed[..indent_len(typed)];
        let last = typed.trim_end().chars().next_back();
        let ending = if self.returns { "\r\n" } else { "\n" };
        let mut with = format!("{ending}{indent}");
        if !last.is_some_and(|last| self.syntax.indent_after.contains(last)) {
            let caret = with.len();
            return (range, with, caret);
        }
        with.push_str(&self.indent_step(indent.len() as u32));
        let caret = with.len();
        if last.and_then(closer).is_some_and(|close| self.text[range.end..].starts_with(close)) {
            with.push_str(&format!("{ending}{indent}"));
        }
        (range, with, caret)
    }

    /// What Tab inserts at column `column`: a tab, or spaces to the next stop.
    fn indent_step(&self, column: u32) -> String {
        let tab = self.tab_width.max(1);
        if self.hard_tabs { "\t".into() } else { " ".repeat(tab - column as usize % tab) }
    }

    /// The lines the cursors' selections touch, in order, each once. (A selection ending at a
    /// line's start doesn't take that line.)
    fn picked_lines(&self) -> Vec<usize> {
        let mut lines = Vec::new();
        for range in self.sels.iter().map(Sel::range) {
            let ends_a_line = range.end > range.start && self.starts[self.line_of(range.end)] as usize == range.end;
            let last = self.line_of(if ends_a_line { range.end - 1 } else { range.end });
            lines.extend(self.line_of(range.start)..=last);
        }
        in_order(&mut lines, |line| *line);
        lines.dedup();
        lines
    }

    /// Tab: indents the selected lines, if any selection spans lines; else inserts an indent at
    /// each cursor.
    fn indent(&mut self) {
        let spans = self.sels.iter().any(|sel| self.line_of(sel.cursor) != self.line_of(sel.anchor));
        if !spans {
            return self.each(true, |code, sel| {
                let range = sel.range();
                let start = code.starts[code.line_of(range.start)] as usize;
                let step = code.indent_step(columns(&code.text[start..range.start], 0, code.tab_width.max(1) as u32));
                let caret = step.len();
                (range, step, caret)
            });
        }
        let edits = self.picked_lines().into_iter().filter(|line| !self.line_text(*line).trim().is_empty()).map(|line| {
            let start = self.starts[line] as usize;
            (start..start, if self.hard_tabs { "\t".to_string() } else { " ".repeat(self.tab_width.max(1)) })
        });
        let edits = edits.collect();
        self.change(edits, None, false);
    }

    /// Shift+Tab: removes one indent step from each cursor's line.
    fn outdent(&mut self) {
        let edits = self.picked_lines().into_iter().map(|line| {
            let (start, text) = (self.starts[line] as usize, self.line_text(line));
            let spaces = text.bytes().take(self.tab_width.max(1)).take_while(|byte| *byte == b' ').count();
            (start..start + if text.starts_with('\t') { 1 } else { spaces }, String::new())
        });
        let edits = edits.filter(|(range, _)| !range.is_empty()).collect();
        self.change(edits, None, false);
    }

    /// What Backspace (or `forward`, Delete) removes at a cursor: the selection, else a
    /// character (with Ctrl a word); and in a space indent, back to the last stop.
    fn doomed(&self, sel: &Sel, forward: bool, words: bool) -> Range<usize> {
        let at = sel.cursor;
        if !sel.range().is_empty() {
            return sel.range();
        }
        let start = self.starts[self.line_of(at)] as usize;
        let lead = &self.text[start..at];
        if !forward && !words && !lead.is_empty() && lead.bytes().all(|byte| byte == b' ') {
            let tab = self.tab_width.max(1);
            return at - ((lead.len() - 1) % tab + 1)..at;
        }
        let other = if words { word(&self.text, at, forward) } else { step(&self.text, at, forward) };
        at.min(other)..at.max(other)
    }

    // ---- the keyboard ----

    /// Handles a key; returns whether the box took it. `page` is the visible row count.
    fn key(&mut self, key: Key, mods: Mods, page: usize, clipboard: &mut dyn Clipboard) -> bool {
        self.escaped = false;
        if key == Key::Escape && mods == Mods::NONE {
            return self.escape();
        }
        if self.edit_key(key, mods, clipboard) || self.bar_key(key, mods) {
            return true;
        }
        if mods.ctrl && mods.alt && matches!(key, Key::Up | Key::Down) {
            return self.add_cursor(key == Key::Up);
        }
        if mods.ctrl && mods.alt && key == Key::Char('f') {
            let line = self.line_of(self.sels[0].cursor);
            return self.fold(line) || self.fold_around(line);
        }
        self.go(key, mods, page)
    }

    /// Escape: closes an open bar, or reduces several cursors to one. With nothing to do it is
    /// not taken, and Tab after it moves focus on.
    fn escape(&mut self) -> bool {
        if self.finding.open || self.going.is_some() {
            (self.finding.open, self.going) = (false, None);
            return true;
        }
        if self.sels.len() > 1 {
            self.sels.truncate(1);
            return true;
        }
        self.escaped = true;
        false
    }

    /// Keys that edit the text or use the clipboard; returns whether this was one.
    fn edit_key(&mut self, key: Key, mods: Mods, clipboard: &mut dyn Clipboard) -> bool {
        let plain = !mods.ctrl && !mods.alt;
        match key {
            Key::Backspace | Key::Delete if !mods.alt => self.each(false, |code, sel| (code.doomed(sel, key == Key::Delete, mods.ctrl), String::new(), 0)),
            Key::Enter if plain => self.each(true, CodeEditor::line_break),
            Key::Tab if plain && mods.shift => self.outdent(),
            Key::Tab if plain => self.indent(),
            Key::Char('a') if mods == Mods::CTRL => self.sels = vec![Sel { cursor: self.text.len(), anchor: 0, column: None }],
            Key::Char('c') if mods == Mods::CTRL => self.copy(clipboard),
            Key::Char('x') if mods == Mods::CTRL => {
                self.copy(clipboard);
                self.each(false, |_, sel| (sel.range(), String::new(), 0));
            }
            Key::Char('v') if mods == Mods::CTRL => clipboard.get().iter().for_each(|pasted| self.put(pasted, false)),
            Key::Char('z' | 'y') if mods.ctrl && !mods.alt => self.undo(mods.shift || key == Key::Char('y')),
            _ => return false,
        }
        true
    }

    /// Copies the cursors' selections, a line each: nothing if none select anything.
    fn copy(&self, clipboard: &mut dyn Clipboard) {
        let mut ranges: Vec<Range<usize>> = self.sels.iter().map(Sel::range).filter(|range| !range.is_empty()).collect();
        in_order(&mut ranges, |range| range.start);
        if !ranges.is_empty() {
            clipboard.set(&ranges.into_iter().map(|range| &self.text[range]).collect::<Vec<_>>().join("\n"));
        }
    }

    /// Keys that open the bars or step through matches; returns whether this was one.
    fn bar_key(&mut self, key: Key, mods: Mods) -> bool {
        match key {
            Key::Char('f' | 'h') if mods == Mods::CTRL => self.open_finding(key == Key::Char('h')),
            Key::Char('g') if mods == Mods::CTRL => self.going = Some((String::new(), true)),
            Key::F(3) if !mods.ctrl && !mods.alt => self.find_next(mods.shift),
            _ => return false,
        }
        true
    }

    /// Opens the find bar (and, `replacing`, replace) with the selection in it, if on one line.
    fn open_finding(&mut self, replacing: bool) {
        let picked = &self.text[self.sels[0].range()];
        if !picked.is_empty() && !picked.contains('\n') {
            self.finding.words = picked.to_string();
        }
        (self.finding.open, self.finding.replacing, self.finding.focus) = (true, replacing, true);
    }

    /// Ctrl+Alt+Up/Down: adds a cursor a row above the top one or below the bottom one, at the
    /// primary's column.
    fn add_cursor(&mut self, up: bool) -> bool {
        let edge = self.sels.iter().map(|sel| sel.cursor).fold(if up { usize::MAX } else { 0 }, |edge, at| if up { edge.min(at) } else { edge.max(at) });
        let column = self.sels[0].column.unwrap_or_else(|| self.place(self.sels[0].cursor).1);
        self.tops();
        let (to, _) = self.row_away(edge, if up { -1 } else { 1 }, Some(column));
        if to != edge {
            self.sels.push(Sel { column: Some(column), ..Sel::at(to) });
        }
        true
    }

    /// Cursor movement keys (with Shift, selecting); returns whether this was one.
    fn go(&mut self, key: Key, mods: Mods, page: usize) -> bool {
        if mods.alt || !matches!(key, Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End | Key::PageUp | Key::PageDown) {
            return false;
        }
        self.tops();
        for at in 0..self.sels.len() {
            let sel = self.sels[at];
            let (to, column) = self.went(&sel, key, mods, page);
            self.sels[at] = Sel { cursor: to, anchor: if mods.shift { sel.anchor } else { to }, column };
        }
        self.merge();
        self.unfold_cursors();
        true
    }

    /// Where a key moves a cursor, and the column it then keeps.
    fn went(&self, sel: &Sel, key: Key, mods: Mods, page: usize) -> (usize, Option<u32>) {
        let (at, picked, text) = (sel.cursor, sel.range(), self.text.as_str());
        let (start, end) = self.line_ends(self.line_of(at));
        let lead = start + indent_len(&text[start..end]);
        let to = match key {
            Key::Left | Key::Right if mods.ctrl => word(text, at, key == Key::Right),
            Key::Left if !mods.shift && !picked.is_empty() => picked.start,
            Key::Right if !mods.shift && !picked.is_empty() => picked.end,
            Key::Left | Key::Right => step(text, at, key == Key::Right),
            Key::Home if mods.ctrl => 0,
            Key::End if mods.ctrl => text.len(),
            // Home goes to the line's first non-blank character, and from there to the line
            // start.
            Key::Home => [lead, start][(at == lead) as usize],
            Key::End => end,
            Key::Up | Key::Down => return self.row_away(at, if key == Key::Up { -1 } else { 1 }, sel.column),
            _ => return self.row_away(at, if key == Key::PageUp { -(page as isize) } else { page as isize }, sel.column),
        };
        (to, None)
    }

    /// The position `rows` up or down from `at` (before the first row: the start; past the
    /// last: the end), at `column`, else at `at`'s column.
    fn row_away(&self, at: usize, rows: isize, column: Option<u32>) -> (usize, Option<u32>) {
        let (row, across) = self.place(at);
        let want = column.unwrap_or(across);
        match row.checked_add_signed(rows) {
            None => (0, Some(want)),
            Some(to) if to >= self.row_count() => (self.text.len(), Some(want)),
            Some(to) => (self.at_column(to, want), Some(want)),
        }
    }

    /// Merges cursors that overlap or touch.
    fn merge(&mut self) {
        let mut at = 1;
        while at < self.sels.len() {
            let (one, other) = (self.sels[at].range(), at);
            let over = (0..at).find(|before| {
                let two = self.sels[*before].range();
                let (low, high) = (one.start.max(two.start), one.end.min(two.end));
                low < high || low == high && (one.is_empty() || two.is_empty())
            });
            let Some(before) = over else {
                at += 1;
                continue;
            };
            let (kept, both) = (self.sels[before], self.sels[before].range().start.min(one.start)..self.sels[before].range().end.max(one.end));
            let forward = kept.cursor >= kept.anchor;
            self.sels[before] = Sel { cursor: if forward { both.end } else { both.start }, anchor: if forward { both.start } else { both.end }, column: kept.column };
            self.sels.remove(other);
        }
    }

    /// Folds the nearest block around line `line` that starts above it.
    fn fold_around(&mut self, line: usize) -> bool {
        let Some(depth) = (0..=line).rev().find_map(|inside| self.depth(inside)) else { return false };
        let outer = (0..line).rev().find(|above| self.depth(*above).is_some_and(|outer| outer < depth));
        outer.is_some_and(|outer| self.fold(outer))
    }

    // ---- the pointer ----

    /// A press at byte `at`, the `clicks`th in a row: one places the cursor, two select the
    /// word, three the line. With Shift the anchor stays; with Alt a cursor is added. Or (not
    /// `first`) the pointer is dragged there.
    fn point(&mut self, at: usize, clicks: u8, first: bool, mods: Mods) {
        if !first {
            self.sels[0].cursor = self.dragged(at);
            return;
        }
        self.clicks = clicks;
        if mods.alt {
            self.sels.insert(0, Sel::at(at));
            return self.merge();
        }
        let anchor = self.sels[0].anchor;
        let picked = match clicks {
            _ if mods.shift => Sel { cursor: at, anchor, column: None },
            2 => Sel { cursor: word_edge(&self.text, at, true), anchor: word_edge(&self.text, at, false), column: None },
            3.. => self.whole_line(at),
            _ => Sel::at(at),
        };
        self.sels = vec![picked];
    }
    /// Where a drag to byte `at` moves the primary cursor: by character, by word after a double
    /// click, by line after a triple one.
    fn dragged(&self, at: usize) -> usize {
        let forward = at >= self.sels[0].anchor;
        match self.clicks {
            2 => word_edge(&self.text, at, forward),
            3.. if forward => self.whole_line(at).cursor,
            3.. => self.starts[self.line_of(at)] as usize,
            _ => at,
        }
    }
    /// The whole line containing byte `at`, with its line end.
    fn whole_line(&self, at: usize) -> Sel {
        let line = self.line_of(at);
        let end = self.starts.get(line + 1).map_or(self.text.len(), |next| *next as usize);
        Sel { cursor: end, anchor: self.starts[line] as usize, column: None }
    }

    // ---- finding ----

    /// The first match starting at or after byte `from` and ending by `to`: case-sensitive or
    /// not, whole word or not.
    fn next_match(&self, from: usize, to: usize) -> Option<Range<usize>> {
        let (words, mut at) = (self.finding.words.as_str(), from);
        while at < to && !words.is_empty() {
            let rest = &self.text[at..to];
            let (start, long) = if self.finding.case { rest.find(words).map(|start| (start, words.len()))? } else { any_case(rest, words)? };
            let found = at + start..at + start + long;
            let alone = !self.finding.whole || !in_word(&self.text, step(&self.text, found.start, false)) || found.start == 0;
            if alone && (!self.finding.whole || !in_word(&self.text, found.end)) {
                return Some(found);
            }
            at = found.start + cluster(&self.text[found.start..]);
        }
        None
    }

    /// Selects the next match after the primary cursor (or before, `back`), wrapping around the
    /// text; and scrolls to it.
    fn find_next(&mut self, back: bool) {
        let picked = self.sels[0].range();
        let found = match back {
            false => self.next_match(picked.end, self.text.len()).or_else(|| self.next_match(0, picked.end)),
            true => self.last_match(picked.start).or_else(|| self.last_match(self.text.len())),
        };
        if let Some(found) = found {
            self.sels = vec![Sel { cursor: found.end, anchor: found.start, column: None }];
            self.unfold_cursors();
            self.reveal = true;
        }
    }
    /// The last match ending by byte `to`.
    fn last_match(&self, to: usize) -> Option<Range<usize>> {
        let mut last = None;
        let mut from = 0;
        while let Some(found) = self.next_match(from, to) {
            from = found.start + cluster(&self.text[found.start..]);
            last = Some(found);
        }
        last
    }

    /// Replaces the selection with the replacement text, if it is a match; then finds the next.
    fn replace_one(&mut self) {
        let picked = self.sels[0].range();
        if !picked.is_empty() && self.next_match(picked.start, picked.end) == Some(picked.clone()) {
            let with = self.finding.with.clone();
            let caret = with.len();
            self.change(vec![(picked, with)], Some(vec![caret]), false);
        }
        self.find_next(false);
    }
    /// Replaces every match, as one change.
    fn replace_all(&mut self) {
        let mut edits = Vec::new();
        let mut from = 0;
        while let Some(found) = self.next_match(from, self.text.len()) {
            from = found.end.max(found.start + 1);
            edits.push((found, self.finding.with.clone()));
        }
        self.change(edits, None, false);
    }

    /// The total match count, and the selected match's index (0 for none). Recounted only when
    /// the search, text or selection changed.
    fn count(&mut self) -> (usize, usize) {
        let picked = self.sels[0].range();
        let key = crate::id::hash((&self.finding.words, self.finding.case, self.finding.whole, self.version));
        if self.finding.counted.0 != key {
            self.finding.counted = (key, self.matches_before(self.text.len()), (0, usize::MAX));
        }
        if self.finding.counted.2.1 != picked.start {
            let place = if self.next_match(picked.start, picked.end) == Some(picked.clone()) { self.matches_before(picked.end) } else { 0 };
            self.finding.counted.2 = (place, picked.start);
        }
        (self.finding.counted.1, self.finding.counted.2.0)
    }
    /// The number of matches ending by byte `to`.
    fn matches_before(&self, to: usize) -> usize {
        let (mut count, mut from) = (0, 0);
        while let Some(found) = self.next_match(from, to) {
            (count, from) = (count + 1, found.end.max(found.start + 1));
        }
        count
    }
}

/// Whether a typed change right after `last` undoes with it: both insert one or more letters at
/// one place, the second right after the first, and the first didn't end a word where the
/// second starts one.
fn joins(last: &Change, edits: &[(usize, String, String)]) -> bool {
    let ([(at, removed, added)], [(was, _, before)]) = (edits, last.edits.as_slice()) else { return false };
    removed.is_empty() && *was + before.len() == *at && (!before.ends_with(char::is_whitespace) || added.starts_with(char::is_whitespace))
}

/// The first case-insensitive match of `needle` in `hay`: its start, and its length in `hay`.
fn any_case(hay: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_ascii() {
        let (bytes, wanted) = (hay.as_bytes(), needle.as_bytes());
        let first = wanted[0].to_ascii_lowercase();
        let found = (0..(bytes.len() + 1).saturating_sub(wanted.len())).find(|at| bytes[*at].to_ascii_lowercase() == first && bytes[*at..*at + wanted.len()].eq_ignore_ascii_case(wanted));
        return found.map(|at| (at, wanted.len()));
    }
    hay.char_indices().find_map(|(at, _)| alike(&hay[at..], needle).map(|long| (at, long)))
}

/// The bytes of `hay` matching `needle` at its start, case-insensitive.
fn alike(hay: &str, needle: &str) -> Option<usize> {
    let mut letters = hay.char_indices();
    for wanted in needle.chars() {
        let (_, letter) = letters.next()?;
        if !letter.to_lowercase().eq(wanted.to_lowercase()) {
            return None;
        }
    }
    Some(letters.next().map_or(hay.len(), |(at, _)| at))
}

/// A code editor's metrics, computed each run: its font, row and font line heights, column
/// width, line number gutter width, and the text's x start.
#[derive(Clone, Copy)]
struct Sizes {
    font: Font,
    row: f32,
    line: f32,
    column: f32,
    numbers: f32,
    text: f32,
}

/// Code editor.
impl Ui {
    /// A code editor showing `code`'s text, highlighted by `code.rules`, with line numbers in a
    /// gutter on the left. It fills its room. `hint` is its screen reader name. `changed` is
    /// set when the user edits the text.
    ///
    /// Keys, besides those of every text box: Tab and Shift+Tab indent and outdent the selected
    /// lines (Escape, then Tab, moves focus on); Enter keeps the indent. Ctrl+F finds, Ctrl+H
    /// replaces, F3 and Shift+F3 find next and previous, Ctrl+G goes to a line.
    /// Ctrl+Alt+Up/Down add a cursor, as does Alt+click; Escape leaves one. Ctrl+Alt+F folds or
    /// unfolds the cursor's block, as does a click on the marker by its line number. Files of
    /// any length open, scroll and edit instantly: only what is visible is described and
    /// highlighted.
    pub fn code_editor(&mut self, code: &mut CodeEditor, hint: &str) -> Response {
        let t = self.theme;
        let sizes = self.code_sizes(code);
        if let Some(room) = self.rooms.get(&code.measured) {
            code.view = [room[0], room[1]];
        }
        code.fit(((code.view[0] - sizes.text - sizes.column) / sizes.column).max(0.0) as u32);
        code.tops();
        let edge = if code.id != 0 && self.focus == code.id { t.accent } else { t.line_strong };
        self.column().grow().min(0.0, 3.0 * sizes.row).fill(t.panel).border(edge, 1.0).radius(t.radius).clip().open();
        let id = self.id("code");
        code.id = id;
        let mut state = self.interact(id);
        let version = code.version;
        if state.focused {
            self.code_input(code, &state, sizes);
        } else if state.hovered {
            self.set_cursor(Cursor::Text);
        }
        self.find_bar(code);
        self.go_bar(code);
        if std::mem::take(&mut code.reveal) {
            self.reveal_code(code, id, sizes);
        }
        self.code_view(code, &state, sizes, hint);
        self.close();
        state.changed = code.version != version;
        self.edit_menu(&state, false);
        state
    }

    /// This run's metrics.
    fn code_sizes(&mut self, code: &CodeEditor) -> Sizes {
        let font = self.theme.font.mono();
        let line = self.text.metrics(font, self.scale).height().ceil();
        let column = self.text.measure(" ", font, self.scale, None).max(1.0);
        let digits = (code.starts.len().ilog10() + 1).max(2) as f32;
        let numbers = (digits * column + 26.0).ceil();
        Sizes { font, row: line.max((font.size * 1.45).round()), line, column, numbers, text: numbers + 6.0 }
    }

    /// Feeds pointer and keys to a focused code editor.
    fn code_input(&mut self, code: &mut CodeEditor, state: &Response, sizes: Sizes) {
        // A selection asked for by a screen reader, or the app (`select_all`), first.
        if self.edit.id == state.id && std::mem::take(&mut self.edit.reveal) {
            code.sels = vec![Sel { cursor: self.edit.cursor.min(code.text.len()), anchor: self.edit.anchor.min(code.text.len()), column: None }];
        }
        if std::mem::take(&mut self.select_all) {
            code.sels = vec![Sel { cursor: code.text.len(), anchor: 0, column: None }];
        }
        let was = (code.version, code.sels.clone());
        self.code_pointer(code, state, sizes);
        let page = ((code.view[1] / sizes.row) as usize).saturating_sub(1).max(1);
        let Ui { input, clipboard, .. } = self;
        input.strokes.retain(|stroke| match stroke {
            Stroke::Key(key, mods) => !code.key(*key, *mods, page, clipboard.as_mut()),
            Stroke::Text(typed) => {
                code.escaped = false;
                code.put(typed, true);
                false
            }
        });
        if was != (code.version, code.sels.clone()) {
            (code.moved, code.reveal, self.again) = (self.now, true, true);
        }
        // The box stands for the edited text: the context menu, and screen reader selection,
        // use it.
        let primary = code.sels[0];
        (self.edit.id, self.edit.secret, self.edit.cursor, self.edit.anchor) = (state.id, false, primary.cursor, primary.anchor);
        self.takes_tab = if code.escaped { 0 } else { state.id };
    }

    /// The pointer in a code editor: on the text it places the cursor (dragging selects); on
    /// the gutter it selects the line, or on a fold marker folds the block.
    fn code_pointer(&mut self, code: &mut CodeEditor, state: &Response, sizes: Sizes) {
        let Some(whole) = self.visible_boxes.scrollers.iter().find(|scroller| scroller.id == state.id).map(|scroller| scroller.rect) else { return };
        let Some(p) = self.input.pointer else { return };
        code.tops();
        let offset = self.scrolls.get(&state.id).copied().unwrap_or_default();
        let on_numbers = p.x - whole.x < sizes.numbers;
        if state.hovered {
            self.set_cursor(if on_numbers { Cursor::Arrow } else { Cursor::Text });
        }
        if !state.pressed {
            return;
        }
        let first = self.input.pressed[0] && state.hovered;
        let row = (((p.y - whole.y + offset[1]) / sizes.row).max(0.0) as usize).min(code.row_count() - 1);
        if first && on_numbers {
            return self.number_pressed(code, row, p.x - whole.x > sizes.numbers - 16.0);
        }
        let at = self.code_at_x(code, row, p.x - whole.x + offset[0] - sizes.text, sizes);
        code.point(at, self.input.clicks, first, self.input.mods);
    }

    /// A click on the gutter at row `row`: on the fold marker it folds or unfolds the line's
    /// block; elsewhere it selects the line.
    fn number_pressed(&mut self, code: &mut CodeEditor, row: usize, on_mark: bool) {
        let line = code.line_at(row);
        if on_mark && code.fold(line) {
            return;
        }
        let picked = code.whole_line(code.starts[line] as usize);
        code.sels = vec![picked];
        code.clicks = 3;
    }

    /// The position in row `row` nearest `x` units from its text start.
    fn code_at_x(&mut self, code: &CodeEditor, row: usize, x: f32, sizes: Sizes) -> usize {
        let (_, from, to, start) = code.row_span(row);
        let (mut at, mut left) = (from, 0.0);
        while at < to {
            let next = at + cluster(&code.text[at..to]);
            let right = self.code_x(code, from, next, start, sizes);
            if x < (left + right) / 2.0 {
                return at;
            }
            (at, left) = (next, right);
        }
        to
    }

    /// The x of byte `at` from its row's text start, the row starting at byte `from` and column
    /// `start`. ASCII characters are one column each, tabs go to their stop, anything else is
    /// measured.
    fn code_x(&mut self, code: &CodeEditor, from: usize, at: usize, start: u32, sizes: Sizes) -> f32 {
        self.code_x_on(code, (0.0, start), from, at, sizes).0
    }

    /// The same, continuing from byte `from` at `x` and column `column`: so each piece of a
    /// long row is placed without rescanning from the row start. Returns byte `at`'s x and
    /// column.
    fn code_x_on(&mut self, code: &CodeEditor, (mut x, mut column): (f32, u32), from: usize, at: usize, sizes: Sizes) -> (f32, u32) {
        let tab = code.tab_width.max(1) as u32;
        for (nth, piece) in code.text[from..at].split('\t').enumerate() {
            if nth > 0 {
                let stop = (column / tab + 1) * tab;
                (x, column) = (x + (stop - column) as f32 * sizes.column, stop);
            }
            let wide = if piece.is_ascii() { piece.len() as f32 * sizes.column } else { self.text.measure(piece, sizes.font, self.scale, None) };
            (x, column) = (x + wide, column + piece.chars().count() as u32);
        }
        (x, column)
    }

    /// Scrolls so the primary cursor is in view.
    fn reveal_code(&mut self, code: &mut CodeEditor, id: Id, sizes: Sizes) {
        let [wide, high] = code.view;
        if high <= 0.0 {
            return;
        }
        code.unfold_cursors();
        code.tops();
        let at = code.sels[0].cursor;
        let (row, _) = code.place(at);
        let (_, from, _, start) = code.row_span(row);
        let x = sizes.text + self.code_x(code, from, at, start, sizes);
        let mut offset = self.scrolls.get(&id).copied().unwrap_or_default();
        let top = row as f32 * sizes.row;
        offset[1] = offset[1].min(top).max(top + sizes.row - high);
        offset[0] = offset[0].min(x - sizes.text).max(x + 2.0 * sizes.column - wide).max(0.0);
        match offset == [0.0; 2] {
            true => self.scrolls.remove(&id),
            false => self.scrolls.insert(id, offset),
        };
    }

    /// The find and replace bar, while open.
    fn find_bar(&mut self, code: &mut CodeEditor) {
        if !code.finding.open {
            return;
        }
        let (t, words) = (self.theme, self.code_words);
        self.column().pad(6.0).gap(6.0).fill(t.raised).open();
        self.wrapping_row().gap(6.0).open();
        self.el().w(Length::Px(200.0)).open();
        if std::mem::take(&mut code.finding.focus) {
            let field = self.next_id();
            self.set_focus(field);
            self.select_all();
        }
        let field = self.text_box(&mut code.finding.words, words[0]);
        self.close();
        let back = self.icon_button(UP, words[9]).clicked;
        let next = self.icon_button(DOWN, words[8]).clicked;
        let (count, place) = code.count();
        self.note(&format!("{place} {} {count}", words[7]));
        self.toggle(&mut code.finding.case, words[2]);
        self.toggle(&mut code.finding.whole, words[3]);
        let shut = self.icon_button(CROSS, words[10]).clicked;
        self.close();
        let with = code.finding.replacing && self.replace_row(code);
        self.close();
        let in_bar = field.focused || with;
        let keyed = |ui: &mut Ui, mods: Mods| in_bar && ui.key(Key::Enter, mods) || ui.key(Key::F(3), mods);
        if field.changed {
            code.sels = vec![Sel::at(code.sels[0].range().start)];
            code.find_next(false);
        } else if back || keyed(self, Mods::SHIFT) {
            code.find_next(true);
        } else if next || keyed(self, Mods::NONE) {
            code.find_next(false);
        }
        if shut || in_bar && self.key(Key::Escape, Mods::NONE) {
            code.finding.open = false;
            self.set_focus(code.id);
        }
    }

    /// The replace row of the bar. Returns whether its box has focus.
    fn replace_row(&mut self, code: &mut CodeEditor) -> bool {
        let words = self.code_words;
        self.wrapping_row().gap(6.0).open();
        self.el().w(Length::Px(200.0)).open();
        let field = self.text_box(&mut code.finding.with, words[1]);
        self.close();
        let one = self.button(words[4]).clicked;
        let all = self.button(words[5]).clicked;
        self.close();
        if one {
            code.replace_one();
        }
        if all {
            code.replace_all();
        }
        field.focused
    }

    /// The go-to-line bar, while open.
    fn go_bar(&mut self, code: &mut CodeEditor) {
        let Some((mut typed, focus)) = code.going.take() else { return };
        let t = self.theme;
        self.row().pad(6.0).gap(6.0).fill(t.raised).open();
        self.el().w(Length::Px(140.0)).open();
        if focus {
            let field = self.next_id();
            self.set_focus(field);
        }
        let field = self.text_box(&mut typed, self.code_words[6]);
        self.close();
        let shut = self.icon_button(CROSS, self.code_words[10]).clicked;
        self.close();
        let go = field.focused && self.key(Key::Enter, Mods::NONE);
        if go && let Ok(line) = typed.trim().parse::<usize>() {
            code.go_to_line(line);
            code.moved = self.now;
        }
        if go || shut || field.focused && self.key(Key::Escape, Mods::NONE) {
            return self.set_focus(code.id);
        }
        code.going = Some((typed, false));
    }

    /// The scrolling area with the visible rows; and what screen readers get of it.
    fn code_view(&mut self, code: &mut CodeEditor, state: &Response, sizes: Sizes, hint: &str) {
        let wide = if code.wrap { 0.0 } else { sizes.text + (code.widest() + 2) as f32 * sizes.column };
        self.el().key("code").grow().scroll().scroll_x().align(Align::Start, Align::Spread).sense().focusable().open();
        // Screen readers get a text box with all its text, the cursor position in characters,
        // and its line of how many.
        if self.listening {
            self.tell_text(state, hint, &code.text);
            let letters = |at: usize| code.text[..at].chars().count() as u32;
            self.tell_caret([letters(code.sels[0].anchor), letters(code.sels[0].cursor)]);
            self.tell_position(code.line_of(code.sels[0].cursor) + 1, code.starts.len());
        }
        self.column().min(wide, 0.0).open();
        code.tops();
        code.measured = self.next_id();
        let caret = state.focused && self.window_focused && self.blink(code.moved);
        let rows = self.rows(code.row_count(), sizes.row);
        let line = code.line_of(code.sels[0].cursor);
        let paired = if state.focused { code.paired() } else { None };
        for row in rows {
            self.code_row(code, row, line, [state.focused, caret], paired, sizes);
        }
        self.close();
        self.close();
        self.close();
    }

    /// One row: its selections, matches and bracket pair, its highlighted text, its cursors,
    /// and over it all, its line number. `line` is the primary cursor's; `shows` says whether
    /// the box has focus, and whether the cursor shows now (it blinks).
    fn code_row(&mut self, code: &mut CodeEditor, row: usize, line: usize, shows: [bool; 2], paired: Option<[usize; 2]>, sizes: Sizes) {
        let t = self.theme;
        let (this, from, to, start) = code.row_span(row);
        let ends = to == code.line_ends(this).1;
        self.el().flow(Flow::Stack).fill(if this == line { t.hover } else { Color::NONE }).open();
        // Behind the text: selections, matches, the bracket pair.
        let picked = t.accent.with_alpha(if shows[0] { 0.35 } else { 0.2 });
        for range in code.sels.iter().map(Sel::range).filter(|range| !range.is_empty() && range.start <= to && range.end > from).collect::<Vec<_>>() {
            let left = self.code_x(code, from, range.start.max(from), start, sizes);
            let right = if range.end > to { self.code_x(code, from, to, start, sizes) + if ends { sizes.column } else { 0.0 } } else { self.code_x(code, from, range.end, start, sizes) };
            self.code_mark(left, right - left, sizes, picked, false);
        }
        let mut found = from;
        while let Some(range) = code.finding.open.then(|| code.next_match(found, to)).flatten() {
            let (left, right) = (self.code_x(code, from, range.start, start, sizes), self.code_x(code, from, range.end, start, sizes));
            self.code_mark(left, right - left, sizes, t.amber.with_alpha(0.3), false);
            found = range.end.max(range.start + 1);
        }
        for at in paired.into_iter().flatten().filter(|at| (from..to).contains(at)) {
            let left = self.code_x(code, from, at, start, sizes);
            self.code_mark(left, sizes.column, sizes, t.text_dim, true);
        }
        self.code_letters(code, this, [from, to], start, sizes);
        // The cursors: the primary where the input method's window goes, the others as marks.
        for (nth, sel) in code.sels.iter().enumerate().filter(|(_, sel)| sel.cursor >= from && (sel.cursor < to || sel.cursor == to && ends)) {
            let x = sizes.text + self.code_x(code, from, sel.cursor, start, sizes);
            let blinks = shows[0] && shows[1];
            // The primary is a text box only while the box has focus: the focused text box's
            // cursor is watched for blinking.
            match nth {
                0 => _ = self.el().text("", sizes.font, t.text).select(shows[0].then_some([0, 0]), blinks).place((Align::Start, Align::Center)).offset(x, 0.0).show(),
                _ => _ = self.el().size(1.0, sizes.line).fill(if blinks { t.text } else { Color::NONE }).place((Align::Start, Align::Center)).offset(x, 0.0).show(),
            }
        }
        self.code_numbers(code, this, from == code.starts[this] as usize, line, sizes);
        self.close();
    }

    /// A mark behind a row's text, `left` units from its text start and so wide: filled, or
    /// (`ring`) outlined.
    fn code_mark(&mut self, left: f32, wide: f32, sizes: Sizes, color: Color, ring: bool) {
        let mark = self.el().size(wide.max(1.0), sizes.row).offset(sizes.text + left, 0.0);
        match ring {
            true => mark.border(color, 1.0).radius(2.0).show(),
            false => mark.fill(color).show(),
        };
    }

    /// A row's text, from byte `ends[0]` to `ends[1]` of line `line` (the row starting at
    /// column `start`): a box per run of one colour, placed where it starts.
    fn code_letters(&mut self, code: &mut CodeEditor, line: usize, [from, to]: [usize; 2], start: u32, sizes: Sizes) {
        let t = self.theme;
        code.colour(line);
        let base = code.starts[line] as usize;
        // Where the last piece went, and its byte: each piece is placed from there.
        let (mut begun, mut went) = (base, (0.0, start, from));
        for nth in 0..code.pieces.len() {
            let (end, sort) = code.pieces[nth];
            let (low, high) = (begun.max(from), (base + end).min(to).max(begun.max(from)));
            begun = base + end;
            // Each piece between tabs on its own: a tab is space, up to its stop.
            for (at, piece) in between_tabs(&code.text, low, high) {
                let (x, column) = self.code_x_on(code, (went.0, went.1), went.2, at, sizes);
                went = (x, column, at);
                self.el().text(piece, sizes.font, sort.color(&t)).select(None, false).place((Align::Start, Align::Center)).offset(sizes.text + x, 0.0).show();
            }
        }
    }

    /// The line number, at the row's left however far it is scrolled sideways, over what
    /// scrolls under it; and beside it, for a line starting a block, the fold marker. Only a
    /// line's first row shows these (`first`).
    fn code_numbers(&mut self, code: &CodeEditor, line: usize, first: bool, current: usize, sizes: Sizes) {
        let t = self.theme;
        let across = self.scrolls.get(&code.id).map_or(0.0, |offset| offset[0]);
        self.row().size(sizes.numbers, sizes.row).fill(t.panel).offset(across, 0.0).pad((0.0, 0.0, 4.0, 0.0)).gap(2.0).align(Align::End, Align::Center).open();
        if first {
            let mut digits = [0u8; 20];
            let shown = number_text(line + 1, &mut digits);
            self.el().text(shown, sizes.font, if line == current { t.text } else { t.text_faint }).show();
        }
        let folded = code.folds.iter().any(|run| run[0] as usize == line + 1);
        match first && (folded || code.block(line).is_some()) {
            true => _ = self.el().icon(if folded { RIGHT } else { DOWN }, 12.0, t.text_faint).show(),
            false => _ = self.el().size(12.0, 12.0).show(),
        }
        self.close();
    }
}

/// The non-empty pieces of `text` from byte `from` to `to` between its tabs, each with its
/// start byte.
fn between_tabs(text: &str, from: usize, to: usize) -> impl Iterator<Item = (usize, &str)> {
    let pieces = text[from..to].split('\t').scan(from, |at, piece| Some((std::mem::replace(at, *at + piece.len() + 1), piece)));
    pieces.filter(|(_, piece)| !piece.is_empty())
}

/// A number written into `digits`, as text.
fn number_text(mut number: usize, digits: &mut [u8; 20]) -> &str {
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (number % 10) as u8;
        number /= 10;
        if number == 0 {
            break;
        }
    }
    std::str::from_utf8(&digits[at..]).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::LocalClipboard;

    /// The text with `|` at each cursor and `[` `]` around each selection.
    fn shown(code: &CodeEditor) -> String {
        let mut marks: Vec<(usize, char)> = Vec::new();
        for sel in &code.sels {
            match sel.range().is_empty() {
                true => marks.push((sel.cursor, '|')),
                false => marks.extend([(sel.range().start, '['), (sel.range().end, ']')]),
            }
        }
        marks.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut text = code.text.clone();
        for (at, mark) in marks {
            text.insert(at, mark);
        }
        text
    }

    /// Presses keys in a windowless code editor, and types.
    fn keys(code: &mut CodeEditor, keys: &[(Key, Mods)]) {
        let mut clipboard = LocalClipboard::default();
        for (key, mods) in keys {
            code.key(*key, *mods, 10, &mut clipboard);
        }
    }
    const NONE: Mods = Mods::NONE;
    const SHIFT: Mods = Mods::SHIFT;
    const CTRL: Mods = Mods::CTRL;

    /// What every line keeps, recomputed from scratch: as it must be after any edit.
    fn same_as_afresh(code: &CodeEditor) {
        let afresh = CodeEditor::new(code.text.clone(), code.syntax);
        assert_eq!((&code.starts, &code.widths), (&afresh.starts, &afresh.widths), "the lines of {:?}", code.text);
    }

    #[test]
    fn a_line_is_coloured_by_the_rules() {
        let mut pieces = Vec::new();
        let open = Syntax::RUST.line("let x: u32 = 0x1f; // done", 0, &mut pieces);
        assert_eq!(open, 0);
        let sorts: Vec<(&str, TokenKind)> = {
            let line = "let x: u32 = 0x1f; // done";
            let mut from = 0;
            pieces.iter().map(|(end, sort)| (&line[std::mem::replace(&mut from, *end)..*end], *sort)).collect()
        };
        assert_eq!(
            sorts,
            [
                ("let", TokenKind::Keyword),
                (" x: ", TokenKind::Plain),
                ("u32", TokenKind::Type),
                (" = ", TokenKind::Plain),
                ("0x1f", TokenKind::Number),
                ("; ", TokenKind::Plain),
                ("// done", TokenKind::Comment)
            ]
        );
        // A multi-line string and a block comment stay open and close on a later line.
        pieces.clear();
        assert_eq!(Syntax::RUST.line("let s = \"one", 0, &mut pieces), 2);
        pieces.clear();
        assert_eq!(Syntax::RUST.line("two\"; /* three", 2, &mut pieces), IN_COMMENT);
        assert_eq!(pieces, [(4, TokenKind::String), (6, TokenKind::Plain), (14, TokenKind::Comment)]);
        pieces.clear();
        assert_eq!(Syntax::RUST.line("four */ x", IN_COMMENT, &mut pieces), 0);
        assert_eq!(pieces, [(7, TokenKind::Comment), (9, TokenKind::Plain)]);
        // A character literal is a string, a lifetime is not; an escaped delimiter doesn't
        // close.
        pieces.clear();
        Syntax::RUST.line("f::<'a>('\\'', 'b', \"\\\"\")", 0, &mut pieces);
        assert_eq!(pieces, [(8, TokenKind::Plain), (12, TokenKind::String), (14, TokenKind::Plain), (17, TokenKind::String), (19, TokenKind::Plain), (23, TokenKind::String), (24, TokenKind::Plain)]);
        // Python's triple quotes span lines, its single ones don't.
        pieces.clear();
        assert_eq!(Syntax::PYTHON.line("x = \"\"\"doc", 0, &mut pieces), 2);
        assert_eq!(Syntax::PYTHON.line("y = 'no end", 0, &mut pieces), 0);
    }

    #[test]
    fn lines_are_kept_right_through_every_edit() {
        let mut code = CodeEditor::new("one\ntwo\r\nthree\n\tfour", &Syntax::PLAIN);
        same_as_afresh(&code);
        assert_eq!(code.widths, [3, 3, 5, 8]);
        for (at, removed, added) in [(0, 0, "zero\n"), (5, 4, ""), (2, 3, "a\nb\nc"), (0, 0, ""), (4, 6, "\r\n\r\n")] {
            let at = at.min(code.text.len());
            let end = (at + removed).min(code.text.len());
            code.change(vec![(at..end, added.to_string())], None, false);
            same_as_afresh(&code);
        }
        // And a change at more places than are patched one by one.
        let edits: Vec<_> = (0..100).map(|n| (0..0, format!("{n}\n"))).take(1).collect();
        code.change(edits, None, false);
        let many: Vec<(Range<usize>, String)> = (0..code.text.len()).step_by(2).map(|at| (at..at, "\n".to_string())).filter(|(range, _)| code.text.is_char_boundary(range.start)).collect();
        code.change(many, None, false);
        same_as_afresh(&code);
    }

    #[test]
    fn what_is_left_open_is_read_only_as_far_as_needed_and_again_only_until_it_agrees() {
        let mut lines: Vec<String> = (0..1000).map(|n| format!("let x{n} = {n};")).collect();
        lines[10] = "/* a comment".into();
        lines[12] = "that ends */".into();
        let mut code = CodeEditor::new(lines.join("\n"), &Syntax::RUST);
        assert_eq!(code.open_at(11), IN_COMMENT);
        assert_eq!(code.sure, 12, "no further than asked");
        assert_eq!(code.open_at(900), 0);
        assert_eq!(code.sure, 901);
        // An edit far above: rescanned from there until it matches, and no further.
        let at = code.starts[3] as usize;
        code.change(vec![(at..at, "1".into())], None, false);
        assert_eq!(code.sure, 4);
        assert_eq!(code.open_at(800), 0);
        assert_eq!(code.sure, 901, "it agreed again at once, and kept the rest");
        // An edit opening a comment: everything after it is inside.
        code.change(vec![(at..at, "/*".into())], None, false);
        assert_eq!(code.open_at(5), IN_COMMENT);
        assert_eq!(code.open_at(800), 0, "the comment ends at line 12");
        assert_eq!(code.open_at(11), IN_COMMENT);
        // Each state is what scanning every line from the start gives.
        let mut open = 0;
        for line in 0..code.starts.len() {
            assert_eq!(code.open_at(line), open, "line {line}");
            open = code.syntax.read(code.line_text(line), open, None);
        }
    }

    #[test]
    fn enter_keeps_the_indent_and_tab_indents() {
        let mut code = CodeEditor::new("fn main() {}", &Syntax::RUST);
        code.sels = vec![Sel::at(11)];
        keys(&mut code, &[(Key::Enter, NONE)]);
        assert_eq!(shown(&code), "fn main() {\n    |\n}", "after a bracket that opens, a step further; the closing one under");
        code.put("let x = 1;", true);
        keys(&mut code, &[(Key::Enter, NONE)]);
        assert_eq!(shown(&code), "fn main() {\n    let x = 1;\n    |\n}");
        keys(&mut code, &[(Key::Backspace, NONE)]);
        assert_eq!(shown(&code), "fn main() {\n    let x = 1;\n|\n}", "Backspace in spaces goes back a stop");
        keys(&mut code, &[(Key::Tab, NONE), (Key::Char('a'), CTRL), (Key::Tab, NONE)]);
        assert_eq!(code.text, "    fn main() {\n        let x = 1;\n    \n    }", "Tab indents every line selected, but blank ones");
        keys(&mut code, &[(Key::Tab, SHIFT), (Key::Tab, SHIFT)]);
        assert_eq!(code.text, "fn main() {\nlet x = 1;\n\n}");
        code.hard_tabs = true;
        code.sels = vec![Sel::at(12)];
        keys(&mut code, &[(Key::Tab, NONE)]);
        assert_eq!(shown(&code), "fn main() {\n\t|let x = 1;\n\n}");
        keys(&mut code, &[(Key::Char('z'), CTRL)]);
        assert_eq!(shown(&code), "fn main() {\n|let x = 1;\n\n}");
    }

    #[test]
    fn several_cursors_type_and_move_together() {
        let mut code = CodeEditor::new("one\ntwo\nthree", &Syntax::PLAIN);
        keys(&mut code, &[(Key::Down, Mods { ctrl: true, alt: true, shift: false }), (Key::Down, Mods { ctrl: true, alt: true, shift: false })]);
        assert_eq!(shown(&code), "|one\n|two\n|three");
        code.put("- ", true);
        assert_eq!(shown(&code), "- |one\n- |two\n- |three");
        keys(&mut code, &[(Key::End, NONE), (Key::Left, SHIFT)]);
        assert_eq!(shown(&code), "- on[e]\n- tw[o]\n- thre[e]");
        keys(&mut code, &[(Key::Char('x'), CTRL), (Key::Char('z'), CTRL)]);
        assert_eq!(shown(&code), "- on[e]\n- tw[o]\n- thre[e]", "undone, the cursors are back as they were");
        keys(&mut code, &[(Key::Home, NONE), (Key::Escape, NONE)]);
        assert_eq!(shown(&code), "|- one\n- two\n- three", "Escape leaves the main cursor: the first");
        // Overlapping cursors merge.
        code.sels = vec![Sel::at(1), Sel::at(2)];
        keys(&mut code, &[(Key::Home, NONE)]);
        assert_eq!(code.sels.len(), 1);
    }

    #[test]
    fn rows_follow_wrapping_and_folding() {
        let mut code = CodeEditor::new("fn a() {\n    one two three four five\n    six\n}\nend", &Syntax::RUST);
        code.wrap = true;
        code.fit(12);
        code.tops();
        assert_eq!(code.wraps, [1, 3, 1, 1, 1]);
        assert_eq!(code.row_count(), 7);
        let spans: Vec<&str> = (0..7).map(|row| code.row_span(row)).map(|(_, from, to, _)| &code.text[from..to]).collect();
        assert_eq!(spans, ["fn a() {", "    one two ", "three four ", "five", "    six", "}", "end"]);
        // Down keeps its column across rows of one line.
        code.sels = vec![Sel::at(code.starts[1] as usize + 5)];
        keys(&mut code, &[(Key::Down, NONE)]);
        assert_eq!(code.place(code.sels[0].cursor), (2, 5));
        // Folding hides the block's lines; a cursor in them moves to the standing line.
        assert!(code.fold(0));
        code.tops();
        assert_eq!(code.row_count(), 3);
        assert_eq!((code.line_at(1), code.line_at(2)), (3, 4));
        assert_eq!(code.sels[0].cursor, 8);
        keys(&mut code, &[(Key::Down, NONE)]);
        assert_eq!(code.line_of(code.sels[0].cursor), 3, "Down goes past what is folded");
        // An edit on a fold's standing line unfolds it.
        code.sels = vec![Sel::at(2)];
        code.put("x", true);
        assert!(code.folds.is_empty());
        same_as_afresh(&code);
    }

    #[test]
    fn find_and_replace_with_case_and_whole_words() {
        let mut code = CodeEditor::new("Cat cat catalog\ncat", &Syntax::PLAIN);
        code.finding.words = "cat".into();
        assert_eq!(code.count(), (4, 0));
        code.finding.whole = true;
        assert_eq!(code.count(), (3, 0));
        code.finding.case = true;
        assert_eq!(code.count(), (2, 0));
        keys(&mut code, &[(Key::F(3), NONE)]);
        assert_eq!(shown(&code), "Cat [cat] catalog\ncat");
        assert_eq!(code.count(), (2, 1));
        keys(&mut code, &[(Key::F(3), NONE), (Key::F(3), NONE)]);
        assert_eq!(shown(&code), "Cat [cat] catalog\ncat", "it goes round");
        keys(&mut code, &[(Key::F(3), SHIFT)]);
        assert_eq!(shown(&code), "Cat cat catalog\n[cat]");
        code.finding.with = "dog".into();
        code.replace_one();
        assert_eq!(shown(&code), "Cat [cat] catalog\ndog");
        code.finding.case = false;
        code.replace_all();
        assert_eq!(code.text, "dog dog catalog\ndog");
        keys(&mut code, &[(Key::Char('z'), CTRL)]);
        assert_eq!(code.text, "Cat cat catalog\ndog", "replacing all is undone as one");
        // Non-ASCII letters are found case-insensitively.
        code.set_text("Ärger ärger");
        (code.finding.words, code.finding.whole) = ("ÄRGER".into(), false);
        assert_eq!(code.count().0, 2);
    }

    #[test]
    fn brackets_pair_only_in_code() {
        let mut code = CodeEditor::new("f(a, \"(\", [b]) // )\n{\n  g()\n}", &Syntax::RUST);
        code.sels = vec![Sel::at(1)];
        assert_eq!(code.paired(), Some([1, 13]));
        code.sels = vec![Sel::at(14)];
        assert_eq!(code.paired(), Some([13, 1]), "the one before the cursor, where none is after it");
        code.sels = vec![Sel::at(20)];
        assert_eq!(code.paired(), Some([20, 28]), "over lines");
        code.sels = vec![Sel::at(6)];
        assert_eq!(code.paired(), None, "a bracket in a quote pairs with nothing");
    }

    #[test]
    fn the_pointer_selects_by_letter_word_and_line() {
        let mut code = CodeEditor::new("one two\nthree", &Syntax::PLAIN);
        code.point(5, 1, true, NONE);
        code.point(10, 1, false, NONE);
        assert_eq!(shown(&code), "one t[wo\nth]ree");
        code.point(5, 2, true, NONE);
        assert_eq!(shown(&code), "one [two]\nthree");
        code.point(5, 3, true, NONE);
        assert_eq!(shown(&code), "[one two\n]three");
        code.point(10, 1, true, NONE);
        code.point(1, 1, true, Mods { alt: true, ..NONE });
        assert_eq!(code.sels.len(), 2, "a press with Alt is one cursor more");
        code.point(2, 1, true, Mods { alt: true, ..NONE });
        assert_eq!(code.sels.len(), 3);
    }
}
