//! Screen readers on iOS: what the window tells (its access nodes, `accessibility`) as
//! `UIAccessibilityElement`s, which VoiceOver, Voice Control and Switch Control read. Nothing is
//! built until one asks: VoiceOver running as the window opens, or the first time the system
//! asks the view for its elements; the window then tells, and the next run builds the nodes.
//!
//! iOS reads a list in reading order, so the elements are the nodes a reader stops at: parts
//! and text, not the boxes holding them. A table is the one container: a data table
//! (`UIAccessibilityContainerDataTable`) holding its headings and cells, which give their row
//! and column. Its rows are all its rows, told or not: a reader asking for a cell out of view,
//! or scrolling it by a page, has that row shown at once. Each element is of the library's
//! class (`TautElement`), kept from run to run while its node's id is told, so a reader keeps
//! its place. Text is read by line as wrapped, and by letter where it is drawn
//! (`ui_accessibility_text`). Roles become traits:
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
use crate::geom::Rect;
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

/// `UIAccessibilityContainerTypeDataTable`.
const DATA_TABLE: isize = 1;
/// `UIAccessibilityScrollDirection`s that page on (up, next) and back (down, previous).
const PAGE_ON: [isize; 2] = [3, 5];
const PAGE_BACK: [isize; 2] = [4, 6];

/// What the window tells: its view, where the page lies in it, its nodes as the last run built
/// them, and the element of each node a reader stops at (null for the rest).
pub(crate) struct Told {
    pub(crate) view: Obj,
    pub(crate) origin: (f64, f64),
    version: u64,
    pub(crate) nodes: Vec<AccessNode>,
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
    let (object, yes_no, plain) = ("@@:".to_string(), format!("{BOOL}@:"), "v@:".to_string());
    let (count, range) = ("Q@:".to_string(), format!("{RANGE}@:"));
    let mut methods: Vec<(&CStr, *const c_void, String)> = vec![
        (c"accessibilityLabel", label as *const c_void, object.clone()),
        (c"accessibilityValue", value as *const c_void, object.clone()),
        (c"accessibilityIdentifier", identifier as *const c_void, object.clone()),
        (c"accessibilityTraits", traits as *const c_void, "Q@:".into()),
        (c"accessibilityFrame", frame as *const c_void, format!("{RECT}@:")),
        (c"isAccessibilityElement", is_element as *const c_void, yes_no.clone()),
        (c"accessibilityContainer", container as *const c_void, object.clone()),
        (c"accessibilityActivate", activate as *const c_void, yes_no.clone()),
        (c"accessibilityIncrement", increment as *const c_void, plain.clone()),
        (c"accessibilityDecrement", decrement as *const c_void, plain.clone()),
        (c"accessibilityElementDidBecomeFocused", became_focused as *const c_void, plain),
        (c"accessibilityContainerType", container_type as *const c_void, "q@:".into()),
        (c"accessibilityElements", table_elements as *const c_void, object.clone()),
        (c"accessibilityRowCount", row_count as *const c_void, count.clone()),
        (c"accessibilityColumnCount", column_count as *const c_void, count),
        (c"accessibilityDataTableCellElementForRow:column:", cell_for as *const c_void, "@@:QQ".into()),
        (c"accessibilityHeaderElementsForColumn:", headings_for as *const c_void, "@@:Q".into()),
        (c"accessibilityRowRange", row_range as *const c_void, range.clone()),
        (c"accessibilityColumnRange", column_range as *const c_void, range),
        (c"accessibilityScroll:", scroll as *const c_void, format!("{BOOL}@:q")),
    ];
    methods.extend(crate::ui_accessibility_text::element_methods());
    let listed: Vec<(&CStr, *const c_void, &str)> = methods.iter().map(|(name, imp, types)| (*name, *imp, types.as_str())).collect();
    let protocols = [c"UIAccessibilityContainerDataTable", c"UIAccessibilityContainerDataTableCell", c"UIAccessibilityReadingContent"];
    made_class(c"TautElement", c"UIAccessibilityElement", &listed, &protocols)
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

/// The view's elements, in reading order: those not in a table, and each table.
extern "C" fn view_elements(_: Obj, _: Sel) -> Obj {
    asked();
    let elements = TOLD.with_borrow(|told| told.as_ref().map(|told| told.elements_within(None)));
    ns_array(&elements.unwrap_or_default())
}

impl Told {
    /// The elements directly in a table (by node index), or in none.
    fn elements_within(&self, table: Option<usize>) -> Vec<Obj> {
        let within = |at: &usize| !self.elements[*at].is_null() && table_above(&self.nodes, *at) == table;
        (0..self.nodes.len()).filter(within).map(|at| self.elements[at]).collect()
    }
    /// The element of node `at`, or null.
    fn element(&self, at: usize) -> Obj {
        self.elements.get(at).copied().unwrap_or(std::ptr::null_mut())
    }
}

/// The table node `at` is in, if any.
fn table_above(nodes: &[AccessNode], at: usize) -> Option<usize> {
    let mut up = nodes[at].parent;
    while let Some(group) = up {
        if nodes[group].role == Role::Table {
            return Some(group);
        }
        up = nodes[group].parent;
    }
    None
}

/// Whether a reader stops at a node: a part or text, not a box holding others; and every cell
/// of a table (a data table has all its cells), and the table itself (a container).
fn has_element(node: &AccessNode) -> bool {
    let holds = matches!(node.role, Role::Group | Role::Dialog | Role::Menu | Role::MenuBar | Role::Tabs | Role::Table | Role::ToolBar | Role::Tree | Role::Document | Role::List | Role::Quote);
    let says = !node.name.is_empty() || !node.value.is_empty();
    let acts = !matches!(node.role, Role::Text | Role::Paragraph | Role::Code | Role::Row | Role::TableRow | Role::ListItem | Role::Cell | Role::ColumnHeader | Role::Heading | Role::Picture);
    node.visible && (matches!(node.role, Role::Table | Role::Cell) || !holds && (says || acts))
}

/// Works with an element's node (and where it is among the nodes).
pub(crate) fn with_node<T>(element: Obj, work: impl FnOnce(&Told, usize, &AccessNode) -> T) -> Option<T> {
    TOLD.with_borrow(|told| {
        let told = told.as_ref()?;
        let at = *told.node_of.get(&(element as usize))?;
        told.nodes.get(at).map(|node| work(told, at, node))
    })
}

/// A table is a container, not an element itself.
extern "C" fn is_element(element: Obj, _: Sel) -> Bool {
    Bool::from(with_node(element, |_, _, node| node.role != Role::Table).unwrap_or(true))
}

/// What an element lies in: its table, or the view.
extern "C" fn container(element: Obj, _: Sel) -> Obj {
    with_node(element, |told, at, _| table_above(&told.nodes, at).map_or(told.view, |table| told.element(table))).unwrap_or(std::ptr::null_mut())
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
        Role::Heading | Role::ColumnHeader => header,
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
    with_node(element, |told, at, _| told.on_screen(crate::accessibility::view(&told.nodes, at))).unwrap_or_default()
}

impl Told {
    /// A rectangle of the page, in the view's points.
    pub(crate) fn in_view(&self, rect: Rect) -> NSRect {
        NSRect::new(self.origin.0 + f64::from(rect.x), self.origin.1 + f64::from(rect.y), f64::from(rect.w), f64::from(rect.h))
    }
    /// A rectangle of the page, in the screen's points.
    pub(crate) fn on_screen(&self, rect: Rect) -> NSRect {
        // SAFETY: the view's rectangle turned into the screen's.
        unsafe { UIAccessibilityConvertFrameToScreenCoordinates(self.in_view(rect), self.view) }
    }
    /// A point on the screen, in the page's units.
    pub(crate) fn screen_point_in_page(&self, at: NSPoint) -> crate::geom::Point {
        let corner = self.on_screen(Rect::default()).origin;
        crate::geom::pt((at.x - corner.x) as f32, (at.y - corner.y) as f32)
    }
    /// A point in the view, in the page's units.
    pub(crate) fn in_page(&self, at: NSPoint) -> crate::geom::Point {
        crate::geom::pt((at.x - self.origin.0) as f32, (at.y - self.origin.1) as f32)
    }
}

/// Has the window do something to an element's node in the next turn: what a reader asked.
pub(crate) fn act(element: Obj, deed: impl FnOnce(&mut Ui, Id) + 'static) -> Bool {
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

// ---- tables ----

extern "C" fn container_type(element: Obj, _: Sel) -> isize {
    with_node(element, |_, _, node| if node.role == Role::Table { DATA_TABLE } else { 0 }).unwrap_or(0)
}

/// A table's headings and cells, in reading order. (Nothing else is a container.)
extern "C" fn table_elements(element: Obj, _: Sel) -> Obj {
    let within = with_node(element, |told, at, node| (node.role == Role::Table).then(|| told.elements_within(Some(at)))).flatten();
    within.map_or(std::ptr::null_mut(), |elements| ns_array(&elements))
}

/// A table's rows, all of them; its columns, those shown.
extern "C" fn row_count(element: Obj, _: Sel) -> usize {
    with_node(element, |_, _, node| node.grid.map_or(0, |[rows, _]| rows as usize)).unwrap_or(0)
}

extern "C" fn column_count(element: Obj, _: Sel) -> usize {
    with_node(element, |_, _, node| node.grid.map_or(0, |[_, columns]| columns as usize)).unwrap_or(0)
}

/// A table's cell, as told; a cell of a row out of view: its row is shown at once (the window
/// runs), then its cell is given.
extern "C" fn cell_for(element: Obj, _: Sel, row: usize, column: usize) -> Obj {
    let place = [row as u32, column as u32];
    if let Some(cell) = cell_element(element, place) {
        return cell;
    }
    let inside = with_node(element, |_, _, node| node.grid.is_some_and(|[rows, columns]| place[0] < rows && place[1] < columns));
    if inside != Some(true) {
        return std::ptr::null_mut();
    }
    act(element, move |ui, table| ui.row_for_reader(table, place[0], None));
    crate::ios::turn();
    cell_element(element, place).unwrap_or(std::ptr::null_mut())
}

/// The element of a table's cell at a row and column, if told.
fn cell_element(table: Obj, place: [u32; 2]) -> Option<Obj> {
    with_node(table, |told, at, _| crate::accessibility::cell_at(&told.nodes, at, place).map(|cell| told.element(cell))).flatten()
}

/// The heading over a table's column.
extern "C" fn headings_for(element: Obj, _: Sel, column: usize) -> Obj {
    let heading = with_node(element, |told, at, _| crate::accessibility::heading_at(&told.nodes, at, column as u32).map(|heading| told.element(heading))).flatten();
    ns_array(&heading.into_iter().collect::<Vec<Obj>>())
}

/// A cell's row; for anything else, none.
extern "C" fn row_range(element: Obj, _: Sel) -> NSRange {
    let row = with_node(element, |_, _, node| node.grid.filter(|_| node.role == Role::Cell).map(|[row, _]| row as usize)).flatten();
    row.map_or(NSRange { location: NOT_FOUND, length: 0 }, |location| NSRange { location, length: 1 })
}

/// A cell's column; a column heading's, from its place among the headings.
extern "C" fn column_range(element: Obj, _: Sel) -> NSRange {
    let of = |node: &AccessNode| match (node.role, node.grid, node.position) {
        (Role::Cell, Some([_, column]), _) => Some(column as usize),
        (Role::ColumnHeader, _, Some([nth, _])) => Some(nth as usize - 1),
        _ => None,
    };
    let column = with_node(element, |_, _, node| of(node)).flatten();
    column.map_or(NSRange { location: NOT_FOUND, length: 0 }, |location| NSRange { location, length: 1 })
}

/// A three-finger swipe over a long table: a page of rows on, or back, shown at once; the
/// reader moves to the first cell of the rows now in view.
extern "C" fn scroll(element: Obj, _: Sel, direction: isize) -> Bool {
    let forward = PAGE_ON.contains(&direction);
    if !forward && !PAGE_BACK.contains(&direction) {
        return 0;
    }
    let page_away = |told: &Told, at: usize, node: &AccessNode| if node.role == Role::Table { crate::accessibility::row_a_page_away(&told.nodes, at, forward) } else { None };
    let Some(row) = with_node(element, page_away).flatten() else { return 0 };
    act(element, move |ui, table| ui.row_for_reader(table, row, None));
    crate::ios::turn();
    let first = with_node(element, |told, at, _| first_cell_in_view(told, at)).flatten();
    // SAFETY: the reader hears the layout changed, and moves to the element given (nil: none).
    unsafe { UIAccessibilityPostNotification(UIAccessibilityLayoutChangedNotification, first.unwrap_or(std::ptr::null_mut())) };
    1
}

/// The element of the first cell of a table's rows in view.
fn first_cell_in_view(told: &Told, table: usize) -> Option<Obj> {
    let [first, _] = crate::accessibility::rows_in_view(&told.nodes, table)?;
    crate::accessibility::cell_at(&told.nodes, table, [first, 0]).map(|cell| told.element(cell))
}

// ---- keeping up ----

/// After a run: the elements follow the nodes, where they changed. An element is made for each
/// new node read and let go of with its node; the reader hears that the layout changed (and
/// where focus went) and what the app announced. Nodes that only moved (a scroll) keep their
/// elements, which follow them.
pub(crate) fn keep(view: Obj, origin: (f64, f64), ui: &Ui) {
    let (version, moved_from) = (ui.access_version(), ui.access_moved_from());
    let nodes = ui.access_nodes();
    let changed = TOLD.with_borrow_mut(|told| {
        let told = told.get_or_insert_with(|| Told { view, origin, version: 0, nodes: Vec::new(), elements: Vec::new(), by_id: HashMap::new(), node_of: HashMap::new() });
        told.origin = origin;
        let was = std::mem::replace(&mut told.version, version);
        if moved_from == Some(was) && told.nodes.len() == nodes.len() {
            crate::accessibility::follow_move(&mut told.nodes, nodes);
            return Some(std::ptr::null_mut());
        }
        (was != version).then(|| renew(told, nodes))
    });
    if let Some(focus_moved) = changed {
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
    let elements: Vec<Obj> = nodes.iter().map(|node| if has_element(node) { element_for(node) } else { std::ptr::null_mut() }).collect();
    told.node_of = elements.iter().enumerate().filter(|(_, element)| !element.is_null()).map(|(at, element)| (*element as usize, at)).collect();
    (told.nodes, told.elements, told.by_id) = (nodes.to_vec(), elements, kept);
    by_id.into_values().for_each(let_go);
    let focused = nodes.iter().position(|node| node.focused);
    match focused {
        Some(at) if Some(nodes[at].id) != focused_before => told.element(at),
        _ => std::ptr::null_mut(),
    }
}

/// A new element of the library's class, in the view (retained: let go of in `renew`).
fn made_element(class: Obj, view: Obj) -> Obj {
    // SAFETY: an element made for its container, the view.
    unsafe { send(send::<Obj>(class, c"alloc", ()), c"initWithAccessibilityContainer:", (view,)) }
}

/// Lets go of an element whose node is gone, and of what reads its text.
fn let_go(element: Obj) {
    crate::ui_accessibility_text::forget(element);
    // SAFETY: an element this made, released once.
    unsafe { send::<()>(element, c"release", ()) };
}
