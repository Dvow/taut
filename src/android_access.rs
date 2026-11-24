//! What Android's screen readers (TalkBack, and `uiautomator`) are told: the window's nodes as
//! virtual views of the library's view, through its `AccessibilityNodeProvider` (the class
//! `taut.Nodes`, written by `android_dex`, whose methods are the functions here). Each node
//! is an `AccessibilityNodeInfo` with the class name of the Android widget it is like
//! (`android.widget.Button`, `CheckBox`, `EditText`, `SeekBar`), its text, state, bounds on
//! the screen and the actions it takes. A reader's actions come back as the interface's own
//! (`press_for_reader` and the like). A reader exploring by touch sends hover events, which
//! the window hears (`hover`); the node under the finger is announced.
use crate::accessibility::{AccessNode, Role};
use crate::android::{Heard, activity, heard, kick, view};
use crate::android_jni::{Env, Native, Obj, Value};
use crate::geom::Point;
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
const SCROLL_FORWARD: i32 = 0x1000;
const SCROLL_BACKWARD: i32 = 0x2000;
const SET_SELECTION: i32 = 0x2_0000;
const EXPAND: i32 = 0x4_0000;
const COLLAPSE: i32 = 0x8_0000;
const SET_TEXT: i32 = 0x20_0000;
/// `android.R.id.accessibilityActionShowOnScreen` and `…SetProgress`.
const SHOW_ON_SCREEN: i32 = 0x0102_0036;
const SET_PROGRESS: i32 = 0x0102_003d;
/// Event types (`AccessibilityEvent.TYPE_*`).
const VIEW_FOCUSED: i32 = 8;
const HOVER_ENTER: i32 = 128;
const HOVER_EXIT: i32 = 256;
const CONTENT_CHANGED: i32 = 2048;
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
pub(crate) fn natives() -> [Native; 3] {
    let native = |name: &'static CStr, signature: &'static CStr, function: *const c_void| Native { name: name.as_ptr(), signature: signature.as_ptr(), function };
    [
        native(c"createAccessibilityNodeInfo", c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;", node_info as *const c_void),
        native(c"performAction", c"(IILandroid/os/Bundle;)Z", perform as *const c_void),
        native(c"findFocus", c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;", find_focus as *const c_void),
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
/// went.
pub(crate) fn keep(ui: &Ui) {
    let version = ui.access_version();
    let changed = TOLD.with_borrow_mut(|told| {
        if std::mem::replace(&mut told.version, version) == version {
            return None;
        }
        told.nodes = ui.access_nodes().to_vec();
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
    told.nodes.iter().find(|node| told.ids.get(&node.id) == Some(&id))
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
    let parent = TOLD.with_borrow(|told| node.parent.and_then(|at| told.nodes.get(at)).and_then(|parent| told.ids.get(&parent.id).copied()));
    env.call(info, c"setParent", c"(Landroid/view/View;I)V", &[Value::obj(view), Value::int(parent.unwrap_or(HOST))]);
    let index = TOLD.with_borrow(|told| told.nodes.iter().position(|one| told.ids.get(&one.id) == Some(&id)));
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
        SCROLL_FORWARD | SCROLL_BACKWARD => Box::new(move |ui| ui.step_for_reader(known, action == SCROLL_FORWARD)),
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
            let range = [int("ACTION_ARGUMENT_SELECTION_START_INT"), int("ACTION_ARGUMENT_SELECTION_END_INT")];
            Box::new(move |ui| ui.select_for_reader(known, range))
        }
        _ => return 0,
    };
    heard(Heard::Other(Box::new(move |ui| {
        deed(ui);
        true
    })));
    kick();
    1
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
