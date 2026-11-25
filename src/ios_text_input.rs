//! Typed text on iOS: a view of the library's own (`TautTextInput`), a sliver over the text
//! cursor, is the first responder while a text box has focus, so the system's keyboard shows,
//! and a hardware keyboard and input methods type through it. Without focus in a text box, the
//! library's view is, and the keyboard goes.
//!
//! It is a `UITextInput` whose document is only what an input method is composing (its
//! *marked text*), as on macOS: the keyboard sees none of the text box's text, so it neither
//! corrects nor predicts words, and capitalizes nothing by itself. Positions are UTF-16 units
//! into that text, kept in objects of the library's position and range classes. What an input
//! method composes is shown over the text cursor, underlined, its selected part thicker; what
//! it commits, and what is typed, is [`Event::Text`]. Return, Tab and Delete from the system's
//! keyboard are keys.
//!
//! The system's Copy, Cut, Paste and Select All (its edit menu, the shortcut bar over an iPad's
//! keyboard, the menu bar) are the keys that do them, Command with C, X, V or A.
use crate::event::{Event, Key, Mods};
use crate::geom::Rect;
use crate::ios::{Heard, heard};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};

#[link(name = "objc")]
unsafe extern "C" {
    fn class_createInstance(class: Obj, extra: usize) -> Obj;
    fn object_getIndexedIvars(object: Obj) -> *mut c_void;
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static NSUnderlineStyleAttributeName: Obj;
    static NSBackgroundColorAttributeName: Obj;
}

/// `UITextLayoutDirection` and `UITextStorageDirection`: what counts as backward.
const LEFT: isize = 3;
const UP: isize = 4;
const BACKWARD: isize = 1;
/// Underline styles: single, thick.
const SINGLE: isize = 1;
const THICK: isize = 2;

/// What the input view keeps: what is composed and its selected part (UTF-16 units), whether a
/// text box has the keyboard, and where its text cursor is (in the library's view's points).
#[derive(Default)]
struct Composing {
    marked: Vec<u16>,
    picked: NSRange,
    editing: bool,
    secret: bool,
    cursor: Option<Rect>,
    delegate: Obj,
    tokenizer: Obj,
    style: Obj,
    label: Obj,
}

thread_local! {
    static INPUT: Cell<Obj> = const { Cell::new(std::ptr::null_mut()) };
    static COMPOSING: RefCell<Composing> = RefCell::new(Composing::default());
}

/// A plain method's types: an object, a `BOOL`, a number.
const OBJECT: &str = "@@:";
const NUMBER: &str = "q@:";

/// The input view's class.
fn input_class() -> Obj {
    let (yes, rect) = (format!("{BOOL}@:"), format!("{RECT}@:@"));
    let methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"canBecomeFirstResponder", yes_please as *const c_void, yes.clone()),
        (c"hasText", yes_please as *const c_void, yes),
        (c"insertText:", inserted as *const c_void, "v@:@".into()),
        (c"deleteBackward", deleted as *const c_void, "v@:".into()),
        (c"textInRange:", text_in as *const c_void, "@@:@".into()),
        (c"replaceRange:withText:", replaced as *const c_void, "v@:@@".into()),
        (c"selectedTextRange", selected_range as *const c_void, OBJECT.into()),
        (c"setSelectedTextRange:", ignored as *const c_void, "v@:@".into()),
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
        (c"selectionRectsForRange:", no_rects as *const c_void, "@@:@".into()),
        (c"closestPositionToPoint:", closest as *const c_void, format!("@@:{POINT}")),
        (c"closestPositionToPoint:withinRange:", closest_within as *const c_void, format!("@@:{POINT}@")),
        (c"characterRangeAtPoint:", no_range as *const c_void, format!("@@:{POINT}")),
        (c"autocorrectionType", no_trait as *const c_void, NUMBER.into()),
        (c"spellCheckingType", no_trait as *const c_void, NUMBER.into()),
        (c"smartQuotesType", no_trait as *const c_void, NUMBER.into()),
        (c"smartDashesType", no_trait as *const c_void, NUMBER.into()),
        (c"smartInsertDeleteType", no_trait as *const c_void, NUMBER.into()),
        (c"autocapitalizationType", none_trait as *const c_void, NUMBER.into()),
        (c"isSecureTextEntry", secret as *const c_void, format!("{BOOL}@:")),
        (c"copy:", copied as *const c_void, "v@:@".into()),
        (c"cut:", cut as *const c_void, "v@:@".into()),
        (c"paste:", pasted as *const c_void, "v@:@".into()),
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

/// Makes the input view, inside the library's view: clear, touched by nothing.
pub(crate) fn open(view: Obj) {
    // SAFETY: a view of the library's class made on the main thread; the library's view keeps
    // it, and it is kept here too.
    unsafe {
        let input: Obj = send(send::<Obj>(input_class(), c"alloc", ()), c"initWithFrame:", (NSRect::new(0.0, 0.0, 1.0, 1.0),));
        send::<()>(input, c"setUserInteractionEnabled:", (0 as Bool,));
        send::<()>(input, c"setBackgroundColor:", (send::<Obj>(class(c"UIColor"), c"clearColor", ()),));
        send::<()>(view, c"addSubview:", (input,));
        let tokenizer: Obj = send(send::<Obj>(class(c"UITextInputStringTokenizer"), c"alloc", ()), c"initWithTextInput:", (input,));
        COMPOSING.with_borrow_mut(|composing| composing.tokenizer = tokenizer);
        INPUT.set(input);
    }
}

/// After a run: the input view takes the keyboard while a text box has focus, and gives it back
/// to the library's view after; it lies over the text cursor. A password box starts the keyboard
/// afresh.
pub(crate) fn follow(view: Obj, ui: &mut Ui) {
    let input = INPUT.get();
    let origin = crate::ios::origin();
    let cursor = ui.ime_area().map(|at| Rect { x: at.x + origin.x as f32, y: at.y + origin.y as f32, ..at });
    let secret = ui.password_focused();
    let (was_editing, was_secret, was_at) = COMPOSING.with_borrow(|composing| (composing.editing, composing.secret, composing.cursor));
    let editing = cursor.is_some();
    COMPOSING.with_borrow_mut(|composing| (composing.editing, composing.secret, composing.cursor) = (editing, secret, cursor));
    if let Some(at) = cursor {
        // SAFETY: the input view moves over the text cursor.
        unsafe { send::<()>(input, c"setFrame:", (NSRect::new(f64::from(at.x), f64::from(at.y), 1.0, f64::from(at.h.max(1.0))),)) };
    }
    if editing == was_editing && secret == was_secret {
        if cursor != was_at {
            moved_cursor();
        }
        return show_marked();
    }
    drop_marked();
    // SAFETY: the keyboard goes to the input view or comes back to the library's view; starting
    // again, the keyboard reads the traits afresh (a password box's).
    unsafe {
        if editing {
            send::<Bool>(input, c"resignFirstResponder", ());
            send::<Bool>(input, c"becomeFirstResponder", ());
        } else {
            send::<Bool>(view, c"becomeFirstResponder", ());
        }
    }
}

/// The text cursor moved: the keyboard hears the selection changed (and starts a word afresh).
fn moved_cursor() {
    let delegate = COMPOSING.with_borrow(|composing| composing.delegate);
    // SAFETY: the keyboard's delegate (nil answers nothing) hears it.
    unsafe {
        send::<()>(delegate, c"selectionWillChange:", (INPUT.get(),));
        send::<()>(delegate, c"selectionDidChange:", (INPUT.get(),));
    }
}

/// Forgets what an input method composed, and tells the keyboard the text changed.
fn drop_marked() {
    let delegate = COMPOSING.with_borrow_mut(|composing| {
        composing.marked.clear();
        composing.delegate
    });
    hide_marked();
    // SAFETY: the keyboard's delegate (nil answers nothing) hears the document changed.
    unsafe {
        send::<()>(delegate, c"textWillChange:", (INPUT.get(),));
        send::<()>(delegate, c"textDidChange:", (INPUT.get(),));
    }
}

extern "C" fn yes_please(_: Obj, _: Sel) -> Bool {
    1
}

pub(crate) extern "C" fn ignored(_: Obj, _: Sel, _: Obj) {}

pub(crate) extern "C" fn ignored_direction(_: Obj, _: Sel, _: isize, _: Obj) {}

// ---- positions and ranges ----

/// A position at an offset (autoreleased).
pub(crate) fn position(offset: usize) -> Obj {
    // SAFETY: an object of the position class with room for an offset beside it, written once.
    unsafe {
        let made = class_createInstance(position_class(), size_of::<usize>());
        object_getIndexedIvars(made).cast::<usize>().write(offset);
        send(made, c"autorelease", ())
    }
}

/// A range between two offsets (autoreleased).
pub(crate) fn range(from: usize, to: usize) -> Obj {
    // SAFETY: an object of the range class with room for two offsets beside it, written once.
    unsafe {
        let made = class_createInstance(range_class(), 2 * size_of::<usize>());
        object_getIndexedIvars(made).cast::<[usize; 2]>().write([from.min(to), from.max(to)]);
        send(made, c"autorelease", ())
    }
}

/// Whether an object is of a class of the library's.
fn is_of(object: Obj, made: Obj) -> bool {
    // SAFETY: every object answers this; nil answers no.
    unsafe { send::<Bool>(object, c"isKindOfClass:", (made,)) != 0 }
}

/// A position's offset: 0 for one not the library's.
pub(crate) fn offset_of(position: Obj) -> usize {
    // SAFETY: a position of the library's class has its offset beside it.
    if is_of(position, position_class()) { unsafe { object_getIndexedIvars(position).cast::<usize>().read() } } else { 0 }
}

/// A range's two offsets: none for one not the library's.
pub(crate) fn ends_of(range: Obj) -> [usize; 2] {
    // SAFETY: a range of the library's class has its offsets beside it.
    if is_of(range, range_class()) { unsafe { object_getIndexedIvars(range).cast::<[usize; 2]>().read() } } else { [0, 0] }
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

/// The document's length: what is composed.
fn length() -> usize {
    COMPOSING.with_borrow(|composing| composing.marked.len())
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

extern "C" fn moved_toward(_: Obj, _: Sel, from: Obj, direction: isize, by: isize) -> Obj {
    moved_toward_within(from, direction, by, length())
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

// ---- the document: what is composed ----

/// The composed text between two offsets.
fn piece(from: usize, to: usize) -> String {
    COMPOSING.with_borrow(|composing| {
        let marked = &composing.marked;
        String::from_utf16_lossy(&marked[from.min(marked.len())..to.min(marked.len())])
    })
}

extern "C" fn text_in(_: Obj, _: Sel, within: Obj) -> Obj {
    let [from, to] = ends_of(within);
    ns_string(&piece(from, to))
}

/// The text in a plain or attributed string.
fn text_of(string: Obj) -> String {
    // SAFETY: both kinds answer `isKindOfClass:`; an attributed one answers `string`.
    let attributed = unsafe { send::<Bool>(string, c"isKindOfClass:", (class(c"NSAttributedString"),)) } != 0;
    let plain = if attributed { unsafe { send(string, c"string", ()) } } else { string };
    rust_string(plain).unwrap_or_default()
}

/// Text typed, or committed by an input method: Return and Tab as their keys (unless a
/// hardware key already said so); the rest as text.
fn typed(text: String) {
    COMPOSING.with_borrow_mut(|composing| composing.marked.clear());
    hide_marked();
    let key = match text.as_str() {
        "\n" | "\r" => Some(Key::Enter),
        "\t" => Some(Key::Tab),
        _ => None,
    };
    match key {
        Some(key) if crate::ios::key_held(key) => {}
        Some(key) => heard(Heard::Event(Event::Key(key, Mods::NONE))),
        None if !text.is_empty() => heard(Heard::Event(Event::Text(text))),
        None => {}
    }
}

extern "C" fn inserted(_: Obj, _: Sel, text: Obj) {
    typed(text_of(text));
}

/// The keyboard's Delete: Backspace (unless a hardware key already said so).
extern "C" fn deleted(_: Obj, _: Sel) {
    if length() > 0 {
        return drop_marked();
    }
    if !crate::ios::key_held(Key::Backspace) {
        heard(Heard::Event(Event::Key(Key::Backspace, Mods::NONE)));
    }
}

/// A keyboard replaces part of the document: what composed there is committed as this text.
extern "C" fn replaced(_: Obj, _: Sel, _: Obj, text: Obj) {
    typed(text_of(text));
}

/// The input method's selection within what it composes; else an empty one at the end.
extern "C" fn selected_range(_: Obj, _: Sel) -> Obj {
    let (picked, long) = COMPOSING.with_borrow(|composing| (composing.picked, composing.marked.len()));
    if long == 0 { range(0, 0) } else { range(picked.location.min(long), (picked.location + picked.length).min(long)) }
}

extern "C" fn marked_range(_: Obj, _: Sel) -> Obj {
    let long = length();
    if long == 0 { std::ptr::null_mut() } else { range(0, long) }
}

extern "C" fn style(_: Obj, _: Sel) -> Obj {
    COMPOSING.with_borrow(|composing| composing.style)
}

extern "C" fn set_style(_: Obj, _: Sel, style: Obj) {
    COMPOSING.with_borrow_mut(|composing| composing.style = style);
}

/// An input method composes: noted, and shown over the text cursor. Nothing composed ends it.
extern "C" fn marked(_: Obj, _: Sel, text: Obj, picked: NSRange) {
    let text = text_of(text);
    COMPOSING.with_borrow_mut(|composing| (composing.marked, composing.picked) = (text.encode_utf16().collect(), picked));
    show_marked();
}

/// The input method is done: what it composed is committed.
extern "C" fn unmarked(_: Obj, _: Sel) {
    let text = piece(0, length());
    typed(text);
}

extern "C" fn delegate(_: Obj, _: Sel) -> Obj {
    COMPOSING.with_borrow(|composing| composing.delegate)
}

extern "C" fn set_delegate(_: Obj, _: Sel, delegate: Obj) {
    COMPOSING.with_borrow_mut(|composing| composing.delegate = delegate);
}

extern "C" fn tokenizer(_: Obj, _: Sel) -> Obj {
    COMPOSING.with_borrow(|composing| composing.tokenizer)
}

/// Where composed text is, in the input view: from the text cursor, as wide as shown.
extern "C" fn first_rect(_: Obj, _: Sel, _: Obj) -> NSRect {
    let (height, width) = COMPOSING.with_borrow(|composing| (composing.cursor.map_or(1.0, |at| at.h), composing.label));
    // SAFETY: a label answers its frame (nil answers nothing).
    let wide = unsafe { send_rect(width, c"frame", ()) }.size.width.max(1.0);
    NSRect::new(0.0, 0.0, wide, f64::from(height))
}

extern "C" fn caret_rect(_: Obj, _: Sel, _: Obj) -> NSRect {
    let height = COMPOSING.with_borrow(|composing| composing.cursor.map_or(1.0, |at| at.h));
    NSRect::new(0.0, 0.0, 2.0, f64::from(height))
}

extern "C" fn no_rects(_: Obj, _: Sel, _: Obj) -> Obj {
    ns_array(&[])
}

extern "C" fn closest(_: Obj, _: Sel, _: NSPoint) -> Obj {
    position(length())
}

extern "C" fn closest_within(_: Obj, _: Sel, _: NSPoint, within: Obj) -> Obj {
    position(ends_of(within)[1])
}

extern "C" fn no_range(_: Obj, _: Sel, _: NSPoint) -> Obj {
    std::ptr::null_mut()
}

/// Corrections, spelling, smart quotes, dashes and spaces: off (`No` is 1).
extern "C" fn no_trait(_: Obj, _: Sel) -> isize {
    1
}

/// Capitals made by the keyboard: none (0).
extern "C" fn none_trait(_: Obj, _: Sel) -> isize {
    0
}

extern "C" fn secret(_: Obj, _: Sel) -> Bool {
    Bool::from(COMPOSING.with_borrow(|composing| composing.secret))
}

// ---- the system's edit actions ----

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

/// Copy, Cut, Paste and Select All are offered; nothing else.
extern "C" fn can_perform(_: Obj, _: Sel, action: Sel, _: Obj) -> Bool {
    let offered = [c"copy:", c"cut:", c"paste:", c"selectAll:"];
    Bool::from(offered.into_iter().any(|name| std::ptr::eq(sel(name), action)))
}

// ---- what is composed, shown ----

/// Shows what is composed over the text cursor, underlined, the selected part thicker, on the
/// system's background; hides it when nothing is.
fn show_marked() {
    let (text, picked, cursor) = COMPOSING.with_borrow(|composing| (String::from_utf16_lossy(&composing.marked), composing.picked, composing.cursor));
    let Some(at) = cursor.filter(|_| !text.is_empty()) else { return hide_marked() };
    let label = label();
    // SAFETY: a string made and handed to the label, which keeps it; the label is moved.
    unsafe {
        send::<()>(label, c"setAttributedText:", (dressed(&text, picked),));
        let font: Obj = send(class(c"UIFont"), c"systemFontOfSize:", (f64::from(at.h * 0.8),));
        send::<()>(label, c"setFont:", (font,));
        send::<()>(label, c"sizeToFit", ());
        let size = send_rect(label, c"frame", ()).size;
        send::<()>(label, c"setFrame:", (NSRect::new(f64::from(at.x), f64::from(at.y), size.width, f64::from(at.h).max(size.height)),));
        send::<()>(label, c"setHidden:", (0 as Bool,));
    }
}

fn hide_marked() {
    let label = COMPOSING.with_borrow(|composing| composing.label);
    // SAFETY: a label (nil answers nothing) is hidden.
    unsafe { send::<()>(label, c"setHidden:", (1 as Bool,)) };
}

/// The label showing what is composed, made the first time, in the library's view.
fn label() -> Obj {
    let known = COMPOSING.with_borrow(|composing| composing.label);
    if !known.is_null() {
        return known;
    }
    // SAFETY: a label made on the main thread and handed to the view, which keeps it.
    let label = unsafe {
        let label: Obj = send(send::<Obj>(class(c"UILabel"), c"alloc", ()), c"initWithFrame:", (NSRect::default(),));
        send::<()>(label, c"setUserInteractionEnabled:", (0 as Bool,));
        send::<()>(label, c"setBackgroundColor:", (send::<Obj>(class(c"UIColor"), c"systemBackgroundColor", ()),));
        send::<()>(label, c"setTextColor:", (send::<Obj>(class(c"UIColor"), c"labelColor", ()),));
        let input = INPUT.get();
        send::<()>(send::<Obj>(input, c"superview", ()), c"addSubview:", (label,));
        send::<()>(label, c"release", ());
        label
    };
    COMPOSING.with_borrow_mut(|composing| composing.label = label);
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
