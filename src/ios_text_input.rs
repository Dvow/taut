//! Typed text on iOS: a view of the library's own (`TautTextInput`), as large as the page, is
//! the first responder while a text box has focus, so the system's keyboard shows, and a
//! hardware keyboard and input methods type through it. Without focus in a text box, the
//! library's view is, and the keyboard goes.
//!
//! It is a `UITextInput` whose document is the focused text box's text, so the keyboard
//! corrects, predicts and capitalizes as in any app. Its changes (`insertText:`,
//! `replaceRange:withText:`, `setSelectedTextRange:`) are made to the view's copy at once, as
//! the keyboard reads it back before the box runs, and reach the box as one stroke each. After
//! each run the copy follows the box, and the keyboard hears when the app changed it. Places on
//! the screen (the caret, a range's rectangles, the position nearest a point) are measured on
//! the box's lines as last laid out. In a password box, and in parts editing text their own way
//! (the code box), the document is only what an input method composes, and the keyboard
//! neither corrects nor capitalizes.
//!
//! Positions are UTF-16 units into the document, kept in objects of the library's position and
//! range classes. What an input method composes (its *marked text*) stands in the document in
//! place of the selection it replaces; it shows over the text cursor, underlined, its selected
//! part thicker, and reaches the box once committed. Return, Tab and Delete are keys.
//!
//! The system's Copy, Cut, Paste, Select and Select All (its edit menu, the shortcut bar over an
//! iPad's keyboard, the menu bar) do what their keys do. A long press or a secondary click in a
//! text box shows the system's edit menu. All of this is handed to the window by the first box
//! that edits text (`hand_typing`), and what reads a text box's text by the first text box
//! (`hand_text_boxes`).
use crate::event::{Event, Key, Mods};
use crate::geom::{Point, Rect, pt, rect};
use crate::ios::{Heard, heard};
use crate::layout::{Align, Content};
use crate::macos_sys::*;
use crate::text::{Carets, Line};
use crate::text_edit::{Replace, step, word_edge};
use crate::ui::{Stroke, Ui};
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::ops::Range;

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static NSUnderlineStyleAttributeName: Obj;
    static NSBackgroundColorAttributeName: Obj;
}

/// `UITextLayoutDirection` and `UITextStorageDirection`: what counts as backward, and up and
/// down.
const LEFT: isize = 3;
const UP: isize = 4;
const DOWN: isize = 5;
const BACKWARD: isize = 1;
/// Underline styles: single, thick.
const SINGLE: isize = 1;
const THICK: isize = 2;
/// Text input traits: the system's choice, no, and capitals starting sentences.
const DEFAULT: isize = 0;
const NO: isize = 1;
const SENTENCES: isize = 2;

/// The document the keyboard edits, and what the input view keeps besides.
#[derive(Default)]
struct Document {
    /// The focused text box's text, its selection (anchor, cursor, in bytes), and the box, as
    /// the box will have them once the changes sent are made: while the text is the document
    /// (`whole`).
    text: String,
    selection: [usize; 2],
    whole: bool,
    box_id: crate::id::Id,
    /// Changes sent to the box and not yet made.
    sent: usize,
    /// What an input method composes, the bytes of `text` it stands for, and its selected part
    /// (UTF-16 units of `marked`).
    marked: String,
    over: [usize; 2],
    picked: NSRange,
    /// Whether a text box has the keyboard, a password box, and where its text cursor is (in
    /// page units).
    editing: bool,
    secret: bool,
    cursor: Option<Rect>,
    delegate: Obj,
    tokenizer: Obj,
    style: Obj,
    label: Obj,
    menu: Obj,
}

thread_local! {
    static INPUT: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    static DOCUMENT: RefCell<Document> = RefCell::new(Document::default());
}

/// Plain methods' types: an object, a number, a `BOOL`.
const OBJECT: &str = "@@:";
const NUMBER: &str = "q@:";

/// The input view's class.
fn input_class() -> Obj {
    let (yes, rect) = (format!("{BOOL}@:"), format!("{RECT}@:@"));
    let methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"canBecomeFirstResponder", yes_please as *const c_void, yes.clone()),
        (c"hasText", has_text as *const c_void, yes.clone()),
        (c"insertText:", inserted as *const c_void, "v@:@".into()),
        (c"deleteBackward", deleted as *const c_void, "v@:".into()),
        (c"textInRange:", text_in as *const c_void, "@@:@".into()),
        (c"replaceRange:withText:", replaced as *const c_void, "v@:@@".into()),
        (c"selectedTextRange", selected_range as *const c_void, OBJECT.into()),
        (c"setSelectedTextRange:", set_selected_range as *const c_void, "v@:@".into()),
        (c"markedTextRange", marked_range as *const c_void, OBJECT.into()),
        (c"markedTextStyle", style as *const c_void, OBJECT.into()),
        (c"setMarkedTextStyle:", set_style as *const c_void, "v@:@".into()),
        (c"setMarkedText:selectedRange:", marked as *const c_void, format!("v@:@{RANGE}")),
        (c"unmarkText", unmarked as *const c_void, "v@:".into()),
        (c"beginningOfDocument", beginning as *const c_void, OBJECT.into()),
        (c"endOfDocument", end as *const c_void, OBJECT.into()),
        (c"textRangeFromPosition:toPosition:", range_between as *const c_void, "@@:@@".into()),
        (c"positionFromPosition:offset:", moved as *const c_void, "@@:@q".into()),
        (c"positionFromPosition:inDirection:offset:", moved_toward as *const c_void, "@@:@qq".into()),
        (c"comparePosition:toPosition:", compared as *const c_void, "q@:@@".into()),
        (c"offsetFromPosition:toPosition:", offset_between as *const c_void, "q@:@@".into()),
        (c"inputDelegate", delegate as *const c_void, OBJECT.into()),
        (c"setInputDelegate:", set_delegate as *const c_void, "v@:@".into()),
        (c"tokenizer", tokenizer as *const c_void, OBJECT.into()),
        (c"positionWithinRange:farthestInDirection:", farthest as *const c_void, "@@:@q".into()),
        (c"characterRangeByExtendingPosition:inDirection:", extended as *const c_void, "@@:@q".into()),
        (c"baseWritingDirectionForPosition:inDirection:", left_to_right as *const c_void, "q@:@q".into()),
        (c"setBaseWritingDirection:forRange:", ignored_direction as *const c_void, "v@:q@".into()),
        (c"firstRectForRange:", first_rect as *const c_void, rect.clone()),
        (c"caretRectForPosition:", caret_rect as *const c_void, rect),
        (c"selectionRectsForRange:", selection_rects as *const c_void, "@@:@".into()),
        (c"closestPositionToPoint:", closest as *const c_void, format!("@@:{POINT}")),
        (c"closestPositionToPoint:withinRange:", closest_within as *const c_void, format!("@@:{POINT}@")),
        (c"characterRangeAtPoint:", range_at as *const c_void, format!("@@:{POINT}")),
        (c"autocorrectionType", correcting as *const c_void, NUMBER.into()),
        (c"spellCheckingType", correcting as *const c_void, NUMBER.into()),
        (c"smartQuotesType", correcting as *const c_void, NUMBER.into()),
        (c"smartDashesType", correcting as *const c_void, NUMBER.into()),
        (c"smartInsertDeleteType", correcting as *const c_void, NUMBER.into()),
        (c"autocapitalizationType", capitals as *const c_void, NUMBER.into()),
        (c"isSecureTextEntry", secret as *const c_void, yes),
        (c"copy:", copied as *const c_void, "v@:@".into()),
        (c"cut:", cut as *const c_void, "v@:@".into()),
        (c"paste:", pasted as *const c_void, "v@:@".into()),
        (c"select:", word_selected as *const c_void, "v@:@".into()),
        (c"selectAll:", all_selected as *const c_void, "v@:@".into()),
        (c"canPerformAction:withSender:", can_perform as *const c_void, format!("{BOOL}@::@")),
        (c"pressesBegan:withEvent:", crate::ios::presses_began as *const c_void, "v@:@@".into()),
        (c"pressesEnded:withEvent:", crate::ios::presses_ended as *const c_void, "v@:@@".into()),
        (c"pressesCancelled:withEvent:", crate::ios::presses_ended as *const c_void, "v@:@@".into()),
    ];
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautTextInput", c"UIView", &listed, &[c"UITextInput", c"UIKeyInput", c"UITextInputTraits"])
}

/// The position class: one UTF-16 offset, kept beside the object.
fn position_class() -> Obj {
    made_class(c"TautTextPosition", c"UITextPosition", &[], &[])
}

/// The range class: two offsets, kept beside the object.
fn range_class() -> Obj {
    let methods: [(&CStr, *const c_void, &str); 3] =
        [(c"start", range_start as *const c_void, OBJECT), (c"end", range_end as *const c_void, OBJECT), (c"isEmpty", range_empty as *const c_void, "B@:")];
    made_class(c"TautTextRange", c"UITextRange", &methods, &[])
}

/// What a selection rectangle keeps beside it: its place, and whether it holds the range's
/// start and end.
#[derive(Clone, Copy)]
struct SelectionRect {
    rect: NSRect,
    start: bool,
    end: bool,
}

/// The selection rectangle class, made once (also what readers' text gives).
fn selection_rect_class() -> Obj {
    let flag = format!("{BOOL}@:");
    let methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"rect", rect_of as *const c_void, format!("{RECT}@:")),
        (c"writingDirection", left_to_right_rect as *const c_void, NUMBER.into()),
        (c"containsStart", holds_start as *const c_void, flag.clone()),
        (c"containsEnd", holds_end as *const c_void, flag.clone()),
        (c"isVertical", not_vertical as *const c_void, flag),
    ];
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautSelectionRect", c"UITextSelectionRect", &listed, &[])
}

/// Hands the window typing, on this thread: the input view, and the long press in text boxes.
/// From the first box that edits text (a text box, the code box), so a program without one
/// carries none of it.
pub(crate) fn hand_typing() {
    if crate::ios::TYPING.get().is_some() {
        return;
    }
    crate::ios::TYPING.set(Some(follow));
    let view = crate::ios::view();
    open(view);
    hold_in_text_boxes(view);
}

/// Makes the input view, inside the library's view: clear, touched by nothing.
fn open(view: Obj) {
    // SAFETY: a view of the library's class made on the main thread; the library's view keeps
    // it, and it is kept here too.
    unsafe {
        let input: Obj = send(send::<Obj>(input_class(), c"alloc", ()), c"initWithFrame:", (NSRect::new(0.0, 0.0, 1.0, 1.0),));
        send::<()>(input, c"setUserInteractionEnabled:", (0 as Bool,));
        send::<()>(input, c"setBackgroundColor:", (send::<Obj>(class(c"UIColor"), c"clearColor", ()),));
        send::<()>(view, c"addSubview:", (input,));
        send::<()>(input, c"release", ());
        let tokenizer: Obj = send(send::<Obj>(class(c"UITextInputStringTokenizer"), c"alloc", ()), c"initWithTextInput:", (input,));
        DOCUMENT.with_borrow_mut(|document| document.tokenizer = tokenizer);
        INPUT.set(input);
    }
}

// ---- after a run: the document follows the box ----

/// What the box shows the keyboard after a run: the text box with focus (if it is one whose
/// text is the document), its text and selection.
struct Focused {
    whole: bool,
    box_id: crate::id::Id,
    text: String,
    selection: [usize; 2],
}

/// What the keyboard needs of text boxes beyond what an input method composes: the focused
/// box's text, places on its lines, and where characters and words end. Handed over the first
/// time a text box is typed in (`hand_text_boxes`), so a program without one carries none of
/// it.
#[derive(Clone, Copy)]
struct TextBoxes {
    text: fn(&Ui) -> Option<String>,
    caret: fn(&mut Ui, usize) -> Option<Rect>,
    pieces: fn(&mut Ui, [usize; 2]) -> Vec<Rect>,
    nearest: fn(&mut Ui, Point) -> Option<usize>,
    /// Where the character before a byte starts, and the word around a byte.
    before: fn(&str, usize) -> usize,
    word: fn(&str, usize) -> [usize; 2],
}

thread_local! {
    static TEXT_BOXES: Cell<Option<TextBoxes>> = const { Cell::new(None) };
}

/// Hands the keyboard what it needs of text boxes (see `TextBoxes`), on this thread.
pub(crate) fn hand_text_boxes() {
    hand_typing();
    if TEXT_BOXES.get().is_some() {
        return;
    }
    TEXT_BOXES.set(Some(TextBoxes {
        text: box_text,
        caret: |ui, at| laid_out(ui)?.caret(ui, at),
        pieces: |ui, bytes| laid_out(ui).map_or_else(Vec::new, |laid| laid.pieces(ui, bytes)),
        nearest: |ui, to| laid_out(ui)?.nearest(ui, to),
        before: |text, at| step(text, at, false),
        word: |text, at| [word_edge(text, at, false), word_edge(text, at, true)],
    }));
}

/// The focused text box's text as last laid out (empty while it shows its hint): while it is
/// the document, a plain text box's.
fn box_text(ui: &Ui) -> Option<String> {
    let plain = ui.ime_area().is_some() && !ui.password_focused() && ui.edit.id == ui.focus();
    if !plain || ui.edit.empty {
        return plain.then(String::new);
    }
    let Content::Text { start, end, .. } = ui.tree.specs.get(ui.typed_in? as usize)?.content else { return None };
    ui.tree.strings.get(start as usize..end as usize).map(str::to_string)
}

/// What the keyboard is to see after this run.
fn focused(ui: &Ui) -> Focused {
    match TEXT_BOXES.get().and_then(|boxes| (boxes.text)(ui)) {
        Some(text) => Focused { whole: true, box_id: ui.edit.id, text, selection: [ui.edit.anchor, ui.edit.cursor] },
        None => Focused { whole: false, box_id: ui.edit.id, text: String::new(), selection: [0; 2] },
    }
}

/// After a run: the input view takes the keyboard while a text box has focus, and gives it back
/// to the library's view after; it lies over the page (`area`, in the view). Its document
/// follows the box, and the keyboard hears what the app changed. A password box, and another
/// kind of box, start the keyboard afresh.
pub(crate) fn follow(ui: &mut Ui, area: NSRect) {
    // SAFETY: the input view lies over the page.
    unsafe { send::<()>(INPUT.get(), c"setFrame:", (area,)) };
    let now = focused(ui);
    let (cursor, secret) = (ui.ime_area(), ui.password_focused());
    let editing = cursor.is_some();
    let (fresh, moved, waiting) = DOCUMENT.with_borrow_mut(|document| {
        let fresh = (editing, secret, now.whole, now.box_id) != (document.editing, document.secret, document.whole, document.box_id);
        let moved = cursor != document.cursor;
        (document.editing, document.secret, document.cursor) = (editing, secret, cursor);
        (fresh, moved, document.sent > 0)
    });
    if fresh {
        start_afresh(editing, now);
    } else if !waiting {
        keep_up(now, moved);
    }
    show_marked();
    // (The menu shows the input view's actions: once it has the keyboard.)
    if editing && let Some(at) = ui.edit.menu_at.take() {
        show_edit_menu(at);
    }
}

/// Another box has focus, or none: the document is the new box's, and the keyboard goes to the
/// input view (starting again, it reads the traits afresh) or comes back to the library's view.
fn start_afresh(editing: bool, now: Focused) {
    DOCUMENT.with_borrow_mut(|document| {
        (document.whole, document.box_id, document.text, document.selection) = (now.whole, now.box_id, now.text, now.selection);
        document.marked.clear();
    });
    hide_marked();
    let (input, view) = (INPUT.get(), crate::ios::view());
    // SAFETY: plain requests of two views of the library's.
    unsafe {
        if editing {
            send::<Bool>(input, c"resignFirstResponder", ());
            send::<Bool>(input, c"becomeFirstResponder", ());
        } else {
            send::<Bool>(view, c"becomeFirstResponder", ());
        }
    }
}

/// The same box has focus: what changed in it besides the keyboard's own changes (the app's, or
/// a key's) is the document's now, and the keyboard hears it. What an input method composed
/// over text that changed is dropped.
fn keep_up(now: Focused, cursor_moved: bool) {
    let (text_changed, selection_changed) = DOCUMENT.with_borrow_mut(|document| {
        let changed = (document.text != now.text, document.selection != now.selection);
        (document.text, document.selection) = (now.text, now.selection);
        if changed.0 {
            document.marked.clear();
        }
        changed
    });
    if text_changed {
        return tell(c"textWillChange:", c"textDidChange:");
    }
    if selection_changed || (cursor_moved && !now.whole) {
        tell(c"selectionWillChange:", c"selectionDidChange:");
    }
}

/// Tells the keyboard (the input view's delegate) of a change it did not make.
fn tell(will: &CStr, did: &CStr) {
    let delegate = DOCUMENT.with_borrow(|document| document.delegate);
    // SAFETY: the keyboard's delegate (nil answers nothing) hears it.
    unsafe {
        send::<()>(delegate, will, (INPUT.get(),));
        send::<()>(delegate, did, (INPUT.get(),));
    }
}

extern "C" fn yes_please(_: Obj, _: Sel) -> Bool {
    1
}

pub(crate) extern "C" fn ignored(_: Obj, _: Sel, _: Obj) {}

pub(crate) extern "C" fn ignored_direction(_: Obj, _: Sel, _: isize, _: Obj) {}

// ---- the document: the box's text, with what is composed in place ----

/// UTF-16 units in `text` before byte `at`.
fn units(text: &str, at: usize) -> usize {
    text.get(..at).map_or_else(|| text.encode_utf16().count(), |before| before.encode_utf16().count())
}

/// The byte of `text` at UTF-16 unit `at` (the end, past it).
fn byte_at(text: &str, at: usize) -> usize {
    let mut counted = 0;
    for (byte, letter) in text.char_indices() {
        if counted >= at {
            return byte;
        }
        counted += letter.len_utf16();
    }
    text.len()
}

/// The two ends in order.
fn sorted([one, other]: [usize; 2]) -> [usize; 2] {
    [one.min(other), one.max(other)]
}

impl Document {
    fn composing(&self) -> bool {
        !self.marked.is_empty()
    }
    /// The document: the text with what is composed in place of what it stands for.
    fn shown(&self) -> String {
        match self.composing() {
            true => [&self.text[..self.over[0]], &self.marked, &self.text[self.over[1]..]].concat(),
            false => self.text.clone(),
        }
    }
    fn length(&self) -> usize {
        units(&self.text, self.text.len()) + units(&self.marked, self.marked.len()) - self.over_units()
    }
    /// The UTF-16 units of the text that what is composed stands for.
    fn over_units(&self) -> usize {
        if self.composing() { units(&self.text[self.over[0]..], self.over[1] - self.over[0]) } else { 0 }
    }
    /// The byte of the text at a document position: inside what is composed, where it starts.
    fn byte_of(&self, at: usize) -> usize {
        if !self.composing() {
            return byte_at(&self.text, at);
        }
        let (before, marked) = (units(&self.text, self.over[0]), units(&self.marked, self.marked.len()));
        match at {
            _ if at <= before => byte_at(&self.text, at),
            _ if at < before + marked => self.over[0],
            _ => self.over[1] + byte_at(&self.text[self.over[1]..], at - before - marked),
        }
    }
    /// The document position of a byte of the text.
    fn position_of(&self, at: usize) -> usize {
        let unit = units(&self.text, at);
        if self.composing() && at >= self.over[1] { unit + units(&self.marked, self.marked.len()) - self.over_units() } else { unit }
    }
    /// What is selected, as document positions: inside what is composed, the input method's own
    /// selection.
    fn selected(&self) -> [usize; 2] {
        if self.composing() {
            let start = units(&self.text, self.over[0]) + self.picked.location;
            return [start, start + self.picked.length];
        }
        let [from, to] = sorted(self.selection);
        [self.position_of(from), self.position_of(to)]
    }
}

/// Replaces the text's bytes `range` with `with` and sets the selection (anchor, cursor): here
/// at once, and in the box as the next run's first stroke.
fn change(range: Range<usize>, with: &str, selection: [usize; 2]) {
    DOCUMENT.with_borrow_mut(|document| {
        document.text.replace_range(range.clone(), with);
        (document.selection, document.sent) = (selection, document.sent + 1);
    });
    let done = Box::new(Replace { range, with: with.into(), selection });
    heard(Heard::Other(Box::new(move |ui: &mut Ui| {
        ui.input.strokes.push(Stroke::Replace(done));
        DOCUMENT.with_borrow_mut(|document| document.sent -= 1);
        true
    })));
}

/// Commits what an input method composed: it replaces what it stood for, the cursor after it.
fn commit_marked() {
    let (over, marked) = DOCUMENT.with_borrow_mut(|document| (document.over, std::mem::take(&mut document.marked)));
    hide_marked();
    if marked.is_empty() && over[0] == over[1] {
        return;
    }
    let cursor = over[0] + marked.len();
    change(over[0]..over[1], &marked, [cursor; 2]);
}

/// Text typed, or committed by an input method, in place of the selection (or of what was
/// composed). Return and Tab are their keys (unless a hardware key already said so). Outside
/// a plain text box the text is typed as it comes.
fn typed(text: String) {
    let key = match text.as_str() {
        "\n" | "\r" => Some(Key::Enter),
        "\t" => Some(Key::Tab),
        _ => None,
    };
    let whole = DOCUMENT.with_borrow(|document| document.whole);
    match key {
        Some(key) => {
            commit_marked_or_drop(whole);
            if !crate::ios::key_held(key) {
                heard(Heard::Event(Event::Key(key, Mods::NONE)));
            }
        }
        None if whole => type_over(&text),
        None => {
            DOCUMENT.with_borrow_mut(|document| document.marked.clear());
            hide_marked();
            if !text.is_empty() {
                heard(Heard::Event(Event::Text(text)));
            }
        }
    }
}

/// What was composed is committed (in a plain text box), or forgotten.
fn commit_marked_or_drop(whole: bool) {
    if whole {
        return commit_marked();
    }
    let committed = DOCUMENT.with_borrow_mut(|document| std::mem::take(&mut document.marked));
    hide_marked();
    if !committed.is_empty() {
        heard(Heard::Event(Event::Text(committed)));
    }
}

/// Text in place of what was composed, or else of the selection; the cursor after it.
fn type_over(text: &str) {
    let range = DOCUMENT.with_borrow_mut(|document| match std::mem::take(&mut document.marked).is_empty() {
        true => sorted(document.selection),
        false => document.over,
    });
    hide_marked();
    change(range[0]..range[1], text, [range[0] + text.len(); 2]);
}

extern "C" fn has_text(_: Obj, _: Sel) -> Bool {
    Bool::from(DOCUMENT.with_borrow(|document| document.length() > 0))
}

extern "C" fn inserted(_: Obj, _: Sel, text: Obj) {
    typed(text_of(text));
}

/// The keyboard's Delete: what was composed goes; else the selection, or the character before
/// the cursor (unless a hardware key already deleted it).
extern "C" fn deleted(_: Obj, _: Sel) {
    let (composing, whole) = DOCUMENT.with_borrow(|document| (document.composing(), document.whole));
    if composing {
        DOCUMENT.with_borrow_mut(|document| document.marked.clear());
        hide_marked();
        return tell(c"textWillChange:", c"textDidChange:");
    }
    if crate::ios::key_held(Key::Backspace) {
        return;
    }
    let Some(boxes) = TEXT_BOXES.get().filter(|_| whole) else {
        return heard(Heard::Event(Event::Key(Key::Backspace, Mods::NONE)));
    };
    let [from, to] = DOCUMENT.with_borrow(|document| sorted(document.selection));
    let from = if from == to { DOCUMENT.with_borrow(|document| (boxes.before)(&document.text, to)) } else { from };
    if from < to {
        change(from..to, "", [from; 2]);
    }
}

/// The keyboard replaces part of the document (a corrected word): in a plain text box, that
/// part of the text, the selection moving with what follows it; elsewhere, typed.
extern "C" fn replaced(_: Obj, _: Sel, within: Obj, text: Obj) {
    let text = text_of(text);
    if !DOCUMENT.with_borrow(|document| document.whole) {
        return typed(text);
    }
    commit_marked();
    let [from, to] = ends_of(within);
    let (bytes, selection) = DOCUMENT.with_borrow(|document| {
        let bytes = document.byte_of(from)..document.byte_of(to);
        (bytes.clone(), document.selection.map(|at| moved_by(at, &bytes, text.len())))
    });
    change(bytes, &text, selection);
}

/// Where a place in the text goes when bytes `replaced` become `added` bytes: after them, by as
/// much as they grew; inside them, to their end.
fn moved_by(at: usize, replaced: &Range<usize>, added: usize) -> usize {
    match at {
        _ if at >= replaced.end => at + added - replaced.len(),
        _ if at > replaced.start => replaced.start + added,
        _ => at,
    }
}

/// The keyboard moves the cursor, or selects (in a plain text box, not while composing).
extern "C" fn set_selected_range(_: Obj, _: Sel, within: Obj) {
    let [from, to] = ends_of(within);
    let (whole, composing, cursor) = DOCUMENT.with_borrow(|document| (document.whole, document.composing(), document.selection[1]));
    if !whole || composing {
        return;
    }
    let selection = DOCUMENT.with_borrow(|document| [document.byte_of(from), document.byte_of(to)]);
    change(cursor..cursor, "", selection);
}

/// The selection, or within what is composed, the input method's.
extern "C" fn selected_range(_: Obj, _: Sel) -> Obj {
    let [from, to] = DOCUMENT.with_borrow(Document::selected);
    range(from, to)
}

extern "C" fn marked_range(_: Obj, _: Sel) -> Obj {
    let marked = DOCUMENT.with_borrow(|document| document.composing().then(|| units(&document.text, document.over[0])).map(|from| (from, units(&document.marked, document.marked.len()))));
    marked.map_or(std::ptr::null_mut(), |(from, long)| range(from, from + long))
}

extern "C" fn style(_: Obj, _: Sel) -> Obj {
    DOCUMENT.with_borrow(|document| document.style)
}

extern "C" fn set_style(_: Obj, _: Sel, style: Obj) {
    DOCUMENT.with_borrow_mut(|document| document.style = style);
}

/// An input method composes, in place of the selection (or of what it composed before).
/// Composing nothing ends it: in a plain text box, the selection it stood for goes with it.
extern "C" fn marked(_: Obj, _: Sel, text: Obj, picked: NSRange) {
    let text = text_of(text);
    let ended = DOCUMENT.with_borrow_mut(|document| {
        if !document.composing() {
            document.over = if document.whole { sorted(document.selection) } else { [0; 2] };
        }
        (document.marked, document.picked) = (text, picked);
        document.whole && !document.composing()
    });
    if ended {
        commit_marked();
    }
    show_marked();
}

/// The input method is done: what it composed is committed.
extern "C" fn unmarked(_: Obj, _: Sel) {
    let whole = DOCUMENT.with_borrow(|document| document.whole);
    commit_marked_or_drop(whole);
}

extern "C" fn text_in(_: Obj, _: Sel, within: Obj) -> Obj {
    let [from, to] = ends_of(within);
    let shown: Vec<u16> = DOCUMENT.with_borrow(|document| document.shown().encode_utf16().collect());
    ns_string(&String::from_utf16_lossy(&shown[from.min(shown.len())..to.min(shown.len())]))
}

/// The text in a plain or attributed string.
fn text_of(string: Obj) -> String {
    // SAFETY: both kinds answer `isKindOfClass:`; an attributed one answers `string`.
    let attributed = unsafe { send::<Bool>(string, c"isKindOfClass:", (class(c"NSAttributedString"),)) } != 0;
    let plain = if attributed { unsafe { send(string, c"string", ()) } } else { string };
    rust_string(plain).unwrap_or_default()
}

extern "C" fn delegate(_: Obj, _: Sel) -> Obj {
    DOCUMENT.with_borrow(|document| document.delegate)
}

extern "C" fn set_delegate(_: Obj, _: Sel, delegate: Obj) {
    DOCUMENT.with_borrow_mut(|document| document.delegate = delegate);
}

extern "C" fn tokenizer(_: Obj, _: Sel) -> Obj {
    DOCUMENT.with_borrow(|document| document.tokenizer)
}

// ---- positions and ranges ----

/// A position at an offset (autoreleased).
pub(crate) fn position(offset: usize) -> Obj {
    // SAFETY: an object of the library's, handed on autoreleased.
    unsafe { send(new_with_extra(position_class(), offset), c"autorelease", ()) }
}

/// A range between two offsets (autoreleased).
pub(crate) fn range(from: usize, to: usize) -> Obj {
    // SAFETY: an object of the library's, handed on autoreleased.
    unsafe { send(new_with_extra(range_class(), sorted([from, to])), c"autorelease", ()) }
}

/// Whether an object is of a class of the library's.
fn is_of(object: Obj, made: Obj) -> bool {
    // SAFETY: every object answers this; nil answers no.
    unsafe { send::<Bool>(object, c"isKindOfClass:", (made,)) != 0 }
}

/// A position's offset: 0 for one not the library's.
pub(crate) fn offset_of(position: Obj) -> usize {
    // SAFETY: a position of the library's class has its offset beside it.
    if is_of(position, position_class()) { unsafe { extra_of(position) } } else { 0 }
}

/// A range's two offsets: none for one not the library's.
pub(crate) fn ends_of(range: Obj) -> [usize; 2] {
    // SAFETY: a range of the library's class has its offsets beside it.
    if is_of(range, range_class()) { unsafe { extra_of(range) } } else { [0, 0] }
}

extern "C" fn range_start(range: Obj, _: Sel) -> Obj {
    position(ends_of(range)[0])
}

extern "C" fn range_end(range: Obj, _: Sel) -> Obj {
    position(ends_of(range)[1])
}

extern "C" fn range_empty(range: Obj, _: Sel) -> Bool {
    let [from, to] = ends_of(range);
    Bool::from(from == to)
}

/// The document's length, in UTF-16 units.
fn length() -> usize {
    DOCUMENT.with_borrow(Document::length)
}

pub(crate) extern "C" fn beginning(_: Obj, _: Sel) -> Obj {
    position(0)
}

extern "C" fn end(_: Obj, _: Sel) -> Obj {
    position(length())
}

pub(crate) extern "C" fn range_between(_: Obj, _: Sel, from: Obj, to: Obj) -> Obj {
    range(offset_of(from), offset_of(to))
}

/// A position `by` units from another in a document `length` units long; nil outside it.
pub(crate) fn moved_within(from: Obj, by: isize, length: usize) -> Obj {
    let to = offset_of(from) as isize + by;
    if (0..=length as isize).contains(&to) { position(to as usize) } else { std::ptr::null_mut() }
}

/// The same toward a side: left, up and backward count down.
pub(crate) fn moved_toward_within(from: Obj, direction: isize, by: isize, length: usize) -> Obj {
    let back = matches!(direction, LEFT | UP | BACKWARD);
    moved_within(from, if back { -by } else { by }, length)
}

/// The character next to a position toward a side, in a document `length` units long.
pub(crate) fn extended_within(from: Obj, direction: isize, length: usize) -> Obj {
    let at = offset_of(from);
    if matches!(direction, LEFT | UP) { range(at.saturating_sub(1), at) } else { range(at, (at + 1).min(length)) }
}

extern "C" fn moved(_: Obj, _: Sel, from: Obj, by: isize) -> Obj {
    moved_within(from, by, length())
}

/// The same toward a side; up and down go by lines, where the box's lines are known.
extern "C" fn moved_toward(_: Obj, _: Sel, from: Obj, direction: isize, by: isize) -> Obj {
    let lines = match direction {
        UP => Some(-by),
        DOWN => Some(by),
        _ => None,
    };
    if let Some(to) = lines.and_then(|lines| line_away(offset_of(from), lines)) {
        return position(to);
    }
    moved_toward_within(from, direction, by, length())
}

/// The position `lines` lines below (above, if negative) a position, at the same x.
fn line_away(at: usize, lines: isize) -> Option<usize> {
    let caret = caret_at(at)?;
    let to = pt(caret.x, caret.y + caret.h * (lines as f32 + 0.5));
    nearest_to(to)
}

pub(crate) extern "C" fn compared(_: Obj, _: Sel, one: Obj, other: Obj) -> isize {
    offset_of(one).cmp(&offset_of(other)) as isize
}

pub(crate) extern "C" fn offset_between(_: Obj, _: Sel, from: Obj, to: Obj) -> isize {
    offset_of(to) as isize - offset_of(from) as isize
}

pub(crate) extern "C" fn farthest(_: Obj, _: Sel, within: Obj, direction: isize) -> Obj {
    let [from, to] = ends_of(within);
    position(if matches!(direction, LEFT | UP) { from } else { to })
}

extern "C" fn extended(_: Obj, _: Sel, from: Obj, direction: isize) -> Obj {
    extended_within(from, direction, length())
}

pub(crate) extern "C" fn left_to_right(_: Obj, _: Sel, _: Obj, _: isize) -> isize {
    0
}

// ---- places on the screen, in page units (the input view's own) ----

/// Where the box's text lies as last laid out: its element, where its text starts and ends in
/// the tree's strings, its font and alignment, the room inside its padding, and its lines.
struct LaidOut {
    start: usize,
    end: usize,
    font: crate::text::Font,
    side: f32,
    inside: Rect,
    line_height: f32,
    lines: Vec<Line>,
}

/// The focused text box's text as last laid out.
fn laid_out(ui: &Ui) -> Option<LaidOut> {
    let at = ui.typed_in?;
    let (spec, place) = (ui.tree.specs.get(at as usize)?, ui.tree.places.get(at as usize)?);
    let Content::Text { start, end, font, wrap, side, .. } = spec.content else { return None };
    let (start, end) = (start as usize, end as usize);
    let lines = match wrap {
        true => ui.tree.lines.get(place.lines.0 as usize..place.lines.1 as usize)?.to_vec(),
        false => vec![Line { start, end, width: place.fit[0] }],
    };
    let side = match side {
        Align::Center => 0.5,
        Align::End => 1.0,
        _ => 0.0,
    };
    Some(LaidOut { start, end, font, side, inside: ui.tree.rect_of(at).inset(spec.frame.pad), line_height: place.line_height, lines })
}

/// A cursor's place on a line: where it stands (with `right-to-left`, of the two).
#[cfg(feature = "right-to-left")]
fn stands([x, _]: Carets) -> f32 {
    x
}
#[cfg(not(feature = "right-to-left"))]
fn stands(x: Carets) -> f32 {
    x
}

impl LaidOut {
    /// The line byte `at` of the tree's strings is on.
    fn row_of(&self, at: usize) -> usize {
        self.lines.iter().rposition(|line| line.start <= at).unwrap_or(0)
    }
    /// What line `row` marks for a selection (each piece's left and right, from the box's
    /// left) and where a cursor on it stands; as `Ui::place_cursor` measures it.
    fn marks(&self, ui: &mut Ui, row: usize, selection: [usize; 2], cursor: Option<usize>, pieces: &mut Vec<[f32; 2]>) -> Option<f32> {
        let line = *self.lines.get(row)?;
        let Ui { tree, text, scale, edit, .. } = ui;
        let paragraph = tree.strings[self.start..line.start].rfind('\n').map_or(self.start, |at| self.start + at + 1);
        let leftward = crate::bidi::leftward(&tree.strings[paragraph..self.end]);
        let words = &tree.strings[line.start..line.end];
        let carets = crate::text::marks(text.as_mut(), words, line.start, line.width, self.font, *scale, leftward, selection, cursor, edit.after, pieces);
        let shift = ((self.inside.w - line.width).max(0.0) * if leftward { 1.0 - self.side } else { self.side }).round();
        pieces.iter_mut().for_each(|piece| *piece = piece.map(|x| x + shift));
        Some(carets.map_or(shift, |carets| stands(carets) + shift))
    }
    /// The caret at byte `at` of the box's text.
    fn caret(&self, ui: &mut Ui, at: usize) -> Option<Rect> {
        let at = (self.start + at).min(self.end);
        let row = self.row_of(at);
        let x = self.marks(ui, row, [0; 2], Some(at), &mut Vec::new())?;
        Some(rect(self.inside.x + x, self.inside.y + row as f32 * self.line_height, 2.0, self.line_height))
    }
    /// The rectangles of bytes `from` to `to` of the box's text, a line at a time.
    fn pieces(&self, ui: &mut Ui, [from, to]: [usize; 2]) -> Vec<Rect> {
        let [from, to] = [(self.start + from).min(self.end), (self.start + to).min(self.end)];
        let (mut found, mut pieces) = (Vec::new(), Vec::new());
        for row in self.row_of(from)..=self.row_of(to) {
            self.marks(ui, row, [from, to], None, &mut pieces);
            let top = self.inside.y + row as f32 * self.line_height;
            found.extend(pieces.iter().map(|[left, right]| rect(self.inside.x + left, top, right - left, self.line_height)));
        }
        found
    }
    /// The byte of the box's text nearest a point: on the line there, the cursor position
    /// standing closest.
    fn nearest(&self, ui: &mut Ui, to: Point) -> Option<usize> {
        let row = (((to.y - self.inside.y) / self.line_height.max(1.0)).max(0.0) as usize).min(self.lines.len().checked_sub(1)?);
        let line = self.lines[row];
        let (mut best, mut at) = ((f32::INFINITY, line.start), line.start);
        loop {
            let x = self.marks(ui, row, [0; 2], Some(at), &mut Vec::new())?;
            let off = (self.inside.x + x - to.x).abs();
            if off < best.0 {
                best = (off, at);
            }
            if at >= line.end {
                return Some(best.1 - self.start);
            }
            at = (self.start + step(&ui.tree.strings[self.start..self.end], at - self.start, true)).min(line.end);
        }
    }
}

/// Measures on the box's text as last laid out, in a plain text box; none while a run is going
/// on.
fn measured<T>(work: impl FnOnce(&mut Ui, TextBoxes) -> Option<T>) -> Option<T> {
    let boxes = TEXT_BOXES.get().filter(|_| DOCUMENT.with_borrow(|document| document.whole))?;
    crate::ios::with_ui(|ui| work(ui, boxes)).flatten()
}

/// The caret at a document position.
fn caret_at(at: usize) -> Option<Rect> {
    let byte = DOCUMENT.with_borrow(|document| document.byte_of(at));
    measured(|ui, boxes| (boxes.caret)(ui, byte))
}

/// The document position nearest a point. (An empty box shows its hint: none there.)
fn nearest_to(to: Point) -> Option<usize> {
    let byte = measured(|ui, boxes| (boxes.nearest)(ui, to))?;
    Some(DOCUMENT.with_borrow(|document| document.position_of(byte.min(document.text.len()))))
}

/// The rectangles of a document range.
fn pieces_of(within: Obj) -> Vec<Rect> {
    let [from, to] = ends_of(within);
    let bytes = DOCUMENT.with_borrow(|document| [document.byte_of(from), document.byte_of(to)]);
    measured(|ui, boxes| Some((boxes.pieces)(ui, bytes))).unwrap_or_default()
}

fn ns_rect(at: Rect) -> NSRect {
    NSRect::new(f64::from(at.x), f64::from(at.y), f64::from(at.w), f64::from(at.h))
}

/// The text cursor as last shown: where the keyboard's places fall back to.
fn cursor_rect() -> NSRect {
    ns_rect(DOCUMENT.with_borrow(|document| document.cursor).unwrap_or(rect(0.0, 0.0, 2.0, 1.0)))
}

/// Where a range is: what is composed, as shown; else its first line's piece.
extern "C" fn first_rect(_: Obj, _: Sel, within: Obj) -> NSRect {
    let label = DOCUMENT.with_borrow(|document| if document.composing() { document.label } else { std::ptr::null_mut() });
    if !label.is_null() {
        let origin = crate::ios::origin();
        // SAFETY: a label answers its frame, in the library's view.
        let frame = unsafe { send_rect(label, c"frame", ()) };
        return NSRect::new(frame.origin.x - origin.x, frame.origin.y - origin.y, frame.size.width, frame.size.height);
    }
    pieces_of(within).first().map_or_else(cursor_rect, |piece| ns_rect(*piece))
}

extern "C" fn caret_rect(_: Obj, _: Sel, at: Obj) -> NSRect {
    caret_at(offset_of(at)).map_or_else(cursor_rect, ns_rect)
}

/// A range's rectangles, a line at a time, the first holding its start and the last its end.
extern "C" fn selection_rects(_: Obj, _: Sel, within: Obj) -> Obj {
    let found = pieces_of(within);
    let last = found.len().saturating_sub(1);
    let made: Vec<Obj> = found.iter().enumerate().map(|(nth, at)| selection_rect(ns_rect(*at), nth == 0, nth == last)).collect();
    ns_array(&made)
}

extern "C" fn closest(_: Obj, _: Sel, to: NSPoint) -> Obj {
    position(nearest_to(pt(to.x as f32, to.y as f32)).unwrap_or_else(length))
}

extern "C" fn closest_within(_: Obj, _: Sel, to: NSPoint, within: Obj) -> Obj {
    let [from, end] = ends_of(within);
    position(nearest_to(pt(to.x as f32, to.y as f32)).unwrap_or(end).clamp(from, end))
}

/// The character at a point.
extern "C" fn range_at(_: Obj, _: Sel, to: NSPoint) -> Obj {
    let Some(at) = nearest_to(pt(to.x as f32, to.y as f32)) else { return std::ptr::null_mut() };
    range(at, (at + 1).min(length()))
}

/// A selection rectangle (autoreleased).
pub(crate) fn selection_rect(rect: NSRect, start: bool, end: bool) -> Obj {
    // SAFETY: an object of the library's, handed on autoreleased.
    unsafe { send(new_with_extra(selection_rect_class(), SelectionRect { rect, start, end }), c"autorelease", ()) }
}

/// What a selection rectangle keeps.
fn kept_by(object: Obj) -> SelectionRect {
    // SAFETY: only objects of the selection rectangle class answer its methods.
    unsafe { extra_of(object) }
}

extern "C" fn rect_of(object: Obj, _: Sel) -> NSRect {
    kept_by(object).rect
}

extern "C" fn holds_start(object: Obj, _: Sel) -> Bool {
    Bool::from(kept_by(object).start)
}

extern "C" fn holds_end(object: Obj, _: Sel) -> Bool {
    Bool::from(kept_by(object).end)
}

/// `NSWritingDirectionLeftToRight`.
extern "C" fn left_to_right_rect(_: Obj, _: Sel) -> isize {
    0
}

extern "C" fn not_vertical(_: Obj, _: Sel) -> Bool {
    0
}

// ---- traits ----

/// Corrections, spelling, smart quotes, dashes and spaces: the system's choice in a plain text
/// box; elsewhere off (the keyboard sees no text there).
extern "C" fn correcting(_: Obj, _: Sel) -> isize {
    if DOCUMENT.with_borrow(|document| document.whole) { DEFAULT } else { NO }
}

/// Capitals at the start of sentences in a plain text box; elsewhere none.
extern "C" fn capitals(_: Obj, _: Sel) -> isize {
    if DOCUMENT.with_borrow(|document| document.whole) { SENTENCES } else { 0 }
}

extern "C" fn secret(_: Obj, _: Sel) -> Bool {
    Bool::from(DOCUMENT.with_borrow(|document| document.secret))
}

// ---- the system's edit actions and menu ----

/// Command with a letter, as the keys would do it.
fn command(letter: char) {
    heard(Heard::Event(Event::Key(Key::Char(letter), Mods::CTRL)));
}

extern "C" fn copied(_: Obj, _: Sel, _: Obj) {
    command('c');
}

extern "C" fn cut(_: Obj, _: Sel, _: Obj) {
    command('x');
}

/// The system's Paste: the pasteboard is read now, while the system lets it be without asking.
extern "C" fn pasted(_: Obj, _: Sel, _: Obj) {
    crate::ios::paste_now();
    command('v');
}

extern "C" fn all_selected(_: Obj, _: Sel, _: Obj) {
    command('a');
}

/// The system's Select: the word at the cursor.
extern "C" fn word_selected(_: Obj, _: Sel, _: Obj) {
    let Some(boxes) = TEXT_BOXES.get().filter(|_| DOCUMENT.with_borrow(|document| document.whole)) else { return };
    let (word, cursor) = DOCUMENT.with_borrow(|document| ((boxes.word)(&document.text, document.selection[1]), document.selection[1]));
    change(cursor..cursor, "", word);
}

/// What the edit menu offers: Cut and Copy with a selection (not of a password), Paste with
/// text to paste, Select with none in a plain text box, and Select All.
extern "C" fn can_perform(_: Obj, _: Sel, action: Sel, _: Obj) -> Bool {
    let (secret, whole, [from, to], empty) = DOCUMENT.with_borrow(|document| (document.secret, document.whole, document.selected(), document.length() == 0));
    let picked = from != to && !secret;
    // SAFETY: a plain question of the pasteboard, which asks the person nothing.
    let pastes = || unsafe { send::<Bool>(crate::ios::pasteboard(), c"hasStrings", ()) } != 0;
    let offered = |name: &CStr| std::ptr::eq(sel(name), action);
    let can = match () {
        _ if offered(c"cut:") || offered(c"copy:") => picked,
        _ if offered(c"paste:") => pastes(),
        _ if offered(c"select:") => whole && from == to && !empty,
        _ => offered(c"selectAll:") && !empty,
    };
    Bool::from(can)
}

/// Shows the system's edit menu at a place on the page (made the first time). Before iOS 16,
/// the shared menu controller's.
fn show_edit_menu(at: Point) {
    let input = INPUT.get();
    let at = NSPoint { x: f64::from(at.x), y: f64::from(at.y) };
    if class(c"UIEditMenuInteraction").is_null() {
        // SAFETY: the shared menu controller shows the first responder's actions at a place.
        return unsafe { send::<()>(send::<Obj>(class(c"UIMenuController"), c"sharedMenuController", ()), c"showMenuFromView:rect:", (input, NSRect::new(at.x, at.y, 1.0, 1.0))) };
    }
    if DOCUMENT.with_borrow(|document| document.menu.is_null()) {
        let menu = edit_menu(input);
        DOCUMENT.with_borrow_mut(|document| document.menu = menu);
    }
    let menu = DOCUMENT.with_borrow(|document| document.menu);
    // SAFETY: a configuration at the place, autoreleased, presented by the input view's
    // interaction.
    unsafe {
        let asked: Obj = send(class(c"UIEditMenuConfiguration"), c"configurationWithIdentifier:sourcePoint:", (std::ptr::null_mut::<c_void>(), at));
        send::<()>(menu, c"presentEditMenuWithConfiguration:", (asked,));
    }
}

/// The edit menu's interaction, handed to the input view, which keeps it.
fn edit_menu(input: Obj) -> Obj {
    // SAFETY: an interaction without a delegate (the menu holds the responder's actions),
    // handed to the view.
    unsafe {
        let menu: Obj = send(send::<Obj>(class(c"UIEditMenuInteraction"), c"alloc", ()), c"initWithDelegate:", (std::ptr::null_mut::<c_void>(),));
        send::<()>(input, c"addInteraction:", (menu,));
        send::<()>(menu, c"release", ());
        menu
    }
}

// ---- the long press in a text box ----

/// The long press's own class: the view knows it from other long presses (a drag's lift).
fn long_press_class() -> Obj {
    made_class(c"TautLongPress", c"UILongPressGestureRecognizer", &[], &[])
}

/// A long press on the text box being typed in shows the system's edit menu: the view learns to
/// hear it, and to let it begin only there.
fn hold_in_text_boxes(view: Obj) {
    let methods: [(&CStr, *const c_void, String); 2] = [(c"held:", held as *const c_void, "v@:@".into()), (c"gestureRecognizerShouldBegin:", may_begin as *const c_void, format!("{BOOL}@:@"))];
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    add_methods(crate::ios::view_class(), &listed);
    // SAFETY: a recognizer made with the view as its target, handed to the view, which keeps it.
    unsafe {
        let gesture: Obj = send(send::<Obj>(long_press_class(), c"alloc", ()), c"initWithTarget:action:", (view, sel(c"held:")));
        send::<()>(gesture, c"setCancelsTouchesInView:", (0 as Bool,));
        send::<()>(view, c"addGestureRecognizer:", (gesture,));
        send::<()>(gesture, c"release", ());
    }
}

/// The long press: the system's edit menu where it is.
extern "C" fn held(view: Obj, _: Sel, gesture: Obj) {
    if crate::ios::state_of(gesture) == crate::ios::BEGAN {
        show_edit_menu(crate::ios::place_of(view, gesture));
    }
}

/// Whether one of the view's gestures may begin: the long press only on the text box being
/// typed in, held as the mouse (elsewhere a long press is the touch feature's right click);
/// others as UIView says.
extern "C" fn may_begin(view: Obj, _: Sel, gesture: Obj) -> Bool {
    if !is_of(gesture, long_press_class()) {
        // SAFETY: UIView's own answer.
        return unsafe { crate::ios::send_super(view, c"UIView", c"gestureRecognizerShouldBegin:", (gesture,)) };
    }
    let at = crate::ios::place_of(view, gesture);
    let typing_here = |ui: &mut Ui| ui.ime_area().is_some() && ui.input.down[0] && ui.visible_boxes.rects.get(&ui.focus()).is_some_and(|rect| rect.contains(at));
    Bool::from(crate::ios::with_ui(typing_here).unwrap_or(false))
}

// ---- what is composed, shown ----

/// Shows what is composed over the text cursor, underlined, the selected part thicker, on the
/// system's background; hides it when nothing is.
fn show_marked() {
    let (text, picked, cursor) = DOCUMENT.with_borrow(|document| (document.marked.clone(), document.picked, document.cursor));
    let Some(at) = cursor.filter(|_| !text.is_empty()) else { return hide_marked() };
    let label = label();
    let origin = crate::ios::origin();
    let (x, y) = (f64::from(at.x) + origin.x, f64::from(at.y) + origin.y);
    // SAFETY: a string made and handed to the label, which keeps it; the label is moved.
    unsafe {
        send::<()>(label, c"setAttributedText:", (dressed(&text, picked),));
        let font: Obj = send(class(c"UIFont"), c"systemFontOfSize:", (f64::from(at.h * 0.8),));
        send::<()>(label, c"setFont:", (font,));
        send::<()>(label, c"sizeToFit", ());
        let size = send_rect(label, c"frame", ()).size;
        send::<()>(label, c"setFrame:", (NSRect::new(x, y, size.width, f64::from(at.h).max(size.height)),));
        send::<()>(label, c"setHidden:", (0 as Bool,));
    }
}

fn hide_marked() {
    let label = DOCUMENT.with_borrow(|document| document.label);
    // SAFETY: a label (nil answers nothing) is hidden.
    unsafe { send::<()>(label, c"setHidden:", (1 as Bool,)) };
}

/// The label showing what is composed, made the first time, in the library's view.
fn label() -> Obj {
    let known = DOCUMENT.with_borrow(|document| document.label);
    if !known.is_null() {
        return known;
    }
    // SAFETY: a label made on the main thread and handed to the view, which keeps it.
    let label = unsafe {
        let label: Obj = send(send::<Obj>(class(c"UILabel"), c"alloc", ()), c"initWithFrame:", (NSRect::default(),));
        send::<()>(label, c"setUserInteractionEnabled:", (0 as Bool,));
        send::<()>(label, c"setBackgroundColor:", (send::<Obj>(class(c"UIColor"), c"systemBackgroundColor", ()),));
        send::<()>(label, c"setTextColor:", (send::<Obj>(class(c"UIColor"), c"labelColor", ()),));
        send::<()>(crate::ios::view(), c"addSubview:", (label,));
        send::<()>(label, c"release", ());
        label
    };
    DOCUMENT.with_borrow_mut(|document| document.label = label);
    label
}

/// The composed text as shown: underlined, its selected part thick (autoreleased).
fn dressed(text: &str, picked: NSRange) -> Obj {
    let number = |style: isize| -> Obj { unsafe { send(class(c"NSNumber"), c"numberWithInteger:", (style,)) } };
    let long = text.encode_utf16().count();
    // SAFETY: an attributed string made from the text, marked over ranges inside it.
    unsafe {
        let made: Obj = send(send::<Obj>(class(c"NSMutableAttributedString"), c"alloc", ()), c"initWithString:", (ns_string(text),));
        send::<()>(made, c"addAttribute:value:range:", (NSUnderlineStyleAttributeName, number(SINGLE), NSRange { location: 0, length: long }));
        let from = picked.location.min(long);
        let within = NSRange { location: from, length: picked.length.min(long - from) };
        if within.length > 0 {
            send::<()>(made, c"addAttribute:value:range:", (NSUnderlineStyleAttributeName, number(THICK), within));
            let tint: Obj = send(class(c"UIColor"), c"systemFillColor", ());
            send::<()>(made, c"addAttribute:value:range:", (NSBackgroundColorAttributeName, tint, within));
        }
        send(made, c"autorelease", ())
    }
}
