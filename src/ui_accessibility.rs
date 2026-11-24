//! Screen readers on iOS: what the window tells (its access nodes, `accessibility`) as
//! `UIAccessibilityElement`s, which VoiceOver, Voice Control and Switch Control read. Nothing is
//! built until one asks: VoiceOver running as the window opens, or the first time the system
//! asks the view for its elements; the window then tells, and the next run builds the nodes.
//!
//! iOS reads a flat list in reading order, so the elements are the nodes a reader stops at:
//! parts and text, not the boxes holding them. Each is an element of the library's class
//! (`TautElement`), kept from run to run while its node's id is told, so a reader keeps its
//! place. Roles become traits:
//!
//! | Node | Traits |
//! | --- | --- |
//! | button, menu item, fold, drop-down; check box, switch, radio button, tab | button (selected when on) |
//! | link; heading; picture | link; header; image |
//! | slider, spin button | adjustable (swipe up and down steps it) |
//! | progress | updates frequently, its value a percentage |
//! | text, paragraph, code, cell | static text |
//!
//! A disabled part is not enabled; a password box's value is dots. What a reader does (activate,
//! step, move to a part) reaches the interface as input does, in the next turn: activating a
//! text box focuses it, and the keyboard comes. A new layout, focus moving and what the app
//! announces are posted to the reader.
use crate::accessibility::{AccessNode, Role};
use crate::id::Id;
use crate::ios::{Heard, heard};
use crate::macos_sys::*;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_void};

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    fn UIAccessibilityIsVoiceOverRunning() -> bool;
    fn UIAccessibilityPostNotification(notification: u32, argument: Obj);
    fn UIAccessibilityConvertFrameToScreenCoordinates(rect: NSRect, view: Obj) -> NSRect;
    static UIAccessibilityLayoutChangedNotification: u32;
    static UIAccessibilityAnnouncementNotification: u32;
    static UIAccessibilityTraitButton: u64;
    static UIAccessibilityTraitLink: u64;
    static UIAccessibilityTraitHeader: u64;
    static UIAccessibilityTraitImage: u64;
    static UIAccessibilityTraitSelected: u64;
    static UIAccessibilityTraitNotEnabled: u64;
    static UIAccessibilityTraitStaticText: u64;
    static UIAccessibilityTraitAdjustable: u64;
    static UIAccessibilityTraitUpdatesFrequently: u64;
}

/// What the window tells: its view, where the page lies in it, its nodes as the last run built
/// them, and the element of each node a reader stops at (null for the rest).
struct Told {
    view: Obj,
    origin: (f64, f64),
    version: u64,
    nodes: Vec<AccessNode>,
    elements: Vec<Obj>,
    /// The elements kept, by their node's id; and each element's node, by the element.
    by_id: HashMap<Id, Obj>,
    node_of: HashMap<usize, usize>,
}

thread_local! {
    static TOLD: RefCell<Option<Told>> = const { RefCell::new(None) };
    /// A reader has asked something: the window tells from now on.
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// What a reader asks of the view: its elements. (It is not one itself.)
pub(crate) fn view_methods() -> Vec<(&'static CStr, *const c_void, String)> {
    vec![(c"accessibilityElements", view_elements as *const c_void, "@@:".into()), (c"isAccessibilityElement", no as *const c_void, format!("{BOOL}@:"))]
}

/// The element class, made once.
fn element_class() -> Obj {
    let (object, yes_no, plain) = ("@@:", format!("{BOOL}@:"), "v@:");
    let methods: [(&CStr, *const c_void, &str); 10] = [
        (c"accessibilityLabel", label as *const c_void, object),
        (c"accessibilityValue", value as *const c_void, object),
        (c"accessibilityIdentifier", identifier as *const c_void, object),
        (c"accessibilityTraits", traits as *const c_void, "Q@:"),
        (c"accessibilityFrame", frame as *const c_void, RECT_GETTER),
        (c"isAccessibilityElement", yes as *const c_void, &yes_no),
        (c"accessibilityActivate", activate as *const c_void, &yes_no),
        (c"accessibilityIncrement", increment as *const c_void, plain),
        (c"accessibilityDecrement", decrement as *const c_void, plain),
        (c"accessibilityElementDidBecomeFocused", became_focused as *const c_void, plain),
    ];
    made_class(c"TautElement", c"UIAccessibilityElement", &methods, &[])
}

/// A method answering a rectangle.
const RECT_GETTER: &str = "{CGRect={CGPoint=dd}{CGSize=dd}}@:";

extern "C" fn yes(_: Obj, _: Sel) -> Bool {
    1
}

extern "C" fn no(_: Obj, _: Sel) -> Bool {
    0
}

/// With VoiceOver running as the window opens, it tells from the first run.
pub(crate) fn listen_if_running(_view: Obj) {
    // SAFETY: a plain question.
    if unsafe { UIAccessibilityIsVoiceOverRunning() } {
        LISTENING.set(true);
    }
}

/// In a turn: once a reader asked, the interface tells (`Ui::set_screen_reader`). Says whether
/// that is news, for the interface to run.
pub(crate) fn listen(ui: &mut Ui) -> bool {
    if !LISTENING.get() || ui.listening {
        return false;
    }
    ui.set_screen_reader(true);
    true
}

/// Notes that a reader asks: from the next turn the window tells.
fn asked() {
    if !LISTENING.replace(true) {
        heard(Heard::Other(Box::new(|_| true)));
    }
}

/// The view's elements, in reading order.
extern "C" fn view_elements(_: Obj, _: Sel) -> Obj {
    asked();
    let elements = TOLD.with_borrow(|told| told.as_ref().map(|told| told.elements.iter().copied().filter(|element| !element.is_null()).collect::<Vec<Obj>>()));
    ns_array(&elements.unwrap_or_default())
}

/// Whether a reader stops at a node: a part or text, not a box holding others.
fn read(node: &AccessNode) -> bool {
    let holds = matches!(node.role, Role::Group | Role::Dialog | Role::Menu | Role::MenuBar | Role::Tabs | Role::Table | Role::ToolBar | Role::Tree | Role::Document | Role::List | Role::Quote);
    let says = !node.name.is_empty() || !node.value.is_empty();
    let acts = !matches!(node.role, Role::Text | Role::Paragraph | Role::Code | Role::Row | Role::TableRow | Role::ListItem | Role::Cell | Role::ColumnHeader | Role::Heading | Role::Picture);
    node.visible && !holds && (says || acts)
}

/// Works with an element's node (and where it is among the nodes).
fn with_node<T>(element: Obj, work: impl FnOnce(&Told, usize, &AccessNode) -> T) -> Option<T> {
    TOLD.with_borrow(|told| {
        let told = told.as_ref()?;
        let at = *told.node_of.get(&(element as usize))?;
        told.nodes.get(at).map(|node| work(told, at, node))
    })
}

extern "C" fn label(element: Obj, _: Sel) -> Obj {
    with_node(element, |_, _, node| ns_string(&node.name)).unwrap_or(std::ptr::null_mut())
}

/// A node's value as said: a text box's text (dots for a password), a slider's amount, a
/// progress bar's percentage.
fn said_value(node: &AccessNode) -> String {
    match (node.role, node.value_range) {
        (Role::PasswordBox, _) => "\u{2022}".repeat(node.value.chars().count()),
        (Role::Progress, Some([amount, low, high])) if high > low => format!("{:.0}%", (amount - low) / (high - low) * 100.0),
        (_, Some([amount, ..])) if node.value.is_empty() => format!("{amount}"),
        _ => node.value.clone(),
    }
}

extern "C" fn value(element: Obj, _: Sel) -> Obj {
    with_node(element, |_, _, node| {
        let said = said_value(node);
        if said.is_empty() { std::ptr::null_mut() } else { ns_string(&said) }
    })
    .unwrap_or(std::ptr::null_mut())
}

/// The node's id, for tests and automation to find it by.
extern "C" fn identifier(element: Obj, _: Sel) -> Obj {
    with_node(element, |_, _, node| ns_string(&format!("{:x}", node.id))).unwrap_or(std::ptr::null_mut())
}

/// A role's traits, and the node's state: selected, not enabled.
fn traits_of(node: &AccessNode) -> u64 {
    // SAFETY: UIKit's constants, read.
    let (button, link, header, image, selected, off, text, adjustable, updating) = unsafe {
        (
            UIAccessibilityTraitButton,
            UIAccessibilityTraitLink,
            UIAccessibilityTraitHeader,
            UIAccessibilityTraitImage,
            UIAccessibilityTraitSelected,
            UIAccessibilityTraitNotEnabled,
            UIAccessibilityTraitStaticText,
            UIAccessibilityTraitAdjustable,
            UIAccessibilityTraitUpdatesFrequently,
        )
    };
    let role = match node.role {
        Role::Button | Role::MenuItem | Role::MenuItemRadio | Role::Submenu | Role::Fold | Role::ComboBox => button,
        Role::CheckBox | Role::Switch | Role::RadioButton | Role::Tab => button,
        Role::Link => link,
        Role::Heading => header,
        Role::Picture => image,
        Role::Slider | Role::SpinButton => adjustable,
        Role::Progress => updating,
        Role::TextBox | Role::PasswordBox => 0,
        _ => text,
    };
    let on = if node.on == Some(true) { selected } else { 0 };
    role | on | if node.enabled { 0 } else { off }
}

extern "C" fn traits(element: Obj, _: Sel) -> u64 {
    with_node(element, |_, _, node| traits_of(node)).unwrap_or(0)
}

/// Where a node is on the screen, in the screen's points.
extern "C" fn frame(element: Obj, _: Sel) -> NSRect {
    let place = with_node(element, |told, at, _| (told.view, told.origin, crate::accessibility::view(&told.nodes, at)));
    let Some((view, (x, y), rect)) = place else { return NSRect::default() };
    let rect = NSRect::new(x + f64::from(rect.x), y + f64::from(rect.y), f64::from(rect.w), f64::from(rect.h));
    // SAFETY: the view's rectangle turned into the screen's.
    unsafe { UIAccessibilityConvertFrameToScreenCoordinates(rect, view) }
}

/// Has the window do something to an element's node in the next turn: what a reader asked.
fn act(element: Obj, deed: impl FnOnce(&mut Ui, Id) + 'static) -> Bool {
    let Some(id) = with_node(element, |_, _, node| node.id) else { return 0 };
    heard(Heard::Other(Box::new(move |ui: &mut Ui| {
        deed(ui, id);
        true
    })));
    1
}

/// A double tap: a text box takes focus (and the keyboard comes); anything else is pressed.
extern "C" fn activate(element: Obj, _: Sel) -> Bool {
    let typed = with_node(element, |_, _, node| matches!(node.role, Role::TextBox | Role::PasswordBox)).unwrap_or(false);
    if typed { act(element, |ui, id| ui.set_focus(id)) } else { act(element, Ui::press_for_reader) }
}

extern "C" fn increment(element: Obj, _: Sel) {
    act(element, |ui, id| ui.step_for_reader(id, true));
}

extern "C" fn decrement(element: Obj, _: Sel) {
    act(element, |ui, id| ui.step_for_reader(id, false));
}

/// A reader moved to a part: it is scrolled into view.
extern "C" fn became_focused(element: Obj, _: Sel) {
    act(element, Ui::show_for_reader);
}

/// After a run: the elements follow the nodes, where they changed. An element is made for each
/// new node read and let go of with its node; the reader hears that the layout changed (and
/// where focus went) and what the app announced.
pub(crate) fn keep(view: Obj, origin: (f64, f64), ui: &Ui) {
    let version = ui.access_version();
    let nodes = ui.access_nodes();
    let renewed = TOLD.with_borrow_mut(|told| {
        let told = told.get_or_insert_with(|| Told { view, origin, version: 0, nodes: Vec::new(), elements: Vec::new(), by_id: HashMap::new(), node_of: HashMap::new() });
        told.origin = origin;
        (std::mem::replace(&mut told.version, version) != version).then(|| renew(told, nodes))
    });
    if let Some(focus_moved) = renewed {
        // SAFETY: the reader hears of the view's new layout, moving to the focused element.
        unsafe { UIAccessibilityPostNotification(UIAccessibilityLayoutChangedNotification, focus_moved) };
    }
    for text in ui.announced() {
        // SAFETY: the reader is handed a string to say.
        unsafe { UIAccessibilityPostNotification(UIAccessibilityAnnouncementNotification, ns_string(text)) };
    }
}

/// The elements made to match the nodes: kept by id, made for new ones, the rest let go of.
/// Gives back the focused element if focus moved to another, else nil.
fn renew(told: &mut Told, nodes: &[AccessNode]) -> Obj {
    let focused_before = told.nodes.iter().find(|node| node.focused).map(|node| node.id);
    let class = element_class();
    let mut by_id = std::mem::take(&mut told.by_id);
    let mut kept = HashMap::with_capacity(nodes.len());
    let view = told.view;
    let mut element_for = |node: &AccessNode| *kept.entry(node.id).or_insert_with(|| by_id.remove(&node.id).unwrap_or_else(|| made_element(class, view)));
    let elements: Vec<Obj> = nodes.iter().map(|node| if read(node) { element_for(node) } else { std::ptr::null_mut() }).collect();
    told.node_of = elements.iter().enumerate().filter(|(_, element)| !element.is_null()).map(|(at, element)| (*element as usize, at)).collect();
    (told.nodes, told.elements, told.by_id) = (nodes.to_vec(), elements, kept);
    // SAFETY: elements this made, each released once.
    by_id.into_values().for_each(|element| unsafe { send::<()>(element, c"release", ()) });
    let focused = nodes.iter().position(|node| node.focused);
    match focused {
        Some(at) if Some(nodes[at].id) != focused_before => told.elements[at],
        _ => std::ptr::null_mut(),
    }
}

/// A new element of the library's class, in the view (retained: let go of in `renew`).
fn made_element(class: Obj, view: Obj) -> Obj {
    // SAFETY: an element made for its container, the view.
    unsafe { send(send::<Obj>(class, c"alloc", ()), c"initWithAccessibilityContainer:", (view,)) }
}
