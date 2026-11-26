//! What Android's screen readers (TalkBack, and `uiautomator`) are told: the window's nodes as
//! virtual views of the library's view, through its `AccessibilityNodeProvider` (the class
//! `taut.Nodes`, written by `android_dex`, whose methods are the functions here). Each node
//! is an `AccessibilityNodeInfo` with the class name of the Android widget it is like
//! (`android.widget.Button`, `CheckBox`, `EditText`, `SeekBar`), its text, state, bounds on
//! the screen and the actions it takes. A reader's actions come back as the interface's own
//! (`press_for_reader` and the like). A reader exploring by touch sends hover events, which
//! the window hears (`hover`); the node under the finger is announced.
//!
//! Text is read by character, word, line (as wrapped) and paragraph, and each letter's place on
//! the screen is given (`EXTRA_DATA_TEXT_CHARACTER_LOCATION_KEY`). A table is a collection:
//! its rows count all rows, told or not, and a reader scrolling it, or asking for a row out of
//! view, has that row shown.
use crate::accessibility::{AccessNode, Role, TextUnit, letter_at_unit, units_before};
use crate::android::{Heard, activity, heard, kick, view};
use crate::android_jni::{Env, Native, Obj, Value};
use crate::geom::{Point, Rect};
use crate::id::Id;
use crate::ui::Ui;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{CStr, c_int, c_void};

/// The host view's own node, and the actions used (`AccessibilityNodeInfo.ACTION_*`).
const HOST: i32 = -1;
const FOCUS: i32 = 1;
const SELECT: i32 = 4;
const CLICK: i32 = 16;
const ACCESSIBILITY_FOCUS: i32 = 64;
const CLEAR_ACCESSIBILITY_FOCUS: i32 = 128;
const NEXT_AT_GRANULARITY: i32 = 256;
const PREVIOUS_AT_GRANULARITY: i32 = 512;
const SCROLL_FORWARD: i32 = 0x1000;
const SCROLL_BACKWARD: i32 = 0x2000;
const SET_SELECTION: i32 = 0x2_0000;
const EXPAND: i32 = 0x4_0000;
const COLLAPSE: i32 = 0x8_0000;
const SET_TEXT: i32 = 0x20_0000;
/// `android.R.id.accessibilityActionShowOnScreen`, `…ScrollToPosition` and `…SetProgress`.
const SHOW_ON_SCREEN: i32 = 0x0102_0036;
const SCROLL_TO_POSITION: i32 = 0x0102_0037;
const SET_PROGRESS: i32 = 0x0102_003d;
/// The units text is read by (`MOVEMENT_GRANULARITY_*`): characters, words, lines, paragraphs.
const GRANULARITIES: [(i32, TextUnit); 4] = [(1, TextUnit::Character), (2, TextUnit::Word), (4, TextUnit::Line), (8, TextUnit::Paragraph)];
/// What a reader asks for each letter's place by, and the most letters it may ask for.
const LETTER_PLACES: &str = "android.view.accessibility.extra.DATA_TEXT_CHARACTER_LOCATION_KEY";
const MOST_LETTERS: i32 = 20_000;
/// Event types (`AccessibilityEvent.TYPE_*`).
const VIEW_FOCUSED: i32 = 8;
const HOVER_ENTER: i32 = 128;
const HOVER_EXIT: i32 = 256;
const CONTENT_CHANGED: i32 = 2048;
const TEXT_TRAVERSED: i32 = 0x2_0000;
const ACCESSIBILITY_FOCUSED: i32 = 0x8000;
const ACCESSIBILITY_FOCUS_CLEARED: i32 = 0x1_0000;
/// Which focus `findFocus` asks for.
const INPUT_FOCUS: i32 = 1;

/// What was told last, and the virtual views' ids.
#[derive(Default)]
struct Told {
    /// A reader asked for nodes: from then on the interface tells.
    asked: bool,
    version: u64,
    nodes: Vec<AccessNode>,
    /// Each node's virtual view id, kept from run to run; and the next one free.
    ids: HashMap<Id, i32>,
    next: i32,
    /// The node with the reader's focus, and the one under an exploring finger.
    reader_focus: Option<i32>,
    hovered: Option<i32>,
    /// The node with keyboard focus, as last told.
    focused: Option<i32>,
    /// Where a reader is in each text it reads that is not a text box's (whose cursor is), by
    /// character.
    reading: HashMap<i32, u32>,
    /// The provider object (`taut.Nodes`), made once.
    provider: Obj,
}

thread_local! {
    static TOLD: RefCell<Told> = RefCell::new(Told { next: 1, ..Told::default() });
}

/// Whether a reader asked for the window's nodes.
pub(crate) fn asked() -> bool {
    TOLD.with_borrow(|told| told.asked)
}

/// The activity went: its view's virtual views go with it.
pub(crate) fn forget() {
    TOLD.with_borrow_mut(|told| (told.reader_focus, told.hovered, told.focused, told.provider) = (None, None, None, std::ptr::null_mut()));
}

/// The provider's native methods.
pub(crate) fn natives() -> [Native; 4] {
    [
        Native::new(c"createAccessibilityNodeInfo", c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;", node_info as *const c_void),
        Native::new(c"performAction", c"(IILandroid/os/Bundle;)Z", perform as *const c_void),
        Native::new(c"findFocus", c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;", find_focus as *const c_void),
        Native::new(c"addExtraDataToAccessibilityNodeInfo", c"(ILandroid/view/accessibility/AccessibilityNodeInfo;Ljava/lang/String;Landroid/os/Bundle;)V", extra_data as *const c_void),
    ]
}

/// The view is asked for its provider: one, made the first time. A reader asking means the
/// interface tells from now on; it runs at once, so the first answer has the nodes.
pub(crate) extern "C" fn provider(env: Env, _: Obj) -> Obj {
    let first = TOLD.with_borrow_mut(|told| !std::mem::replace(&mut told.asked, true));
    if first {
        heard(Heard::Other(Box::new(|_| true)));
        kick();
    }
    let made = TOLD.with_borrow(|told| told.provider);
    if !made.is_null() {
        return made;
    }
    let class = crate::android::classes()[1];
    let provider = env.global(env.new_object(class, c"()V", &[]));
    TOLD.with_borrow_mut(|told| told.provider = provider);
    provider
}

/// After a run: the nodes are kept, and readers hear that the window changed, and where focus
/// went. (Nodes that only moved, as in a scroll, are followed where they went.)
pub(crate) fn keep(ui: &Ui) {
    let (version, moved_from) = (ui.access_version(), ui.access_moved_from());
    let changed = TOLD.with_borrow_mut(|told| {
        let was = std::mem::replace(&mut told.version, version);
        if was == version {
            return None;
        }
        let nodes = ui.access_nodes();
        if moved_from == Some(was) && told.nodes.len() == nodes.len() {
            crate::accessibility::follow_move(&mut told.nodes, nodes);
            return Some(None);
        }
        told.nodes = nodes.to_vec();
        let Told { nodes, ids, next, .. } = told;
        for node in nodes.iter() {
            ids.entry(node.id).or_insert_with(|| (*next, *next += 1).0);
        }
        let focused = told.nodes.iter().find(|node| node.focused).and_then(|node| told.ids.get(&node.id).copied());
        Some((std::mem::replace(&mut told.focused, focused) != focused).then_some(focused).flatten())
    });
    let Some(focus_moved) = changed else { return };
    send(HOST, CONTENT_CHANGED);
    focus_moved.into_iter().for_each(|id| send(id, VIEW_FOCUSED));
    for said in ui.announced() {
        announce(said);
    }
}

/// The virtual view id of a node, and the node.
fn node_of(told: &Told, id: i32) -> Option<&AccessNode> {
    index_of(told, id).map(|at| &told.nodes[at])
}

/// Where a virtual view's node is among the nodes.
fn index_of(told: &Told, id: i32) -> Option<usize> {
    told.nodes.iter().position(|node| told.ids.get(&node.id) == Some(&id))
}

/// The class of the Android widget a role is like: what a reader names it by.
fn class_of(role: Role) -> &'static str {
    match role {
        Role::Button | Role::MenuItem | Role::MenuItemRadio | Role::Submenu | Role::Tab | Role::ColumnHeader => "android.widget.Button",
        Role::CheckBox => "android.widget.CheckBox",
        Role::Switch => "android.widget.Switch",
        Role::RadioButton => "android.widget.RadioButton",
        Role::TextBox | Role::PasswordBox | Role::SpinButton => "android.widget.EditText",
        Role::Slider => "android.widget.SeekBar",
        Role::Progress => "android.widget.ProgressBar",
        Role::ComboBox => "android.widget.Spinner",
        Role::Picture => "android.widget.ImageView",
        Role::Tabs => "android.widget.TabWidget",
        Role::Table => "android.widget.GridView",
        Role::Tree | Role::Menu | Role::MenuBar => "android.widget.ListView",
        Role::Text | Role::Heading | Role::Paragraph | Role::Link => "android.widget.TextView",
        _ => "android.view.ViewGroup",
    }
}

/// Whether a reader can press a node.
fn pressable(role: Role) -> bool {
    use Role::*;
    matches!(role, Button | CheckBox | Switch | RadioButton | Tab | Row | MenuItem | MenuItemRadio | Submenu | ComboBox | Link | TreeItem | TableRow | ColumnHeader | Fold)
}

/// Where a node is on the screen, in pixels: left, top, right, bottom.
fn on_screen(env: Env, node: &AccessNode) -> [i32; 4] {
    let (view, scale, insets) = view();
    let origin = window_origin(env, view);
    let rect = if node.seen.w > 0.0 { node.seen } else { node.rect };
    let edge = |at: f32, offset: i32| (at * scale).round() as i32 + offset;
    [edge(rect.x, origin[0] + insets[0]), edge(rect.y, origin[1] + insets[1]), edge(rect.x + rect.w, origin[0] + insets[0]), edge(rect.y + rect.h, origin[1] + insets[1])]
}

/// Where the window's top left is on the screen.
fn window_origin(env: Env, view: Obj) -> [i32; 2] {
    let root = env.call(view, c"getRootView", c"()Landroid/view/View;", &[]).object();
    let place = crate::android_jni::new_ints(env, 2);
    env.call(root, c"getLocationOnScreen", c"([I)V", &[Value::obj(place)]);
    let mut xy = [0i32; 2];
    crate::android_jni::ints(env, place, &mut xy);
    xy
}

/// A reader asks for a node: the host's (with the top nodes as its children), or a virtual
/// view's.
extern "C" fn node_info(env: Env, _: Obj, id: c_int) -> Obj {
    let (view, _, _) = view();
    if view.is_null() {
        return std::ptr::null_mut();
    }
    if id == HOST {
        let info =
            env.call_static(c"android/view/accessibility/AccessibilityNodeInfo", c"obtain", c"(Landroid/view/View;)Landroid/view/accessibility/AccessibilityNodeInfo;", &[Value::obj(view)]).object();
        env.call(view, c"onInitializeAccessibilityNodeInfo", c"(Landroid/view/accessibility/AccessibilityNodeInfo;)V", &[Value::obj(info)]);
        add_children(env, info, view, None);
        return info;
    }
    let Some(node) = TOLD.with_borrow(|told| node_of(told, id).cloned()) else { return std::ptr::null_mut() };
    let info = env
        .call_static(c"android/view/accessibility/AccessibilityNodeInfo", c"obtain", c"(Landroid/view/View;I)Landroid/view/accessibility/AccessibilityNodeInfo;", &[Value::obj(view), Value::int(id)])
        .object();
    describe(env, info, view, id, &node);
    info
}

/// Adds the children of a node (by index; none: the top nodes) to its info.
fn add_children(env: Env, info: Obj, view: Obj, parent: Option<usize>) {
    let children: Vec<i32> = TOLD.with_borrow(|told| {
        let at = |node: &AccessNode| parent.is_none_or(|parent| node.parent == Some(parent));
        let tops = |node: &AccessNode| parent.is_some() || node.parent.is_none();
        told.nodes.iter().filter(|node| at(node) && tops(node) && node.visible).filter_map(|node| told.ids.get(&node.id).copied()).collect()
    });
    for child in children {
        env.call(info, c"addChild", c"(Landroid/view/View;I)V", &[Value::obj(view), Value::int(child)]);
    }
}

/// What a node's info says: what it is like, its text and state, where it is, what can be done
/// to it.
fn describe(env: Env, info: Obj, view: Obj, id: i32, node: &AccessNode) {
    let set = |name: &CStr, on: bool| env.call(info, name, c"(Z)V", &[Value::bool(on)]);
    let text = |name: &CStr, said: &str| env.call(info, name, c"(Ljava/lang/CharSequence;)V", &[Value::obj(env.string(said))]);
    let package = env.call(activity(), c"getPackageName", c"()Ljava/lang/String;", &[]).object();
    env.call(info, c"setPackageName", c"(Ljava/lang/CharSequence;)V", &[Value::obj(package)]);
    text(c"setClassName", class_of(node.role));
    if !node.description.is_empty() {
        text(c"setTooltipText", &node.description);
    }
    let parent = TOLD.with_borrow(|told| node.parent.and_then(|at| told.nodes.get(at)).and_then(|parent| told.ids.get(&parent.id).copied()));
    env.call(info, c"setParent", c"(Landroid/view/View;I)V", &[Value::obj(view), Value::int(parent.unwrap_or(HOST))]);
    let index = TOLD.with_borrow(|told| index_of(told, id));
    add_children(env, info, view, index);
    let [left, top, right, bottom] = on_screen(env, node);
    let bounds = env.new_named(c"android/graphics/Rect", c"(IIII)V", &[Value::int(left), Value::int(top), Value::int(right), Value::int(bottom)]);
    env.call(info, c"setBoundsInScreen", c"(Landroid/graphics/Rect;)V", &[Value::obj(bounds)]);
    match node.role {
        Role::TextBox | Role::PasswordBox | Role::SpinButton => {
            text(c"setText", &node.value);
            text(c"setHintText", &node.name);
            set(c"setEditable", true);
            set(c"setPassword", node.role == Role::PasswordBox);
        }
        Role::ComboBox => {
            text(c"setText", &node.value);
            text(c"setContentDescription", &node.name);
        }
        _ => {
            text(c"setText", &node.name);
        }
    }
    if let Some(on) = node.on.filter(|_| matches!(node.role, Role::CheckBox | Role::Switch | Role::RadioButton | Role::MenuItemRadio)) {
        set(c"setCheckable", true);
        set(c"setChecked", on);
    }
    set(c"setSelected", node.on == Some(true) && matches!(node.role, Role::Tab | Role::Row | Role::TableRow | Role::TreeItem));
    set(c"setHeading", node.role == Role::Heading);
    set(c"setEnabled", node.enabled);
    set(c"setVisibleToUser", node.visible);
    set(c"setFocused", node.focused);
    let reader_focus = TOLD.with_borrow(|told| told.reader_focus);
    set(c"setAccessibilityFocused", reader_focus == Some(id));
    actions(env, info, node, reader_focus == Some(id));
    if node.reads_text() && !node.text().is_empty() {
        text_info(env, info, node);
    }
    let place = TOLD.with_borrow(|told| index.and_then(|at| place_in_collection(&told.nodes, at)));
    place.into_iter().for_each(|place| collection_info(env, info, place));
}

/// What a reader reads text by: the units it moves by (character to paragraph), a text box's
/// cursor, and each letter's place on request.
fn text_info(env: Env, info: Obj, node: &AccessNode) {
    let add = |action: i32| env.call(info, c"addAction", c"(I)V", &[Value::int(action)]);
    env.call(info, c"setMovementGranularities", c"(I)V", &[Value::int(GRANULARITIES.iter().map(|(bit, _)| bit).sum())]);
    add(NEXT_AT_GRANULARITY);
    add(PREVIOUS_AT_GRANULARITY);
    if let Some([anchor, cursor]) = node.caret {
        let [anchor, cursor] = [anchor, cursor].map(|letter| units_before(node.text(), letter) as i32);
        env.call(info, c"setTextSelection", c"(II)V", &[Value::int(anchor), Value::int(cursor)]);
    }
    let key = env.string(LETTER_PLACES);
    let keys = env.call_static(c"java/util/Collections", c"singletonList", c"(Ljava/lang/Object;)Ljava/util/List;", &[Value::obj(key)]).object();
    env.call(info, c"setAvailableExtraData", c"(Ljava/util/List;)V", &[Value::obj(keys)]);
}

/// Where a node is in a table or tree, as Android's collections count: the whole (its rows
/// and columns, whether a tree, whether its rows can be selected, and the rows in view), or one
/// item.
/// (A table's headings are its row 0; its rows follow.)
#[derive(Clone, Copy, Debug, PartialEq)]
enum Collection {
    Whole { rows: i32, columns: i32, tree: bool, selectable: bool, in_view: Option<[u32; 3]> },
    Item { row: i32, column: i32, columns: i32, heading: bool, selected: bool },
}

/// Node `at`'s place in a table or tree, if it is one or in one.
fn place_in_collection(nodes: &[AccessNode], at: usize) -> Option<Collection> {
    let node = &nodes[at];
    let selected = node.on == Some(true);
    let parent = node.parent.filter(|up| matches!(nodes[*up].role, Role::Table | Role::Tree));
    match (node.role, parent) {
        (Role::Table, _) => Some(table_whole(nodes, at)),
        (Role::Tree, _) => Some(Collection::Whole { rows: tree_items(nodes, at).count() as i32, columns: 1, tree: true, selectable: selectable_rows(nodes, at), in_view: None }),
        (Role::TableRow, Some(table)) => Some(Collection::Item { row: index_in_set(node) + heading_rows(nodes, table), column: 0, columns: column_count(nodes, table), heading: false, selected }),
        (Role::ColumnHeader, Some(_)) => Some(Collection::Item { row: 0, column: index_in_set(node), columns: 1, heading: true, selected: false }),
        (Role::TreeItem, Some(tree)) => Some(Collection::Item { row: tree_items(nodes, tree).take_while(|item| *item < at).count() as i32, column: 0, columns: 1, heading: false, selected }),
        (Role::Cell, _) => cell_place(nodes, at),
        _ => None,
    }
}

/// A table as a whole: its rows (its headings' first), columns, and the rows in view.
fn table_whole(nodes: &[AccessNode], table: usize) -> Collection {
    let rows = nodes[table].grid.map_or(0, |[rows, _]| rows);
    let in_view = crate::accessibility::rows_in_view(nodes, table).map(|[from, to]| [from, to, rows]);
    Collection::Whole { rows: rows as i32 + heading_rows(nodes, table), columns: column_count(nodes, table), tree: false, selectable: selectable_rows(nodes, table), in_view }
}

/// A table cell's place: its row (after the headings) and column, selected with its row.
fn cell_place(nodes: &[AccessNode], at: usize) -> Option<Collection> {
    let ([row, column], table) = (nodes[at].grid?, crate::accessibility::table_of(nodes, at)?);
    let selected = nodes[at].parent.is_some_and(|row| nodes[row].on == Some(true));
    Some(Collection::Item { row: row as i32 + heading_rows(nodes, table), column: column as i32, columns: 1, heading: false, selected })
}

/// The nodes directly in node `parent`.
fn children(nodes: &[AccessNode], parent: usize) -> impl Iterator<Item = usize> + '_ {
    (parent + 1..nodes.len()).filter(move |child| nodes[*child].parent == Some(parent))
}

/// A tree's items, by node index.
fn tree_items(nodes: &[AccessNode], tree: usize) -> impl Iterator<Item = usize> + '_ {
    children(nodes, tree).filter(|item| nodes[*item].role == Role::TreeItem)
}

/// The rows a table's headings take: one where it has headings.
fn heading_rows(nodes: &[AccessNode], table: usize) -> i32 {
    i32::from(children(nodes, table).any(|child| nodes[child].role == Role::ColumnHeader))
}

/// Whether a table's rows or a tree's items can be selected.
fn selectable_rows(nodes: &[AccessNode], parent: usize) -> bool {
    children(nodes, parent).any(|child| nodes[child].on.is_some() && matches!(nodes[child].role, Role::TableRow | Role::TreeItem))
}

/// A table's columns (those shown).
fn column_count(nodes: &[AccessNode], table: usize) -> i32 {
    nodes[table].grid.map_or(1, |[_, columns]| columns as i32)
}

/// A node's place in its set, from 0.
fn index_in_set(node: &AccessNode) -> i32 {
    node.position.map_or(0, |[nth, _]| nth as i32 - 1)
}

/// Tells a reader a node's place in its table or tree; a long table scrolls by rows.
fn collection_info(env: Env, info: Obj, place: Collection) {
    let item_info = c"android/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo";
    match place {
        Collection::Whole { rows, columns, tree, selectable, in_view } => {
            let mode = Value::int(if selectable { 2 } else { 0 });
            let args = [Value::int(rows), Value::int(columns), Value::bool(tree), mode];
            let whole = env
                .call_static(c"android/view/accessibility/AccessibilityNodeInfo$CollectionInfo", c"obtain", c"(IIZI)Landroid/view/accessibility/AccessibilityNodeInfo$CollectionInfo;", &args)
                .object();
            env.call(info, c"setCollectionInfo", c"(Landroid/view/accessibility/AccessibilityNodeInfo$CollectionInfo;)V", &[Value::obj(whole)]);
            in_view.into_iter().for_each(|in_view| scrolls(env, info, in_view));
        }
        Collection::Item { row, column, columns, heading, selected } => {
            let args = [Value::int(row), Value::int(1), Value::int(column), Value::int(columns), Value::bool(heading), Value::bool(selected)];
            let item = env.call_static(item_info, c"obtain", c"(IIIIZZ)Landroid/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo;", &args).object();
            env.call(info, c"setCollectionItemInfo", c"(Landroid/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo;)V", &[Value::obj(item)]);
        }
    }
}

/// A long table's scroll actions: forward while rows lie after those in view (`from` to `to` of
/// `rows`), back while some lie before, and to any row.
fn scrolls(env: Env, info: Obj, [from, to, rows]: [u32; 3]) {
    let add = |action: i32| env.call(info, c"addAction", c"(I)V", &[Value::int(action)]);
    let (back, on) = (from > 0, to < rows);
    env.call(info, c"setScrollable", c"(Z)V", &[Value::bool(back || on)]);
    if back {
        add(SCROLL_BACKWARD);
    }
    if on {
        add(SCROLL_FORWARD);
    }
    let to_row =
        env.new_named(c"android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction", c"(ILjava/lang/CharSequence;)V", &[Value::int(SCROLL_TO_POSITION), Value::obj(std::ptr::null_mut())]);
    env.call(info, c"addAction", c"(Landroid/view/accessibility/AccessibilityNodeInfo$AccessibilityAction;)V", &[Value::obj(to_row)]);
}

/// The actions a node takes, and the states that go with them.
fn actions(env: Env, info: Obj, node: &AccessNode, reader_focused: bool) {
    let add = |action: i32| env.call(info, c"addAction", c"(I)V", &[Value::int(action)]);
    let set = |name: &CStr, on: bool| env.call(info, name, c"(Z)V", &[Value::bool(on)]);
    add(if reader_focused { CLEAR_ACCESSIBILITY_FOCUS } else { ACCESSIBILITY_FOCUS });
    let interactive = pressable(node.role) || matches!(node.role, Role::TextBox | Role::PasswordBox | Role::Slider | Role::SpinButton);
    set(c"setFocusable", interactive && node.enabled);
    if interactive && node.enabled {
        add(FOCUS);
    }
    if pressable(node.role) && node.enabled {
        set(c"setClickable", true);
        add(CLICK);
    }
    if let Some(open) = node.expanded {
        add(if open { COLLAPSE } else { EXPAND });
    }
    if node.role == Role::TreeItem {
        add(SELECT);
    }
    if matches!(node.role, Role::TextBox | Role::PasswordBox) && node.enabled {
        add(SET_TEXT);
        add(SET_SELECTION);
    }
    if let Some([value, low, high]) = node.value_range {
        let range = env.call_static(
            c"android/view/accessibility/AccessibilityNodeInfo$RangeInfo",
            c"obtain",
            c"(IFFF)Landroid/view/accessibility/AccessibilityNodeInfo$RangeInfo;",
            &[Value::int(1), Value::float(low), Value::float(high), Value::float(value)],
        );
        env.call(info, c"setRangeInfo", c"(Landroid/view/accessibility/AccessibilityNodeInfo$RangeInfo;)V", &[range]);
    }
    if node.role == Role::Slider && node.enabled {
        add(SCROLL_FORWARD);
        add(SCROLL_BACKWARD);
        let progress =
            env.new_named(c"android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction", c"(ILjava/lang/CharSequence;)V", &[Value::int(SET_PROGRESS), Value::obj(std::ptr::null_mut())]);
        env.call(info, c"addAction", c"(Landroid/view/accessibility/AccessibilityNodeInfo$AccessibilityAction;)V", &[Value::obj(progress)]);
    }
    if !node.visible {
        let show =
            env.new_named(c"android/view/accessibility/AccessibilityNodeInfo$AccessibilityAction", c"(ILjava/lang/CharSequence;)V", &[Value::int(SHOW_ON_SCREEN), Value::obj(std::ptr::null_mut())]);
        env.call(info, c"addAction", c"(Landroid/view/accessibility/AccessibilityNodeInfo$AccessibilityAction;)V", &[Value::obj(show)]);
    }
}

/// A reader acts on a node: what it asked happens to the interface in its next turn.
extern "C" fn perform(env: Env, _: Obj, id: c_int, action: c_int, arguments: Obj) -> u8 {
    let (view, _, _) = view();
    if id == HOST {
        return u8::from(env.call(view, c"performAccessibilityAction", c"(ILandroid/os/Bundle;)Z", &[Value::int(action), Value::obj(arguments)]).as_bool());
    }
    let Some(node) = TOLD.with_borrow(|told| node_of(told, id).cloned()) else { return 0 };
    let known = node.id;
    let deed: Box<dyn FnOnce(&mut Ui)> = match action {
        ACCESSIBILITY_FOCUS => return reader_focus(Some(id)),
        CLEAR_ACCESSIBILITY_FOCUS => return reader_focus(None),
        CLICK => Box::new(move |ui| ui.press_for_reader(known)),
        FOCUS => Box::new(move |ui| ui.set_focus(known)),
        SCROLL_FORWARD | SCROLL_BACKWARD if node.role == Role::Table => return page(id, known, action == SCROLL_FORWARD),
        SCROLL_FORWARD | SCROLL_BACKWARD => Box::new(move |ui| ui.step_for_reader(known, action == SCROLL_FORWARD)),
        SCROLL_TO_POSITION => {
            let row = env.call(arguments, c"getInt", c"(Ljava/lang/String;)I", &[Value::obj(env.string("android.view.accessibility.action.ARGUMENT_ROW_INT"))]).as_int();
            return scroll_to_row(id, known, row);
        }
        NEXT_AT_GRANULARITY | PREVIOUS_AT_GRANULARITY => return move_by_granularity(env, id, &node, arguments, action == NEXT_AT_GRANULARITY),
        EXPAND | COLLAPSE => Box::new(move |ui| ui.open_for_reader(known, action == EXPAND)),
        SELECT => Box::new(move |ui| ui.pick_for_reader(known)),
        SHOW_ON_SCREEN => Box::new(move |ui| ui.show_for_reader(known)),
        SET_TEXT => {
            let text = env.call(arguments, c"getCharSequence", c"(Ljava/lang/String;)Ljava/lang/CharSequence;", &[Value::obj(env.string("ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE"))]).object();
            let text = env.text(text).unwrap_or_default();
            Box::new(move |ui| ui.type_for_reader(known, &text))
        }
        SET_PROGRESS => {
            let key = env.string("android.view.accessibility.action.ARGUMENT_PROGRESS_VALUE");
            let amount = env.call(arguments, c"getFloat", c"(Ljava/lang/String;)F", &[Value::obj(key)]).as_float();
            Box::new(move |ui| ui.set_for_reader(known, amount))
        }
        SET_SELECTION => {
            let int = |name: &str| env.call(arguments, c"getInt", c"(Ljava/lang/String;)I", &[Value::obj(env.string(name))]).as_int().max(0) as u32;
            let range = [int("ACTION_ARGUMENT_SELECTION_START_INT"), int("ACTION_ARGUMENT_SELECTION_END_INT")].map(|unit| letter_at_unit(node.text(), unit));
            Box::new(move |ui| ui.select_for_reader(known, range))
        }
        _ => return 0,
    };
    in_next_turn(deed)
}

/// Has the interface do something for a reader, in its next turn.
fn in_next_turn(deed: impl FnOnce(&mut Ui) + 'static) -> u8 {
    heard(Heard::Other(Box::new(move |ui| {
        deed(ui);
        true
    })));
    kick();
    1
}

/// A reader scrolls a long table a page on, or back: the row a page past those in view (or
/// before them) is shown.
fn page(id: i32, table: Id, forward: bool) -> u8 {
    let row = TOLD.with_borrow(|told| crate::accessibility::row_a_page_away(&told.nodes, index_of(told, id)?, forward));
    row.map_or(0, |row| in_next_turn(move |ui| ui.row_for_reader(table, row, None)))
}

/// A reader asks for a table's row (counting its headings' row first): it is shown.
fn scroll_to_row(id: i32, table: Id, asked: i32) -> u8 {
    let row = TOLD.with_borrow(|told| u32::try_from(asked).ok()?.checked_sub(heading_rows(&told.nodes, index_of(told, id)?) as u32));
    row.map_or(0, |row| in_next_turn(move |ui| ui.row_for_reader(table, row, None)))
}

/// A reader moves through a node's text by a unit (`GRANULARITIES`), forward or back: a text
/// box's cursor moves (selecting, where asked); other text's place (a password box's too) is
/// kept here. Readers hear the unit passed over.
fn move_by_granularity(env: Env, id: i32, node: &AccessNode, arguments: Obj, forward: bool) -> u8 {
    let asked = env.call(arguments, c"getInt", c"(Ljava/lang/String;)I", &[Value::obj(env.string("ACTION_ARGUMENT_MOVEMENT_GRANULARITY_INT"))]).as_int();
    let extend = env.call(arguments, c"getBoolean", c"(Ljava/lang/String;)Z", &[Value::obj(env.string("ACTION_ARGUMENT_EXTEND_SELECTION_BOOLEAN"))]).as_bool();
    let Some(unit) = GRANULARITIES.iter().find(|(bit, _)| *bit == asked).map(|(_, unit)| *unit) else { return 0 };
    let typed = node.role == Role::TextBox;
    let count = node.text().chars().count() as u32;
    let kept = TOLD.with_borrow(|told| told.reading.get(&id).copied());
    let from = if typed { node.caret.map_or(0, |[_, cursor]| cursor) } else { kept.unwrap_or(if forward { 0 } else { count }) };
    let Some([start, end]) = node.unit_range(from, unit, forward) else { return 0 };
    let to = if forward { end } else { start };
    if typed {
        let anchor = if extend { node.caret.map_or(from, |[anchor, _]| anchor) } else { to };
        let known = node.id;
        in_next_turn(move |ui| ui.select_for_reader(known, [anchor, to]));
    } else {
        TOLD.with_borrow_mut(|told| told.reading.insert(id, to));
    }
    traversed(env, id, [start, end].map(|letter| units_before(node.text(), letter) as i32), forward, asked);
    1
}

/// Tells readers a reader moved over UTF-16 units `from` to `to` of a node's text, forward or
/// back, by a granularity.
fn traversed(env: Env, id: i32, [from, to]: [i32; 2], forward: bool, granularity: i32) {
    let action = if forward { NEXT_AT_GRANULARITY } else { PREVIOUS_AT_GRANULARITY };
    send_with(id, TEXT_TRAVERSED, |event| {
        env.call(event, c"setFromIndex", c"(I)V", &[Value::int(from)]);
        env.call(event, c"setToIndex", c"(I)V", &[Value::int(to)]);
        env.call(event, c"setAction", c"(I)V", &[Value::int(action)]);
        env.call(event, c"setMovementGranularity", c"(I)V", &[Value::int(granularity)]);
    });
}

/// A reader asks where a node's letters are (`LETTER_PLACES`): from a UTF-16 unit, so many,
/// each a rectangle on the screen in pixels, or none for one that does not show.
extern "C" fn extra_data(env: Env, _: Obj, id: c_int, info: Obj, key: Obj, arguments: Obj) {
    if env.text(key).as_deref() != Some(LETTER_PLACES) {
        return;
    }
    let Some(node) = TOLD.with_borrow(|told| node_of(told, id).cloned()) else { return };
    let int = |name: &str| env.call(arguments, c"getInt", c"(Ljava/lang/String;I)I", &[Value::obj(env.string(name)), Value::int(-1)]).as_int();
    let start = int("android.view.accessibility.extra.DATA_TEXT_CHARACTER_LOCATION_ARG_START_INDEX");
    let long = int("android.view.accessibility.extra.DATA_TEXT_CHARACTER_LOCATION_ARG_LENGTH").min(MOST_LETTERS);
    let units = node.text().encode_utf16().count() as i32;
    if start < 0 || long <= 0 || start >= units {
        return;
    }
    let end = (start + long).min(units);
    let (view, scale, insets) = view();
    let origin = window_origin(env, view);
    let corner = [origin[0] + insets[0], origin[1] + insets[1]].map(|at| at as f32);
    let places = crate::android_jni::new_object_array(env, c"android/graphics/RectF", end - start);
    for unit in start..end {
        let place = node.letter_rect(letter_at_unit(node.text(), unit as u32));
        let shows = !place.grow(0.5).intersect(node.seen).is_empty();
        let rect = shows.then(|| letter_on_screen(env, place, corner, scale));
        if let Some(rect) = rect {
            crate::android_jni::set_object(env, places, unit - start, rect);
            env.drop_local(rect);
        }
    }
    let extras = env.call(info, c"getExtras", c"()Landroid/os/Bundle;", &[]).object();
    env.call(extras, c"putParcelableArray", c"(Ljava/lang/String;[Landroid/os/Parcelable;)V", &[Value::obj(key), Value::obj(places)]);
}

/// A letter's place on the screen, in pixels, from the window's corner there.
fn letter_on_screen(env: Env, place: Rect, corner: [f32; 2], scale: f32) -> Obj {
    let edges = [place.x * scale + corner[0], place.y * scale + corner[1], place.right() * scale + corner[0], place.bottom() * scale + corner[1]];
    env.new_named(c"android/graphics/RectF", c"(FFFF)V", &edges.map(Value::float))
}

/// The reader's focus moves to a node, or leaves: the reader hears.
fn reader_focus(to: Option<i32>) -> u8 {
    let was = TOLD.with_borrow_mut(|told| std::mem::replace(&mut told.reader_focus, to));
    if let Some(was) = was.filter(|was| Some(*was) != to) {
        send(was, ACCESSIBILITY_FOCUS_CLEARED);
    }
    to.into_iter().for_each(|id| send(id, ACCESSIBILITY_FOCUSED));
    1
}

/// A reader asks which node has a kind of focus.
extern "C" fn find_focus(env: Env, this: Obj, kind: c_int) -> Obj {
    let id = TOLD.with_borrow(|told| if kind == INPUT_FOCUS { told.focused } else { told.reader_focus });
    id.map_or(std::ptr::null_mut(), |id| node_info(env, this, id))
}

/// A finger explores the screen (a reader is on): the node under it is announced as it comes
/// under, the last one left as it goes.
pub(crate) fn hover(at: Point, gone: bool) {
    let under = if gone { None } else { TOLD.with_borrow(|told| node_under(told, at)) };
    let was = TOLD.with_borrow_mut(|told| std::mem::replace(&mut told.hovered, under));
    if was == under {
        return;
    }
    was.into_iter().for_each(|id| send(id, HOVER_EXIT));
    under.into_iter().for_each(|id| send(id, HOVER_ENTER));
}

/// The innermost visible node at a place in the window.
fn node_under(told: &Told, at: Point) -> Option<i32> {
    let inside = |node: &&AccessNode| node.visible && node.seen.contains(at);
    told.nodes.iter().rev().find(inside).and_then(|node| told.ids.get(&node.id).copied())
}

/// Tells readers something happened to a node (or the host view): an event sent up from the
/// view as from that virtual view, with its text.
fn send(id: i32, kind: i32) {
    send_with(id, kind, |_| {});
}

/// The same, the event filled in further by `more`.
fn send_with(id: i32, kind: i32, more: impl FnOnce(Obj)) {
    let Some(env) = crate::android_jni::env() else { return };
    let (view, _, _) = view();
    if view.is_null() {
        return;
    }
    env.frame(|| {
        if id == HOST {
            env.call(view, c"sendAccessibilityEvent", c"(I)V", &[Value::int(kind)]);
            return;
        }
        let event = env.call_static(c"android/view/accessibility/AccessibilityEvent", c"obtain", c"(I)Landroid/view/accessibility/AccessibilityEvent;", &[Value::int(kind)]).object();
        let node = TOLD.with_borrow(|told| node_of(told, id).cloned());
        let package = env.call(activity(), c"getPackageName", c"()Ljava/lang/String;", &[]).object();
        env.call(event, c"setPackageName", c"(Ljava/lang/CharSequence;)V", &[Value::obj(package)]);
        let class = node.as_ref().map_or("android.view.View", |node| class_of(node.role));
        env.call(event, c"setClassName", c"(Ljava/lang/CharSequence;)V", &[Value::obj(env.string(class))]);
        env.call(event, c"setSource", c"(Landroid/view/View;I)V", &[Value::obj(view), Value::int(id)]);
        let said = node.map(|node| if node.name.is_empty() { node.value } else { node.name }).unwrap_or_default();
        let list = env.call(event, c"getText", c"()Ljava/util/List;", &[]).object();
        env.call(list, c"add", c"(Ljava/lang/Object;)Z", &[Value::obj(env.string(&said))]);
        more(event);
        let parent = env.call(view, c"getParent", c"()Landroid/view/ViewParent;", &[]).object();
        env.call(parent, c"requestSendAccessibilityEvent", c"(Landroid/view/View;Landroid/view/accessibility/AccessibilityEvent;)Z", &[Value::obj(view), Value::obj(event)]);
    });
}

/// Says what the app announced (`Ui::announce`).
fn announce(text: &str) {
    let Some(env) = crate::android_jni::env() else { return };
    let (view, _, _) = view();
    env.frame(|| env.call(view, c"announceForAccessibility", c"(Ljava/lang/CharSequence;)V", &[Value::obj(env.string(text))]));
}
