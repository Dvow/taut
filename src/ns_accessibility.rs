//! Screen readers on macOS: what a window tells (its access nodes, `accessibility`) as
//! NSAccessibility elements, which VoiceOver and other assistive programs read through the
//! system. Nothing is built until one asks: the first time an assistive program asks a view
//! for its content, every window starts telling (`Ui::set_screen_reader`), and the next run
//! builds the nodes.
//!
//! Each node is an element of the library's class (`TautElement`, made from
//! `NSAccessibilityElement`), kept from run to run while its node's id is told, so a reader
//! keeps its place. An element answers from its node as the last run left it: role, name,
//! value, state, its place on the screen, its parent and children. What a reader does
//! (press, step, focus, type, scroll into view) reaches the window's interface as input does,
//! in the loop's next turn.
use crate::accessibility::{AccessNode, Role};
use crate::id::Id;
use crate::macos::{Heard, heard};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_void};

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    fn NSAccessibilityPostNotification(element: Obj, name: Obj);
    fn NSAccessibilityPostNotificationWithUserInfo(element: Obj, name: Obj, info: Obj);
    static NSAccessibilityLayoutChangedNotification: Obj;
    static NSAccessibilityFocusedUIElementChangedNotification: Obj;
    static NSAccessibilityValueChangedNotification: Obj;
    static NSAccessibilityUIElementDestroyedNotification: Obj;
    static NSAccessibilityAnnouncementRequestedNotification: Obj;
    static NSAccessibilityAnnouncementKey: Obj;
    static NSAccessibilityMarkedMisspelledTextAttribute: Obj;
}
#[link(name = "objc")]
unsafe extern "C" {
    fn sel_getName(name: Sel) -> *const std::ffi::c_char;
}

/// What one window tells: its view, its nodes as the last run built them (and which those
/// are), and an element for each.
struct Told {
    view: Obj,
    version: u64,
    nodes: Vec<AccessNode>,
    elements: Vec<Obj>,
    /// The elements kept, by their node's id.
    by_id: HashMap<Id, Obj>,
}

thread_local! {
    static TOLD: RefCell<Vec<Told>> = const { RefCell::new(Vec::new()) };
    /// Each element's window and node, by the element.
    static WHERE: RefCell<HashMap<usize, (Obj, usize)>> = RefCell::new(HashMap::new());
    /// An assistive program has asked something: the windows tell from now on.
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// The view's methods an assistive program asks of it: its elements, the focused one, the one
/// at a point.
pub(crate) fn view_methods() -> Vec<(&'static CStr, *const c_void, String)> {
    vec![
        (c"accessibilityChildren", view_children as *const c_void, "@@:".into()),
        (c"accessibilityFocusedUIElement", view_focused as *const c_void, "@@:".into()),
        (c"accessibilityHitTest:", view_hit as *const c_void, format!("@@:{POINT}")),
        (c"isAccessibilityElement", no as *const c_void, format!("{BOOL}@:")),
    ]
}

/// The element class, made once.
fn element_class() -> Obj {
    let yes_no = format!("{BOOL}@:");
    let object = "@@:".to_string();
    let mut methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"accessibilityRole", role as *const c_void, object.clone()),
        (c"accessibilitySubrole", subrole as *const c_void, object.clone()),
        (c"accessibilityLabel", label as *const c_void, object.clone()),
        (c"accessibilityTitle", label as *const c_void, object.clone()),
        (c"accessibilityValue", value as *const c_void, object.clone()),
        (c"accessibilityMinValue", least as *const c_void, object.clone()),
        (c"accessibilityMaxValue", most as *const c_void, object.clone()),
        (c"accessibilityParent", parent as *const c_void, object.clone()),
        (c"accessibilityChildren", children as *const c_void, object.clone()),
        (c"accessibilityWindow", window as *const c_void, object.clone()),
        (c"accessibilityTopLevelUIElement", window as *const c_void, object.clone()),
        (c"accessibilityFrame", frame as *const c_void, format!("{RECT}@:")),
        (c"isAccessibilityElement", yes as *const c_void, yes_no.clone()),
        (c"isAccessibilityEnabled", enabled as *const c_void, yes_no.clone()),
        (c"isAccessibilityFocused", focused as *const c_void, yes_no.clone()),
        (c"isAccessibilitySelected", selected as *const c_void, yes_no.clone()),
        (c"isAccessibilityExpanded", expanded as *const c_void, yes_no.clone()),
        (c"accessibilityPerformPress", press as *const c_void, yes_no.clone()),
        (c"accessibilityPerformIncrement", increment as *const c_void, yes_no.clone()),
        (c"accessibilityPerformDecrement", decrement as *const c_void, yes_no.clone()),
        (c"accessibilityPerformScrollToVisible", scroll_to as *const c_void, yes_no.clone()),
        (c"setAccessibilityFocused:", set_focused as *const c_void, format!("v@:{BOOL}")),
        (c"setAccessibilityValue:", set_value as *const c_void, "v@:@".into()),
    ];
    methods.extend(structure_methods());
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    made_class(c"TautElement", c"NSAccessibilityElement", &listed, &[])
}

extern "C" fn yes(_: Obj, _: Sel) -> Bool {
    1
}

extern "C" fn no(_: Obj, _: Sel) -> Bool {
    0
}

/// Notes that an assistive program asks: from the next turn every window tells.
fn asked(view: Obj) {
    if !LISTENING.replace(true) {
        heard(Heard::Other(view, Box::new(|_| true)));
    }
}

/// Whether windows tell now (see `asked`).
pub(crate) fn listening() -> bool {
    LISTENING.get()
}

/// Works with an element's node.
fn with_node<T>(element: Obj, work: impl FnOnce(&Told, usize) -> T) -> Option<T> {
    let (view, at) = WHERE.with_borrow(|places| places.get(&(element as usize)).copied())?;
    TOLD.with_borrow(|told| told.iter().find(|told| told.view == view).filter(|told| at < told.nodes.len()).map(|told| work(told, at)))
}

/// The elements of these nodes, as an array (autoreleased).
fn array_of(told: &Told, nodes: impl Iterator<Item = usize>) -> Obj {
    let elements: Vec<Obj> = nodes.map(|at| told.elements[at]).collect();
    ns_array(&elements)
}

extern "C" fn view_children(view: Obj, _: Sel) -> Obj {
    asked(view);
    let top = TOLD.with_borrow(|told| told.iter().find(|told| told.view == view).map(|told| array_of(told, (0..told.nodes.len()).filter(|at| told.nodes[*at].parent.is_none()))));
    top.unwrap_or_else(|| ns_array(&[]))
}

extern "C" fn view_focused(view: Obj, _: Sel) -> Obj {
    asked(view);
    let found = TOLD.with_borrow(|told| told.iter().find(|told| told.view == view).and_then(|told| told.nodes.iter().rposition(|node| node.focused).map(|at| told.elements[at])));
    found.unwrap_or(view)
}

/// A point on the screen, in a view's units.
fn in_view(view: Obj, at: NSPoint) -> crate::geom::Point {
    // SAFETY: the screen point turned into the window's, then the (flipped) view's.
    let inside: NSPoint = unsafe {
        let window: Obj = send(view, c"window", ());
        let on_window = send_rect(window, c"convertRectFromScreen:", (NSRect { origin: at, size: NSSize::default() },)).origin;
        send(view, c"convertPoint:fromView:", (on_window, std::ptr::null_mut::<c_void>()))
    };
    crate::geom::pt(inside.x as f32, inside.y as f32)
}

/// The element at a point on the screen: the last one there (content comes after its group).
extern "C" fn view_hit(view: Obj, _: Sel, at: NSPoint) -> Obj {
    asked(view);
    let point = in_view(view, at);
    let found =
        TOLD.with_borrow(|told| told.iter().find(|told| told.view == view).and_then(|told| told.nodes.iter().rposition(|node| node.visible && node.seen.contains(point)).map(|at| told.elements[at])));
    found.unwrap_or(view)
}

/// A role as AppKit names it, and the subrole where one says more.
fn roles(role: Role) -> (&'static str, Option<&'static str>) {
    match role {
        Role::Text | Role::Paragraph | Role::Code => ("AXStaticText", None),
        Role::Button => ("AXButton", None),
        Role::CheckBox => ("AXCheckBox", None),
        Role::Switch => ("AXCheckBox", Some("AXSwitch")),
        Role::RadioButton => ("AXRadioButton", None),
        Role::Tab => ("AXRadioButton", Some("AXTabButton")),
        Role::Row | Role::TableRow => ("AXRow", None),
        Role::TreeItem => ("AXRow", Some("AXOutlineRow")),
        Role::Slider => ("AXSlider", None),
        Role::SpinButton => ("AXIncrementor", None),
        Role::TextBox => ("AXTextField", None),
        Role::PasswordBox => ("AXTextField", Some("AXSecureTextField")),
        Role::Fold => ("AXDisclosureTriangle", None),
        Role::ComboBox => ("AXPopUpButton", None),
        Role::MenuItem | Role::MenuItemRadio | Role::Submenu => ("AXMenuItem", None),
        Role::ColumnHeader | Role::Cell => ("AXCell", None),
        Role::Progress => ("AXProgressIndicator", None),
        Role::Dialog => ("AXGroup", Some("AXDialog")),
        Role::Menu => ("AXMenu", None),
        Role::MenuBar => ("AXMenuBar", None),
        Role::Tabs => ("AXTabGroup", None),
        Role::Link => ("AXLink", None),
        Role::Table => ("AXTable", None),
        Role::ToolBar => ("AXToolbar", None),
        Role::Tree => ("AXOutline", None),
        Role::Picture => ("AXImage", None),
        Role::Heading => ("AXHeading", None),
        Role::List => ("AXList", None),
        _ => ("AXGroup", None),
    }
}

extern "C" fn role(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| ns_string(roles(told.nodes[at].role).0)).unwrap_or(std::ptr::null_mut())
}

extern "C" fn subrole(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| roles(told.nodes[at].role).1.map_or(std::ptr::null_mut(), ns_string)).unwrap_or(std::ptr::null_mut())
}

extern "C" fn label(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| ns_string(&told.nodes[at].name)).unwrap_or(std::ptr::null_mut())
}

/// A number, as an `NSNumber` (autoreleased).
fn number(value: f64) -> Obj {
    // SAFETY: a plain class method.
    unsafe { send(class(c"NSNumber"), c"numberWithDouble:", (value,)) }
}

/// A node's value: on or off for what can be, a slider's amount, a text box's text, a heading's
/// level.
extern "C" fn value(element: Obj, _: Sel) -> Obj {
    let of = |node: &AccessNode| match (node.role, node.value_range, node.on) {
        (Role::CheckBox | Role::Switch | Role::RadioButton | Role::Tab, _, Some(on)) => number(f64::from(u8::from(on))),
        (_, Some([amount, ..]), _) => number(f64::from(amount)),
        (Role::Heading, ..) => number(f64::from(node.level)),
        _ if !node.value.is_empty() => ns_string(&node.value),
        _ => std::ptr::null_mut(),
    };
    with_node(element, |told, at| of(&told.nodes[at])).unwrap_or(std::ptr::null_mut())
}

extern "C" fn least(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| told.nodes[at].value_range.map_or(std::ptr::null_mut(), |[_, low, _]| number(f64::from(low)))).unwrap_or(std::ptr::null_mut())
}

extern "C" fn most(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| told.nodes[at].value_range.map_or(std::ptr::null_mut(), |[.., high]| number(f64::from(high)))).unwrap_or(std::ptr::null_mut())
}

extern "C" fn parent(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| told.nodes[at].parent.map_or(told.view, |up| told.elements[up])).unwrap_or(std::ptr::null_mut())
}

extern "C" fn children(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| array_of(told, (at + 1..told.nodes.len()).filter(|under| told.nodes[*under].parent == Some(at)))).unwrap_or_else(|| ns_array(&[]))
}

extern "C" fn window(element: Obj, _: Sel) -> Obj {
    // SAFETY: a view answers its window.
    with_node(element, |told, _| unsafe { send(told.view, c"window", ()) }).unwrap_or(std::ptr::null_mut())
}

/// Where a node is on the screen, in AppKit's screen points.
extern "C" fn frame(element: Obj, _: Sel) -> NSRect {
    let place = with_node(element, |told, at| (told.view, crate::accessibility::view(&told.nodes, at)));
    place.map_or(NSRect::default(), |(view, rect)| on_screen(view, rect))
}

/// A rectangle of a view, in AppKit's screen points.
fn on_screen(view: Obj, rect: crate::geom::Rect) -> NSRect {
    let rect = NSRect::new(f64::from(rect.x), f64::from(rect.y), f64::from(rect.w), f64::from(rect.h));
    // SAFETY: the view's rectangle turned into its window's, then the screen's.
    unsafe {
        let window: Obj = send(view, c"window", ());
        let in_window = send_rect(view, c"convertRect:toView:", (rect, std::ptr::null_mut::<c_void>()));
        send_rect(window, c"convertRectToScreen:", (in_window,))
    }
}

/// A state of an element's node, as a `BOOL`.
fn state(element: Obj, of: impl FnOnce(&AccessNode) -> bool) -> Bool {
    with_node(element, |told, at| of(&told.nodes[at])).unwrap_or(false) as Bool
}

extern "C" fn enabled(element: Obj, _: Sel) -> Bool {
    state(element, |node| node.enabled)
}

extern "C" fn focused(element: Obj, _: Sel) -> Bool {
    state(element, |node| node.focused)
}

extern "C" fn selected(element: Obj, _: Sel) -> Bool {
    state(element, |node| matches!(node.role, Role::Tab | Role::Row | Role::TableRow | Role::TreeItem) && node.on == Some(true))
}

extern "C" fn expanded(element: Obj, _: Sel) -> Bool {
    state(element, |node| node.expanded.or(matches!(node.role, Role::Fold | Role::ComboBox).then_some(node.on == Some(true))).unwrap_or(false))
}

/// Has the element's window do something to its node in the next turn: what a reader asked.
fn act(element: Obj, deed: impl FnOnce(&mut Ui, Id) + 'static) -> Bool {
    let Some((view, id)) = with_node(element, |told, at| (told.view, told.nodes[at].id)) else { return 0 };
    heard(Heard::Other(
        view,
        Box::new(move |ui: &mut Ui| {
            deed(ui, id);
            true
        }),
    ));
    1
}

extern "C" fn press(element: Obj, _: Sel) -> Bool {
    act(element, Ui::press_for_reader)
}

extern "C" fn increment(element: Obj, _: Sel) -> Bool {
    act(element, |ui, id| ui.step_for_reader(id, true))
}

extern "C" fn decrement(element: Obj, _: Sel) -> Bool {
    act(element, |ui, id| ui.step_for_reader(id, false))
}

extern "C" fn scroll_to(element: Obj, _: Sel) -> Bool {
    act(element, Ui::show_for_reader)
}

extern "C" fn set_focused(element: Obj, _: Sel, on: Bool) {
    if on != 0 {
        act(element, |ui, id| ui.set_focus(id));
    }
}

/// A reader sets a value: a text box's text, or a slider's amount.
extern "C" fn set_value(element: Obj, _: Sel, value: Obj) {
    let Some(role) = with_node(element, |told, at| told.nodes[at].role) else { return };
    if role == Role::TextBox {
        let text = rust_string(value).unwrap_or_default();
        act(element, move |ui, id| ui.type_for_reader(id, &text));
    } else if role == Role::Slider {
        // SAFETY: a number answers its value.
        let amount = unsafe { send::<f64>(value, c"doubleValue", ()) } as f32;
        act(element, move |ui, id| ui.set_for_reader(id, amount));
    }
}

/// After a run: a window's elements follow its nodes, where they changed. An element is made
/// for each new node and let go of with its node; the system hears that the layout changed,
/// where focus went, and what the app announced.
pub(crate) fn keep(view: Obj, ui: &Ui) {
    let (version, moved_from) = (ui.access_version(), ui.access_moved_from());
    let nodes = ui.access_nodes();
    let focus_before = TOLD.with_borrow(|told| told.iter().find(|told| told.view == view).and_then(|told| told.nodes.iter().find(|node| node.focused).map(|node| node.id)));
    let changed = TOLD.with_borrow_mut(|told| {
        let at = told.iter().position(|told| told.view == view).unwrap_or_else(|| {
            told.push(Told { view, version: 0, nodes: Vec::new(), elements: Vec::new(), by_id: HashMap::new() });
            told.len() - 1
        });
        let told = &mut told[at];
        let was = std::mem::replace(&mut told.version, version);
        // (Moved only, as by a scroll: where its elements lie changed, and no more.)
        if moved_from == Some(was) && told.nodes.len() == nodes.len() {
            crate::accessibility::follow_move(&mut told.nodes, nodes);
            return Some(None);
        }
        (was != version).then(|| Some(renew(told, nodes)))
    });
    // SAFETY: the system is told of elements of the library's, and of its view.
    unsafe {
        if changed == Some(None) {
            NSAccessibilityPostNotification(view, NSAccessibilityLayoutChangedNotification);
        }
        if let Some(Some(gone)) = changed {
            gone.into_iter().for_each(|element| NSAccessibilityPostNotification(element, NSAccessibilityUIElementDestroyedNotification));
            NSAccessibilityPostNotification(view, NSAccessibilityLayoutChangedNotification);
            let focus = nodes.iter().find(|node| node.focused).map(|node| node.id);
            if focus != focus_before {
                NSAccessibilityPostNotification(view, NSAccessibilityFocusedUIElementChangedNotification);
            }
            NSAccessibilityPostNotification(view, NSAccessibilityValueChangedNotification);
        }
        ui.announced().iter().for_each(|text| announce(text));
    }
}

/// Has the system say something the app announced.
fn announce(text: &str) {
    // SAFETY: a dictionary of one string, handed to the system.
    unsafe {
        let info: Obj = send(class(c"NSDictionary"), c"dictionaryWithObject:forKey:", (ns_string(text), NSAccessibilityAnnouncementKey));
        let app: Obj = send(class(c"NSApplication"), c"sharedApplication", ());
        NSAccessibilityPostNotificationWithUserInfo(app, NSAccessibilityAnnouncementRequestedNotification, info);
    }
}

/// A window's elements made to match its nodes: kept by id, made for new ones. Gives back those
/// let go of (released once the system heard).
fn renew(told: &mut Told, nodes: &[AccessNode]) -> Vec<Obj> {
    let class = element_class();
    let mut by_id = std::mem::take(&mut told.by_id);
    let mut kept = HashMap::with_capacity(nodes.len());
    let elements: Vec<Obj> = nodes.iter().map(|node| *kept.entry(node.id).or_insert_with(|| by_id.remove(&node.id).unwrap_or_else(|| made_element(class)))).collect();
    WHERE.with_borrow_mut(|places| {
        places.retain(|_, (view, _)| *view != told.view);
        places.extend(elements.iter().enumerate().map(|(at, element)| (*element as usize, (told.view, at))));
    });
    (told.nodes, told.elements, told.by_id) = (nodes.to_vec(), elements, kept);
    let gone: Vec<Obj> = by_id.into_values().collect();
    // SAFETY: elements this made, each released once, after the system hears they are gone.
    gone.iter().for_each(|element| unsafe { send::<()>(*element, c"autorelease", ()) });
    gone
}

/// A new element of the library's class (retained: let go of in `renew` or `forget`).
fn made_element(class: Obj) -> Obj {
    // SAFETY: an element made with `init`.
    unsafe { send(send::<Obj>(class, c"alloc", ()), c"init", ()) }
}

/// A window closed: its elements are let go of.
pub(crate) fn forget(view: Obj) {
    let Some(told) = TOLD.with_borrow_mut(|told| told.iter().position(|told| told.view == view).map(|at| told.remove(at))) else { return };
    WHERE.with_borrow_mut(|places| places.retain(|_, (of, _)| *of != view));
    // SAFETY: elements this made, each released once.
    told.by_id.into_values().for_each(|element| unsafe {
        NSAccessibilityPostNotification(element, NSAccessibilityUIElementDestroyedNotification);
        send::<()>(element, c"release", ());
    });
}

// ---- text, tables and trees ----

/// What an assistive program asks of text boxes (lengths, the selection, lines, pieces, by
/// UTF-16 units as AppKit counts), of tables and trees (rows, columns, cells, headings), and of
/// their rows and items (place, level, folded or not).
fn structure_methods() -> Vec<(&'static CStr, *const c_void, String)> {
    let (count, object, yes_no) = ("q@:".to_string(), "@@:".to_string(), format!("{BOOL}@:"));
    vec![
        (c"accessibilityNumberOfCharacters", character_count as *const c_void, count.clone()),
        (c"accessibilitySelectedTextRange", selected_range as *const c_void, format!("{RANGE}@:")),
        (c"setAccessibilitySelectedTextRange:", select_range as *const c_void, format!("v@:{RANGE}")),
        (c"accessibilitySelectedText", selected_text as *const c_void, object.clone()),
        (c"accessibilityVisibleCharacterRange", whole_range as *const c_void, format!("{RANGE}@:")),
        (c"accessibilityInsertionPointLineNumber", cursor_line as *const c_void, count.clone()),
        (c"accessibilityStringForRange:", string_for as *const c_void, format!("@@:{RANGE}")),
        (c"accessibilityAttributedStringForRange:", attributed_for as *const c_void, format!("@@:{RANGE}")),
        (c"accessibilityLineForIndex:", line_for as *const c_void, "q@:q".into()),
        (c"accessibilityRangeForLine:", range_for_line as *const c_void, format!("{RANGE}@:q")),
        (c"accessibilityRangeForIndex:", range_for_index as *const c_void, format!("{RANGE}@:q")),
        (c"accessibilityFrameForRange:", frame_for_range as *const c_void, format!("{RECT}@:{RANGE}")),
        (c"accessibilityRangeForPosition:", range_for_position as *const c_void, format!("{RANGE}@:{POINT}")),
        (c"accessibilityRowCount", row_count as *const c_void, count.clone()),
        (c"accessibilityColumnCount", column_count as *const c_void, count.clone()),
        (c"accessibilityRows", rows as *const c_void, object.clone()),
        (c"accessibilityVisibleRows", rows as *const c_void, object.clone()),
        (c"accessibilitySelectedRows", selected_rows as *const c_void, object.clone()),
        (c"accessibilityColumnHeaderUIElements", headings as *const c_void, object.clone()),
        (c"accessibilityCellForColumn:row:", cell_for as *const c_void, "@@:qq".into()),
        (c"accessibilityIndex", index as *const c_void, count.clone()),
        (c"accessibilityRowIndexRange", row_range as *const c_void, format!("{RANGE}@:")),
        (c"accessibilityColumnIndexRange", column_range as *const c_void, format!("{RANGE}@:")),
        (c"accessibilitySortDirection", sort_direction as *const c_void, count.clone()),
        (c"accessibilityDisclosureLevel", disclosure_level as *const c_void, count),
        (c"isAccessibilityDisclosed", disclosed as *const c_void, yes_no),
        (c"setAccessibilityDisclosed:", set_disclosed as *const c_void, format!("v@:{BOOL}")),
        (c"accessibilityDisclosedByRow", disclosed_by as *const c_void, object.clone()),
        (c"accessibilityDisclosedRows", disclosed_rows as *const c_void, object),
        (c"setAccessibilitySelected:", set_selected as *const c_void, format!("v@:{BOOL}")),
        (c"isAccessibilitySelectorAllowed:", allowed as *const c_void, format!("{BOOL}@::")),
    ]
}

/// Which roles each of those answers for: the rest are not offered to readers.
fn roles_for(name: &CStr) -> Option<&'static [Role]> {
    const TEXT: &[Role] = &[Role::TextBox, Role::PasswordBox, Role::Text, Role::Heading, Role::Paragraph, Role::Code];
    const BOXES: &[Role] = &[Role::TextBox, Role::PasswordBox];
    let found: &[Role] = match name.to_bytes() {
        b"accessibilityRowCount" | b"accessibilityColumnCount" | b"accessibilityColumnHeaderUIElements" | b"accessibilityCellForColumn:row:" => &[Role::Table],
        b"accessibilityRows" | b"accessibilityVisibleRows" | b"accessibilitySelectedRows" => &[Role::Table, Role::Tree],
        b"accessibilityIndex" => &[Role::TableRow, Role::TreeItem],
        b"accessibilityRowIndexRange" => &[Role::Cell],
        b"accessibilityColumnIndexRange" => &[Role::Cell, Role::ColumnHeader],
        b"accessibilitySortDirection" => &[Role::ColumnHeader],
        b"accessibilityDisclosureLevel" | b"isAccessibilityDisclosed" | b"setAccessibilityDisclosed:" | b"accessibilityDisclosedByRow" | b"accessibilityDisclosedRows" => &[Role::TreeItem],
        b"accessibilityStringForRange:" | b"accessibilityAttributedStringForRange:" | b"accessibilityFrameForRange:" | b"accessibilityRangeForPosition:" => TEXT,
        b"accessibilityLineForIndex:" | b"accessibilityRangeForLine:" | b"accessibilityRangeForIndex:" => TEXT,
        b"accessibilityNumberOfCharacters" | b"accessibilityVisibleCharacterRange" => TEXT,
        b"accessibilitySelectedTextRange" | b"setAccessibilitySelectedTextRange:" | b"accessibilitySelectedText" | b"accessibilityInsertionPointLineNumber" => BOXES,
        _ => return None,
    };
    Some(found)
}

/// Whether the element answers a method: those above only for their roles.
extern "C" fn allowed(element: Obj, _: Sel, asked: Sel) -> Bool {
    // SAFETY: a method name is a string the runtime keeps.
    let name = unsafe { CStr::from_ptr(sel_getName(asked)) };
    let Some(roles) = roles_for(name) else { return 1 };
    state(element, |node| roles.contains(&node.role))
}

/// UTF-16 units in the first `chars` characters of a text.
fn units(text: &str, chars: usize) -> usize {
    text.chars().take(chars).map(char::len_utf16).sum()
}

/// The character at (or holding) a UTF-16 unit of a text, from its start.
fn chars_to(text: &str, unit: usize) -> u32 {
    let (mut seen, mut count) = (0, 0);
    for letter in text.chars() {
        seen += letter.len_utf16();
        if seen > unit {
            break;
        }
        count += 1;
    }
    count
}

/// The text of a text box or of plain text, if the element is one.
fn text_of_box(element: Obj) -> Option<String> {
    with_node(element, |told, at| told.nodes[at].reads_text().then(|| told.nodes[at].text().to_string())).flatten()
}

/// The part of a text a UTF-16 range covers, cut to its end.
fn piece(text: &str, range: NSRange) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let from = range.location.min(units.len());
    String::from_utf16_lossy(&units[from..(from + range.length).min(units.len())])
}

extern "C" fn character_count(element: Obj, _: Sel) -> isize {
    text_of_box(element).map_or(0, |text| text.encode_utf16().count() as isize)
}

/// The selection, from its start: the cursor alone where nothing is selected.
fn selection(element: Obj) -> NSRange {
    let found = with_node(element, |told, at| told.nodes[at].caret.map(|[anchor, cursor]| (told.nodes[at].value.clone(), anchor.min(cursor), anchor.max(cursor))));
    let Some(Some((text, from, to))) = found else { return NSRange::default() };
    let start = units(&text, from as usize);
    NSRange { location: start, length: units(&text, to as usize) - start }
}

extern "C" fn selected_range(element: Obj, _: Sel) -> NSRange {
    selection(element)
}

extern "C" fn select_range(element: Obj, _: Sel, range: NSRange) {
    let Some(text) = text_of_box(element) else { return };
    let ends = [chars_to(&text, range.location), chars_to(&text, range.location + range.length)];
    act(element, move |ui, id| ui.select_for_reader(id, ends));
}

extern "C" fn selected_text(element: Obj, _: Sel) -> Obj {
    text_of_box(element).map_or(std::ptr::null_mut(), |text| ns_string(&piece(&text, selection(element))))
}

/// The characters that show.
extern "C" fn whole_range(element: Obj, _: Sel) -> NSRange {
    let shown = with_node(element, |told, at| (told.nodes[at].text().to_string(), told.nodes[at].shown_letters()));
    let Some((text, [from, to])) = shown else { return NSRange::default() };
    let start = units(&text, from as usize);
    NSRange { location: start, length: units(&text, to as usize) - start }
}

/// The line (from 0, as wrapped) a UTF-16 unit of an element's text is on.
fn line_of(element: Obj, unit: usize) -> isize {
    with_node(element, |told, at| told.nodes[at].line_of(chars_to(told.nodes[at].text(), unit)) as isize).unwrap_or(0)
}

extern "C" fn cursor_line(element: Obj, _: Sel) -> isize {
    let Some(text) = text_of_box(element) else { return 0 };
    let cursor = with_node(element, |told, at| told.nodes[at].caret.map_or(0, |[_, cursor]| cursor)).unwrap_or(0);
    line_of(element, units(&text, cursor as usize))
}

extern "C" fn string_for(element: Obj, _: Sel, range: NSRange) -> Obj {
    text_of_box(element).map_or(std::ptr::null_mut(), |text| ns_string(&piece(&text, range)))
}

/// A piece of the text as an attributed string, its misspelled words marked as such.
extern "C" fn attributed_for(element: Obj, _: Sel, range: NSRange) -> Obj {
    let Some(text) = text_of_box(element) else { return std::ptr::null_mut() };
    let wrong = with_node(element, |told, at| told.nodes[at].misspelled.clone()).unwrap_or_default();
    // SAFETY: an attributed string made from the piece and marked over ranges inside it.
    unsafe {
        let made: Obj = send(class(c"NSMutableAttributedString"), c"alloc", ());
        let made: Obj = send(made, c"initWithString:", (ns_string(&piece(&text, range)),));
        let yes: Obj = send(class(c"NSNumber"), c"numberWithBool:", (1 as Bool,));
        for [from, to] in wrong {
            let (from, to) = (units(&text, from as usize), units(&text, to as usize));
            let (start, end) = (from.max(range.location), to.min(range.location + range.length));
            if start < end {
                let within = NSRange { location: start - range.location, length: end - start };
                send::<()>(made, c"addAttribute:value:range:", (NSAccessibilityMarkedMisspelledTextAttribute, yes, within));
            }
        }
        send(made, c"autorelease", ())
    }
}

extern "C" fn line_for(element: Obj, _: Sel, unit: isize) -> isize {
    line_of(element, unit.max(0) as usize)
}

/// A line's range (from 0, as wrapped), with its line break. Past the last, empty at the end.
extern "C" fn range_for_line(element: Obj, _: Sel, line: isize) -> NSRange {
    let found = with_node(element, |told, at| (told.nodes[at].text().to_string(), told.nodes[at].line_range(line.max(0) as usize)));
    let Some((text, range)) = found else { return NSRange::default() };
    let [from, to] = range.unwrap_or([text.chars().count() as u32; 2]).map(|letter| units(&text, letter as usize));
    NSRange { location: from, length: to - from }
}

/// The character holding a UTF-16 unit: one or two units.
extern "C" fn range_for_index(element: Obj, _: Sel, unit: isize) -> NSRange {
    let Some(text) = text_of_box(element) else { return NSRange::default() };
    let nth = chars_to(&text, unit.max(0) as usize) as usize;
    let length = text.chars().nth(nth).map_or(0, char::len_utf16);
    NSRange { location: units(&text, nth), length }
}

/// Where a piece of the text is drawn: the box around its letters.
extern "C" fn frame_for_range(element: Obj, _: Sel, range: NSRange) -> NSRect {
    let place = |told: &Told, at: usize| {
        let (node, text) = (&told.nodes[at], told.nodes[at].text());
        let [from, to] = [chars_to(text, range.location), chars_to(text, range.location + range.length)];
        let letters = (from..to.max(from + 1)).map(|letter| node.letter_rect(letter)).reduce(|one, other| one.union(other));
        (told.view, letters.unwrap_or(node.rect))
    };
    with_node(element, place).map_or(NSRect::default(), |(view, rect)| on_screen(view, rect))
}

/// The character at a point on the screen: empty where none is.
extern "C" fn range_for_position(element: Obj, _: Sel, at: NSPoint) -> NSRange {
    let letter = |told: &Told, at_node: usize| {
        let node = &told.nodes[at_node];
        let letter = node.letter_at(in_view(told.view, at))?;
        let (text, length) = (node.text(), node.text().chars().nth(letter as usize).map_or(0, char::len_utf16));
        Some(NSRange { location: units(text, letter as usize), length })
    };
    with_node(element, letter).flatten().unwrap_or_default()
}

/// A table's or tree's rows (its items), by node index.
fn rows_in(told: &Told, at: usize) -> impl Iterator<Item = usize> + '_ {
    let row = if told.nodes[at].role == Role::Tree { Role::TreeItem } else { Role::TableRow };
    (at + 1..told.nodes.len()).filter(move |under| told.nodes[*under].parent == Some(at) && told.nodes[*under].role == row)
}

extern "C" fn row_count(element: Obj, _: Sel) -> isize {
    with_node(element, |told, at| told.nodes[at].grid.map_or(0, |[rows, _]| rows as isize)).unwrap_or(0)
}

extern "C" fn column_count(element: Obj, _: Sel) -> isize {
    with_node(element, |told, at| told.nodes[at].grid.map_or(0, |[_, columns]| columns as isize)).unwrap_or(0)
}

extern "C" fn rows(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| array_of(told, rows_in(told, at))).unwrap_or(std::ptr::null_mut())
}

extern "C" fn selected_rows(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at| array_of(told, rows_in(told, at).filter(|row| told.nodes[*row].on == Some(true)))).unwrap_or(std::ptr::null_mut())
}

extern "C" fn headings(element: Obj, _: Sel) -> Obj {
    let heads = |told: &Told, at: usize| array_of(told, (at + 1..told.nodes.len()).filter(|under| told.nodes[*under].parent == Some(at) && told.nodes[*under].role == Role::ColumnHeader));
    with_node(element, heads).unwrap_or(std::ptr::null_mut())
}

/// A table's cell. One of a long table's row not told (it tells rows in view) is none, and the
/// row is scrolled into view, its cell there once the window has run.
extern "C" fn cell_for(element: Obj, _: Sel, column: isize, row: isize) -> Obj {
    let place = [row.max(0) as u32, column.max(0) as u32];
    let found = with_node(element, |told, at| (crate::accessibility::cell_at(&told.nodes, at, place).map(|cell| told.elements[cell]), told.nodes[at].grid.unwrap_or_default()));
    let Some((cell, [rows, columns])) = found else { return std::ptr::null_mut() };
    if cell.is_none() && place[0] < rows && place[1] < columns {
        act(element, move |ui, table| ui.row_for_reader(table, place[0], None));
    }
    cell.unwrap_or(std::ptr::null_mut())
}

/// A table row's index among all rows; a tree item's among its tree's items told.
extern "C" fn index(element: Obj, _: Sel) -> isize {
    let of = |told: &Told, at: usize| match told.nodes[at].role {
        Role::TableRow => told.nodes[at].position.map_or(0, |[nth, _]| nth as isize - 1),
        _ => told.nodes[at].parent.map_or(0, |tree| rows_in(told, tree).position(|item| item == at).unwrap_or(0) as isize),
    };
    with_node(element, of).unwrap_or(0)
}

extern "C" fn row_range(element: Obj, _: Sel) -> NSRange {
    with_node(element, |told, at| told.nodes[at].grid.map_or(NSRange::default(), |[row, _]| NSRange { location: row as usize, length: 1 })).unwrap_or_default()
}

/// A cell's column; a column heading's, from its place among the headings.
extern "C" fn column_range(element: Obj, _: Sel) -> NSRange {
    let of = |node: &AccessNode| match (node.grid, node.position) {
        (Some([_, column]), _) if node.role == Role::Cell => Some(column as usize),
        (_, Some([nth, _])) => Some(nth as usize - 1),
        _ => None,
    };
    with_node(element, |told, at| of(&told.nodes[at]).map_or(NSRange::default(), |location| NSRange { location, length: 1 })).unwrap_or_default()
}

/// A column heading's sort order: ascending (1), descending (2), none (0).
extern "C" fn sort_direction(element: Obj, _: Sel) -> isize {
    with_node(element, |told, at| told.nodes[at].sorted.map_or(0, |down| 1 + isize::from(down))).unwrap_or(0)
}

extern "C" fn disclosure_level(element: Obj, _: Sel) -> isize {
    with_node(element, |told, at| told.nodes[at].level.saturating_sub(1) as isize).unwrap_or(0)
}

extern "C" fn disclosed(element: Obj, _: Sel) -> Bool {
    state(element, |node| node.expanded == Some(true))
}

/// A reader opens or closes a tree item: as the arrow keys do, unless it is so already.
extern "C" fn set_disclosed(element: Obj, _: Sel, open: Bool) {
    let open = open != 0;
    if with_node(element, |told, at| told.nodes[at].expanded.is_some_and(|now| now != open)) == Some(true) {
        act(element, move |ui, id| ui.open_for_reader(id, open));
    }
}

/// The item a tree item lies in: the nearest before it a level up.
extern "C" fn disclosed_by(element: Obj, _: Sel) -> Obj {
    let of = |told: &Told, at: usize| {
        let (level, tree) = (told.nodes[at].level, told.nodes[at].parent?);
        let before: Vec<usize> = rows_in(told, tree).take_while(|item| *item < at).collect();
        before.into_iter().rev().take_while(|item| told.nodes[*item].level + 1 >= level).find(|item| told.nodes[*item].level + 1 == level).map(|item| told.elements[item])
    };
    with_node(element, of).flatten().unwrap_or(std::ptr::null_mut())
}

/// The items that lie in a tree item: those after it a level down, until one as high as it.
extern "C" fn disclosed_rows(element: Obj, _: Sel) -> Obj {
    let of = |told: &Told, at: usize| {
        let (level, tree) = (told.nodes[at].level, told.nodes[at].parent.unwrap_or(at));
        let after = rows_in(told, tree).filter(|item| *item > at).take_while(|item| told.nodes[*item].level > level);
        array_of(told, after.filter(|item| told.nodes[*item].level == level + 1))
    };
    with_node(element, of).unwrap_or(std::ptr::null_mut())
}

/// A reader selects (or unselects) a row or tree item: a tree item as a click selects it alone,
/// or as Control+Space unselects it; another as a click. Nothing where it is so already.
extern "C" fn set_selected(element: Obj, _: Sel, on: Bool) {
    let on = on != 0;
    let Some((role, now)) = with_node(element, |told, at| (told.nodes[at].role, told.nodes[at].on == Some(true))) else { return };
    match (role, now == on, on) {
        (_, true, _) => {}
        (_, false, true) => _ = act(element, Ui::press_for_reader),
        (Role::TreeItem, false, false) => _ = act(element, Ui::pick_for_reader),
        _ => {}
    }
}
