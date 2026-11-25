//! Text as iOS readers read it (see `ui_accessibility`): by line, as wrapped, through each text
//! element's `UIAccessibilityReadingContent`; by letter, word and selection through a
//! `UITextInput` of the library's (`TautTextReader`), the element's
//! `accessibilityTextInputResponder`. Its document is the element's text, read only: positions
//! are UTF-16 units into it (as in `ios_text_input`), its rectangles are where the letters are
//! drawn, in the library's view. A text box's selection is its own, and a reader setting it
//! selects in the box; other text (a password box's too) keeps the reader's selection here.
use crate::accessibility::{AccessNode, Role, letter_at_unit, units_before};
use crate::geom::Rect;
use crate::ios_text_input::{
    beginning, compared, ends_of, extended_within, farthest, ignored, ignored_direction, left_to_right, moved_toward_within, moved_within, offset_between, offset_of, position, range, range_between,
};
use crate::macos_sys::*;
use crate::ui_accessibility::{Told, act, with_node};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_void};

#[link(name = "objc")]
unsafe extern "C" {
    fn class_createInstance(class: Obj, extra: usize) -> Obj;
    fn object_getIndexedIvars(object: Obj) -> *mut c_void;
}

/// What reads an element's text: its reader object, the reader's tokenizer (words and lines,
/// found through the reader), and the selection of text that is not a text box's.
struct Reader {
    object: Obj,
    tokenizer: Obj,
    selection: [usize; 2],
}

thread_local! {
    /// The readers made, by their element.
    static READERS: RefCell<HashMap<usize, Reader>> = RefCell::new(HashMap::new());
}

/// What a text element answers besides its role: its lines, and its reader.
pub(crate) fn element_methods() -> Vec<(&'static CStr, *const c_void, String)> {
    vec![
        (c"accessibilityLineNumberForPoint:", line_at as *const c_void, format!("q@:{POINT}")),
        (c"accessibilityContentForLineNumber:", line_text as *const c_void, "@@:q".into()),
        (c"accessibilityFrameForLineNumber:", line_frame as *const c_void, format!("{RECT}@:q")),
        (c"accessibilityPageContent", page_text as *const c_void, "@@:".into()),
        (c"accessibilityTextInputResponder", reader_of as *const c_void, "@@:".into()),
    ]
}

/// Works with the node of a text element (and the window's nodes): none for anything else.
fn with_text<T>(element: Obj, work: impl FnOnce(&Told, &AccessNode) -> T) -> Option<T> {
    with_node(element, |told, _, node| node.reads_text().then(|| work(told, node))).flatten()
}

// ---- lines ----

/// The line (as wrapped) at a point on the screen. None (`NOT_FOUND`) off the text.
extern "C" fn line_at(element: Obj, _: Sel, at: NSPoint) -> isize {
    let line = with_text(element, |told, node| node.letter_at(told.screen_point_in_page(at)).map(|letter| node.line_of(letter) as isize)).flatten();
    line.unwrap_or(NOT_FOUND as isize)
}

/// A line's text, its break included.
extern "C" fn line_text(element: Obj, _: Sel, line: isize) -> Obj {
    let text = with_text(element, |_, node| node.line_range(line.max(0) as usize).map(|[from, to]| letters(node.text(), from, to))).flatten();
    text.map_or(std::ptr::null_mut(), |text| ns_string(&text))
}

/// Where a line shows on the screen: the box around its letters, cut to what shows.
extern "C" fn line_frame(element: Obj, _: Sel, line: isize) -> NSRect {
    let place = |told: &Told, node: &AccessNode| {
        let [from, to] = node.line_range(line.max(0) as usize)?;
        let letters = (from..to).map(|letter| node.letter_rect(letter)).reduce(|one, other| one.union(other))?;
        Some(told.on_screen(letters.intersect(node.seen)))
    };
    with_text(element, place).flatten().unwrap_or_default()
}

extern "C" fn page_text(element: Obj, _: Sel) -> Obj {
    with_text(element, |_, node| ns_string(node.text())).unwrap_or(std::ptr::null_mut())
}

/// Characters `from` to `to` of a text.
fn letters(text: &str, from: u32, to: u32) -> String {
    text.chars().skip(from as usize).take(to.saturating_sub(from) as usize).collect()
}

// ---- the reader: a UITextInput over the element's text ----

/// The reader class, made once.
fn reader_class() -> Obj {
    let (object, rect) = ("@@:".to_string(), format!("{RECT}@:@"));
    let methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"textInputView", input_view as *const c_void, object.clone()),
        (c"hasText", has_text as *const c_void, format!("{BOOL}@:")),
        (c"insertText:", ignored as *const c_void, "v@:@".into()),
        (c"deleteBackward", nothing as *const c_void, "v@:".into()),
        (c"textInRange:", text_in as *const c_void, "@@:@".into()),
        (c"replaceRange:withText:", ignored_pair as *const c_void, "v@:@@".into()),
        (c"selectedTextRange", selected_range as *const c_void, object.clone()),
        (c"setSelectedTextRange:", set_selected_range as *const c_void, "v@:@".into()),
        (c"markedTextRange", none as *const c_void, object.clone()),
        (c"markedTextStyle", none as *const c_void, object.clone()),
        (c"setMarkedTextStyle:", ignored as *const c_void, "v@:@".into()),
        (c"setMarkedText:selectedRange:", ignored_marked as *const c_void, format!("v@:@{RANGE}")),
        (c"unmarkText", nothing as *const c_void, "v@:".into()),
        (c"beginningOfDocument", beginning as *const c_void, object.clone()),
        (c"endOfDocument", end as *const c_void, object.clone()),
        (c"textRangeFromPosition:toPosition:", range_between as *const c_void, "@@:@@".into()),
        (c"positionFromPosition:offset:", moved as *const c_void, "@@:@q".into()),
        (c"positionFromPosition:inDirection:offset:", moved_toward as *const c_void, "@@:@qq".into()),
        (c"comparePosition:toPosition:", compared as *const c_void, "q@:@@".into()),
        (c"offsetFromPosition:toPosition:", offset_between as *const c_void, "q@:@@".into()),
        (c"inputDelegate", none as *const c_void, object.clone()),
        (c"setInputDelegate:", ignored as *const c_void, "v@:@".into()),
        (c"tokenizer", tokenizer as *const c_void, object),
        (c"positionWithinRange:farthestInDirection:", farthest as *const c_void, "@@:@q".into()),
        (c"characterRangeByExtendingPosition:inDirection:", extended as *const c_void, "@@:@q".into()),
        (c"baseWritingDirectionForPosition:inDirection:", left_to_right as *const c_void, "q@:@q".into()),
        (c"setBaseWritingDirection:forRange:", ignored_direction as *const c_void, "v@:q@".into()),
        (c"firstRectForRange:", first_rect as *const c_void, rect.clone()),
        (c"caretRectForPosition:", caret_rect as *const c_void, rect),
        (c"selectionRectsForRange:", selection_rects as *const c_void, "@@:@".into()),
        (c"closestPositionToPoint:", closest as *const c_void, format!("@@:{POINT}")),
        (c"closestPositionToPoint:withinRange:", closest_within as *const c_void, format!("@@:{POINT}@")),
        (c"characterRangeAtPoint:", letter_at_point as *const c_void, format!("@@:{POINT}")),
    ];
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautTextReader", c"NSObject", &listed, &[c"UITextInput"])
}

/// A text element's reader, made the first time it is asked for; nil for another element.
extern "C" fn reader_of(element: Obj, _: Sel) -> Obj {
    if with_text(element, |_, _| ()).is_none() {
        return std::ptr::null_mut();
    }
    let known = READERS.with_borrow(|readers| readers.get(&(element as usize)).map(|reader| reader.object));
    known.unwrap_or_else(|| made_reader(element))
}

/// A new reader for an element, with its tokenizer, kept until the element goes.
fn made_reader(element: Obj) -> Obj {
    // SAFETY: an object of the reader class with room for its element beside it, written once;
    // then a tokenizer of the system's for it. Both are kept (retained) until `forget`.
    let (object, tokenizer) = unsafe {
        let object = class_createInstance(reader_class(), size_of::<Obj>());
        object_getIndexedIvars(object).cast::<Obj>().write(element);
        let tokenizer: Obj = send(send::<Obj>(class(c"UITextInputStringTokenizer"), c"alloc", ()), c"initWithTextInput:", (object,));
        (object, tokenizer)
    };
    READERS.with_borrow_mut(|readers| readers.insert(element as usize, Reader { object, tokenizer, selection: [0, 0] }));
    object
}

/// An element went: its reader goes too.
pub(crate) fn forget(element: Obj) {
    let Some(reader) = READERS.with_borrow_mut(|readers| readers.remove(&(element as usize))) else { return };
    // SAFETY: the two objects `made_reader` made, each released once.
    unsafe {
        send::<()>(reader.tokenizer, c"release", ());
        send::<()>(reader.object, c"release", ());
    }
}

/// A reader's element.
fn element_of(reader: Obj) -> Obj {
    // SAFETY: a reader of the library's class has its element beside it.
    unsafe { object_getIndexedIvars(reader).cast::<Obj>().read() }
}

/// Works with a reader's text node (and the window's nodes).
fn with_reader<T>(reader: Obj, work: impl FnOnce(&Told, &AccessNode) -> T) -> Option<T> {
    with_text(element_of(reader), work)
}

/// The reader's text, and its length in UTF-16 units.
fn text_of(reader: Obj) -> (String, usize) {
    let text = with_reader(reader, |_, node| node.text().to_string()).unwrap_or_default();
    let units = text.encode_utf16().count();
    (text, units)
}

extern "C" fn nothing(_: Obj, _: Sel) {}

extern "C" fn none(_: Obj, _: Sel) -> Obj {
    std::ptr::null_mut()
}

extern "C" fn ignored_pair(_: Obj, _: Sel, _: Obj, _: Obj) {}

extern "C" fn ignored_marked(_: Obj, _: Sel, _: Obj, _: NSRange) {}

/// The view the text is drawn in: rectangles and points are in its units.
extern "C" fn input_view(reader: Obj, _: Sel) -> Obj {
    with_reader(reader, |told, _| told.view).unwrap_or(std::ptr::null_mut())
}

extern "C" fn has_text(reader: Obj, _: Sel) -> Bool {
    Bool::from(text_of(reader).1 > 0)
}

extern "C" fn end(reader: Obj, _: Sel) -> Obj {
    position(text_of(reader).1)
}

extern "C" fn moved(reader: Obj, _: Sel, from: Obj, by: isize) -> Obj {
    moved_within(from, by, text_of(reader).1)
}

extern "C" fn moved_toward(reader: Obj, _: Sel, from: Obj, direction: isize, by: isize) -> Obj {
    moved_toward_within(from, direction, by, text_of(reader).1)
}

extern "C" fn extended(reader: Obj, _: Sel, from: Obj, direction: isize) -> Obj {
    extended_within(from, direction, text_of(reader).1)
}

extern "C" fn tokenizer(reader: Obj, _: Sel) -> Obj {
    READERS.with_borrow(|readers| readers.get(&(element_of(reader) as usize)).map(|reader| reader.tokenizer)).unwrap_or(std::ptr::null_mut())
}

/// The text between two UTF-16 units.
extern "C" fn text_in(reader: Obj, _: Sel, within: Obj) -> Obj {
    let units: Vec<u16> = text_of(reader).0.encode_utf16().collect();
    let [from, to] = ends_of(within).map(|end| end.min(units.len()));
    ns_string(&String::from_utf16_lossy(&units[from..to.max(from)]))
}

/// Whether a text is a text box's, whose selection is its own.
fn in_text_box(node: &AccessNode) -> bool {
    node.role == Role::TextBox
}

/// A text box's selection (none at its start while it lacks focus); the reader's own in other
/// text.
extern "C" fn selected_range(reader: Obj, _: Sel) -> Obj {
    let boxed = |_: &Told, node: &AccessNode| in_text_box(node).then(|| node.caret.unwrap_or_default().map(|letter| units_before(node.text(), letter) as usize));
    let kept = || READERS.with_borrow(|readers| readers.get(&(element_of(reader) as usize)).map_or([0, 0], |reader| reader.selection));
    let [anchor, cursor] = with_reader(reader, boxed).flatten().unwrap_or_else(kept);
    range(anchor, cursor)
}

/// A reader selects: in a text box, the box's own selection (it takes focus); elsewhere, kept
/// here.
extern "C" fn set_selected_range(reader: Obj, _: Sel, picked: Obj) {
    let ends = ends_of(picked);
    let element = element_of(reader);
    let boxed = |_: &Told, node: &AccessNode| in_text_box(node).then(|| ends.map(|unit| letter_at_unit(node.text(), unit as u32)));
    match with_reader(reader, boxed).flatten() {
        Some(letters) => _ = act(element, move |ui, id| ui.select_for_reader(id, letters)),
        None => READERS.with_borrow_mut(|readers| readers.get_mut(&(element as usize)).into_iter().for_each(|reader| reader.selection = ends)),
    }
}

/// The places of a UTF-16 range's lines, cut to what shows (an empty range: the cursor's
/// place), in the view.
fn range_places(reader: Obj, within: Obj) -> Vec<NSRect> {
    let places = |told: &Told, node: &AccessNode| {
        let letters = ends_of(within).map(|unit| letter_at_unit(node.text(), unit as u32));
        node.range_rects(letters).into_iter().map(|place| told.in_view(place)).collect()
    };
    with_reader(reader, places).unwrap_or_default()
}

extern "C" fn first_rect(reader: Obj, _: Sel, within: Obj) -> NSRect {
    range_places(reader, within).first().copied().unwrap_or_default()
}

/// The cursor's place at a position: one unit wide, as tall as the letter there.
extern "C" fn caret_rect(reader: Obj, _: Sel, at: Obj) -> NSRect {
    let place = |told: &Told, node: &AccessNode| {
        let letter = node.letter_rect(letter_at_unit(node.text(), offset_of(at) as u32));
        told.in_view(Rect { w: 1.0, ..letter })
    };
    with_reader(reader, place).unwrap_or_default()
}

/// A range's lines as selection rectangles, the first holding its start, the last its end.
extern "C" fn selection_rects(reader: Obj, _: Sel, within: Obj) -> Obj {
    let places = range_places(reader, within);
    let last = places.len().saturating_sub(1);
    let rects: Vec<Obj> = places.iter().enumerate().map(|(nth, place)| selection_rect(*place, nth == 0, nth == last)).collect();
    ns_array(&rects)
}

/// The position nearest a point in the view: the letter there, or the start above the text and
/// the end below it.
fn nearest(reader: Obj, at: NSPoint) -> usize {
    let found = |told: &Told, node: &AccessNode| {
        let point = crate::geom::pt((at.x - told.origin.0) as f32, (at.y - told.origin.1) as f32);
        let letter = node.letter_at(point).unwrap_or(if point.y < node.rect.y { 0 } else { node.text().chars().count() as u32 });
        units_before(node.text(), letter) as usize
    };
    with_reader(reader, found).unwrap_or(0)
}

extern "C" fn closest(reader: Obj, _: Sel, at: NSPoint) -> Obj {
    position(nearest(reader, at))
}

extern "C" fn closest_within(reader: Obj, _: Sel, at: NSPoint, within: Obj) -> Obj {
    let [from, to] = ends_of(within);
    position(nearest(reader, at).clamp(from, to))
}

/// The character at a point in the view; nil off the text.
extern "C" fn letter_at_point(reader: Obj, _: Sel, at: NSPoint) -> Obj {
    let found = |told: &Told, node: &AccessNode| {
        let letter = node.letter_at(crate::geom::pt((at.x - told.origin.0) as f32, (at.y - told.origin.1) as f32))?;
        Some([letter, letter + 1].map(|end| units_before(node.text(), end) as usize))
    };
    with_reader(reader, found).flatten().map_or(std::ptr::null_mut(), |[from, to]| range(from, to))
}

// ---- selection rectangles ----

/// What a selection rectangle keeps beside it: its place, and whether it holds the range's
/// start and end.
#[derive(Clone, Copy)]
struct SelectionPlace {
    place: NSRect,
    start: bool,
    end: bool,
}

/// The selection rectangle class, made once.
fn selection_rect_class() -> Obj {
    let yes_no = format!("{BOOL}@:");
    let methods: [(&CStr, *const c_void, &str); 5] = [
        (c"rect", rect_of as *const c_void, &format!("{RECT}@:")),
        (c"writingDirection", left_to_right_rect as *const c_void, "q@:"),
        (c"containsStart", holds_start as *const c_void, &yes_no),
        (c"containsEnd", holds_end as *const c_void, &yes_no),
        (c"isVertical", not_vertical as *const c_void, &yes_no),
    ];
    made_class(c"TautSelectionRect", c"UITextSelectionRect", &methods, &[])
}

/// A selection rectangle (autoreleased).
fn selection_rect(place: NSRect, start: bool, end: bool) -> Obj {
    // SAFETY: an object of the class with room for what it keeps beside it, written once.
    unsafe {
        let made = class_createInstance(selection_rect_class(), size_of::<SelectionPlace>());
        object_getIndexedIvars(made).cast::<SelectionPlace>().write(SelectionPlace { place, start, end });
        send(made, c"autorelease", ())
    }
}

/// What a selection rectangle keeps.
fn place_of(rect: Obj) -> SelectionPlace {
    // SAFETY: a rectangle of the library's class keeps this beside it.
    unsafe { object_getIndexedIvars(rect).cast::<SelectionPlace>().read() }
}

extern "C" fn rect_of(rect: Obj, _: Sel) -> NSRect {
    place_of(rect).place
}

extern "C" fn holds_start(rect: Obj, _: Sel) -> Bool {
    Bool::from(place_of(rect).start)
}

extern "C" fn holds_end(rect: Obj, _: Sel) -> Bool {
    Bool::from(place_of(rect).end)
}

extern "C" fn not_vertical(_: Obj, _: Sel) -> Bool {
    0
}

/// `NSWritingDirectionLeftToRight`.
extern "C" fn left_to_right_rect(_: Obj, _: Sel) -> isize {
    0
}
