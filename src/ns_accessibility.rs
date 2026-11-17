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
    let methods: Vec<(&CStr, *const c_void, String)> = vec![
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

/// The element at a point on the screen: the last one there (content comes after its group).
extern "C" fn view_hit(view: Obj, _: Sel, at: NSPoint) -> Obj {
    asked(view);
    // SAFETY: the screen point turned into the window's, then the (flipped) view's.
    let inside: NSPoint = unsafe {
        let window: Obj = send(view, c"window", ());
        let on_window = send_rect(window, c"convertRectFromScreen:", (NSRect { origin: at, size: NSSize::default() },)).origin;
        send(view, c"convertPoint:fromView:", (on_window, std::ptr::null_mut::<c_void>()))
    };
    let point = crate::geom::pt(inside.x as f32, inside.y as f32);
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
    let Some((view, rect)) = place else { return NSRect::default() };
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
    let version = ui.access_version();
    let nodes = ui.access_nodes();
    let focus_before = TOLD.with_borrow(|told| told.iter().find(|told| told.view == view).and_then(|told| told.nodes.iter().find(|node| node.focused).map(|node| node.id)));
    let changed = TOLD.with_borrow_mut(|told| {
        let at = told.iter().position(|told| told.view == view).unwrap_or_else(|| {
            told.push(Told { view, version: 0, nodes: Vec::new(), elements: Vec::new(), by_id: HashMap::new() });
            told.len() - 1
        });
        let told = &mut told[at];
        (std::mem::replace(&mut told.version, version) != version).then(|| renew(told, nodes))
    });
    // SAFETY: the system is told of elements of the library's, and of its view.
    unsafe {
        if let Some(gone) = changed {
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
