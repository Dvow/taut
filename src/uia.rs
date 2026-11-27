//! What screen readers are told, handed to Windows through UI Automation.
//!
//! A client asks the window for its provider (`WM_GETOBJECT`), and that for others: children,
//! the element at a point, the focused one. Each `Provider` is a hand-written COM object: one
//! for the window and one per entry of [`Ui::visible_access_nodes`]. Providers answer from a
//! copy of that list taken after every run, so they need nothing of the window and keep
//! answering ("element not available") once their element or window is gone. Actions (invoke,
//! toggle, set value) reach the window as a message, as mouse and keyboard input do. After
//! every run, Windows gets the events: focus moved, a name, value or state changed, elements
//! came or went.
//!
//! Windows calls providers only on their window's thread, which is made single-threaded
//! apartment on first ask: nothing is shared or locked.
//!
//! A program nobody asks pays nothing: the two libraries load on the first ask, the list is
//! kept from then on, and dropped once no provider is held and Windows says no client listens.
use crate::accessibility::{self, AccessNode, Role};
use crate::geom::{Rect, pt};
use crate::id::Id;
use crate::ui::Ui;
use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::rc::Rc;
use std::sync::OnceLock;

#[allow(clippy::upper_case_acronyms)]
type HANDLE = isize;
/// A `BSTR`: UTF-16, freed by whoever ends up with it.
type Text = *mut u16;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn SendMessageW(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize;
    fn ClientToScreen(hwnd: HANDLE, point: *mut Pixel) -> i32;
    fn SystemParametersInfoW(what: u32, with: u32, value: *mut c_void, how: u32) -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, how: u32) -> i32;
    fn CoUninitialize();
}

/// A point in a window or on the screen, in pixels.
#[repr(C)]
#[derive(Default)]
struct Pixel {
    x: i32,
    y: i32,
}

/// `WM_GETOBJECT` object ids: UI Automation's, and MSAA's client id, which Windows answers from
/// the same provider.
const BY_AUTOMATION: i32 = -25;
const THE_OLDER_WAY: i32 = -4;

/// Whether a screen reader says it runs (`SPI_GETSCREENREADER`).
fn reader_runs() -> bool {
    let mut runs = 0i32;
    // SAFETY: Windows writes one BOOL where pointed to.
    unsafe { SystemParametersInfoW(0x46, 0, (&raw mut runs).cast(), 0) != 0 && runs != 0 }
}

/// The message a provider sends its window: "do what the screen reader asked". (After the one
/// that wakes a window.)
pub(crate) const WM_READER: u32 = 0x8002;

/// `UIA_E_ELEMENTNOTAVAILABLE`, `UIA_E_ELEMENTNOTENABLED`, `UIA_E_INVALIDOPERATION`,
/// `E_NOINTERFACE`, `E_INVALIDARG`.
const GONE: i32 = 0x8004_0201_u32 as i32;
const NOT_NOW: i32 = 0x8004_0200_u32 as i32;
const NOT_DONE: i32 = 0x8013_1509_u32 as i32;
const NO_INTERFACE: i32 = 0x8000_4002_u32 as i32;
const NO_SUCH: i32 = 0x8007_0057_u32 as i32;

/// Property ids: control type, name, has focus, focusable, enabled, automation id.
const CONTROL_TYPE: i32 = 30003;
const NAME: i32 = 30005;
const HAS_KEYBOARD: i32 = 30008;
const TAKES_KEYBOARD: i32 = 30009;
const ENABLED: i32 = 30010;
const AUTOMATION_ID: i32 = 30011;
/// More, which only some elements have: accelerator key, help text and full description (what
/// it says more than its name), is password, position in set and set size, is dialog.
const KEYS: i32 = 30006;
const HELP: i32 = 30013;
const DESCRIPTION: i32 = 30159;
const HIDES_TEXT: i32 = 30019;
const NTH: i32 = 30152;
const OF_SO_MANY: i32 = 30153;
const IS_DIALOG: i32 = 30174;
/// Heading level: 80051 for level 1, 80052 for 2, and so on (80050 for none).
const HEADING_LEVEL: i32 = 30173;
/// Tree item level.
const LEVEL: i32 = 30154;
/// Item status, and ARIA properties (for a sorted column heading, its direction).
const STATUS: i32 = 30026;
const ARIA: i32 = 30102;
/// Grid row and column counts, raised when they change.
const ROW_COUNT: i32 = 30062;
const COLUMN_COUNT: i32 = 30063;
/// Properties raised when they change: value, range value, expand state, selected, toggle
/// state.
const VALUE: i32 = 30045;
const AMOUNT: i32 = 30047;
const UNFOLDED: i32 = 30070;
const IS_SELECTED: i32 = 30079;
const TICKED: i32 = 30086;
/// Event ids: focus changed, selection item selected; structure change kind: children bulk
/// invalidated.
const FOCUS_CHANGED: i32 = 20005;
const ELEMENT_SELECTED: i32 = 20012;
const ALL_INSIDE: i32 = 2;
/// Text events: selection changed (or caret moved), text changed.
const SELECTION_MOVED: i32 = 20014;
const TEXT_CHANGED: i32 = 20015;
/// Text attribute ids handled: read-only, and annotation types (only one known: spelling
/// error).
const READ_ONLY: i32 = 40015;
const MARKS: i32 = 40031;
const SPELT_WRONG: i32 = 60001;
/// Is offscreen: scrolled out of its container or the window.
const OFF_THE_SCREEN: i32 = 30022;

/// A property value, before it becomes a `VARIANT`.
#[derive(Debug, PartialEq)]
enum Value {
    None,
    Number(i32),
    Real(f64),
    Truth(bool),
    Text(String),
}

/// A `VARIANT`: a type tag and the value, in room for the largest.
#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    kind: u16,
    unused: [u16; 3],
    held: Held,
}
#[repr(C)]
#[derive(Clone, Copy)]
union Held {
    number: i32,
    real: f64,
    truth: i16,
    text: Text,
    object: *mut c_void,
    room: [usize; 2],
}

/// The Windows functions only a read-out window needs. Their two libraries have no other use,
/// so they load, and the functions are found by name, the first time a window is asked.
struct Automation {
    /// `UiaReturnRawElementProvider`: answers `WM_GETOBJECT` with a provider.
    give: unsafe extern "system" fn(HANDLE, usize, isize, *mut Face) -> isize,
    /// `UiaHostProviderFromHwnd`: Windows' own provider for a window (title, frame, place among
    /// windows).
    host: unsafe extern "system" fn(HANDLE, *mut *mut Face) -> i32,
    /// `UiaRaiseAutomationEvent`, `UiaRaiseAutomationPropertyChangedEvent`,
    /// `UiaRaiseStructureChangedEvent`.
    raise: unsafe extern "system" fn(*mut Face, i32) -> i32,
    raise_change: unsafe extern "system" fn(*mut Face, i32, Variant, Variant) -> i32,
    raise_structure: unsafe extern "system" fn(*mut Face, i32, *const i32, i32) -> i32,
    /// `UiaClientsAreListening`.
    listening: unsafe extern "system" fn() -> i32,
    /// `UiaRaiseNotificationEvent` (Windows 10 1709 on; where missing, nothing is said).
    say: Option<unsafe extern "system" fn(*mut Face, i32, i32, Text, Text) -> i32>,
    /// `UiaDisconnectProvider`.
    disconnect: unsafe extern "system" fn(*mut Face) -> i32,
    /// `SysAllocStringLen`, `SysFreeString`.
    text: unsafe extern "system" fn(*const u16, u32) -> Text,
    free_text: unsafe extern "system" fn(Text),
    /// `SafeArrayCreateVector`, `SafeArrayPutElement`; `SafeArrayGetUBound`,
    /// `SafeArrayGetElement`.
    numbers: unsafe extern "system" fn(u16, i32, u32) -> *mut c_void,
    put: unsafe extern "system" fn(*mut c_void, *const i32, *const c_void) -> i32,
    last: unsafe extern "system" fn(*mut c_void, u32, *mut i32) -> i32,
    get: unsafe extern "system" fn(*mut c_void, *const i32, *mut c_void) -> i32,
    /// `UiaGetReservedNotSupportedValue` (an unsupported text attribute) and
    /// `UiaGetReservedMixedAttributeValue` (mixed).
    unknown: unsafe extern "system" fn(*mut *mut c_void) -> i32,
    mixed: unsafe extern "system" fn(*mut *mut c_void) -> i32,
}

static AUTOMATION: OnceLock<Option<Automation>> = OnceLock::new();

/// The functions, loading their libraries on first use. Nothing where any is missing.
fn automation() -> Option<&'static Automation> {
    AUTOMATION.get_or_init(Automation::load).as_ref()
}

impl Automation {
    fn load() -> Option<Automation> {
        // SAFETY: the names are NUL-terminated, and each function has the signature Windows
        // documents for its name. The libraries stay loaded for the program's life, and so do
        // the functions.
        unsafe {
            let (core, ole) = (library("uiautomationcore.dll")?, library("oleaut32.dll")?);
            Some(Automation {
                give: found(core, c"UiaReturnRawElementProvider")?,
                host: found(core, c"UiaHostProviderFromHwnd")?,
                raise: found(core, c"UiaRaiseAutomationEvent")?,
                raise_change: found(core, c"UiaRaiseAutomationPropertyChangedEvent")?,
                raise_structure: found(core, c"UiaRaiseStructureChangedEvent")?,
                listening: found(core, c"UiaClientsAreListening")?,
                say: found(core, c"UiaRaiseNotificationEvent"),
                disconnect: found(core, c"UiaDisconnectProvider")?,
                text: found(ole, c"SysAllocStringLen")?,
                free_text: found(ole, c"SysFreeString")?,
                numbers: found(ole, c"SafeArrayCreateVector")?,
                put: found(ole, c"SafeArrayPutElement")?,
                last: found(ole, c"SafeArrayGetUBound")?,
                get: found(ole, c"SafeArrayGetElement")?,
                unknown: found(core, c"UiaGetReservedNotSupportedValue")?,
                mixed: found(core, c"UiaGetReservedMixedAttributeValue")?,
            })
        }
    }

    /// A `BSTR`, freed by whoever it is handed to.
    fn text(&self, text: &str) -> Text {
        let units: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: Windows copies as many units as told.
        unsafe { (self.text)(units.as_ptr(), units.len() as u32) }
    }

    /// A `VARIANT`. (A `BSTR` in it is Windows' to free when the value is an answer, this
    /// library's when only shown to Windows.)
    fn variant(&self, value: &Value) -> Variant {
        let mut held = Held { room: [0; 2] };
        let kind = match value {
            Value::None => 0,
            Value::Number(number) => (held.number = *number, 3).1,
            Value::Real(real) => (held.real = *real, 5).1,
            Value::Truth(truth) => (held.truth = -(*truth as i16), 11).1,
            Value::Text(text) => (held.text = self.text(text), 8).1,
        };
        Variant { kind, unused: [0; 3], held }
    }
}

/// A system library, loaded only from the system folder.
fn library(name: &str) -> Option<HANDLE> {
    let library = crate::windows::system_library(name);
    (library != 0).then_some(library)
}

/// A function of a loaded library, by name.
///
/// # Safety
/// `F` is the function type the library has under that name.
unsafe fn found<F: Copy>(library: HANDLE, name: &CStr) -> Option<F> {
    let at = crate::windows::function_in(library, name);
    // SAFETY: a function pointer is an address the size of `F`, as the caller promises.
    (at != 0 && size_of::<F>() == size_of::<usize>()).then(|| unsafe { std::mem::transmute_copy(&at) })
}

/// What Windows was last told of one window. Kept by the window's thread while the window
/// lives, and by every provider for it while held.
struct WindowTree {
    automation: &'static Automation,
    thread: u32,
    /// The window; 0 once it is gone.
    hwnd: Cell<HANDLE>,
    /// Pixels per unit, and whether it has focus.
    scale: Cell<f32>,
    keyboard: Cell<bool>,
    /// Whether nothing has been told of it yet; and its elements as of the last run, and which
    /// list they were (`Ui::access_version`).
    fresh: Cell<bool>,
    nodes: RefCell<Vec<AccessNode>>,
    turn: Cell<u64>,
    /// The providers handed out for it that are still held.
    out: RefCell<Vec<*mut Provider>>,
    /// What a provider asks the window to do, until done.
    asked: RefCell<Option<Ask>>,
}

/// An action a screen reader asks for.
#[derive(Debug, PartialEq)]
enum Ask {
    Press(Id),
    Set(Id, f32),
    Type(Id, String),
    Focus(Id),
    /// Select the text between two characters of a text box.
    Select(Id, [u32; 2]),
    /// Expand a tree item (true), or collapse it.
    Open(Id, bool),
    /// Add a tree item to the selection, or remove it.
    Pick(Id),
    /// Scroll its container so it shows.
    Show(Id),
    /// Show a table's row (by its table and index), and select it: alone, or toggled.
    Row(Id, u32, Option<bool>),
}

/// The thread's windows that were asked for their elements, and whether the interface keeps its
/// list because of that (not because the app asked).
#[derive(Default)]
struct Reading {
    windows: Vec<Rc<WindowTree>>,
    ours: bool,
}

thread_local! {
    static READING: RefCell<Reading> = RefCell::new(Reading::default());
}

impl WindowTree {
    /// The window's client corner on screen (for a gone window, the screen's).
    fn corner(&self) -> Pixel {
        let mut corner = Pixel::default();
        // SAFETY: maps a point of a window to the screen; for a gone window the call just
        // fails.
        unsafe { ClientToScreen(self.hwnd.get(), &mut corner) };
        corner
    }
}

/// What Windows was told of this window, if it ever asked.
fn window_tree(hwnd: HANDLE) -> Option<Rc<WindowTree>> {
    READING.with_borrow(|reading| reading.windows.iter().find(|tree| tree.hwnd.get() == hwnd).cloned())
}

/// Answers `WM_GETOBJECT`: the window's provider goes to Windows. On a window's first ask,
/// `set_screen_reader` makes the interface keep its list and run, which tells of the window
/// (`listening`: the list is kept already). Nothing where Windows lacks UI Automation, or where
/// the thread's COM setup would have providers called from other threads: Windows answers by
/// itself then.
///
/// And nothing for an MSAA ask with no screen reader running: Windows asks that way now and
/// then with no reader anywhere (the caret, for the touch keyboard). So a window nobody reads
/// loads no library and keeps no list.
pub(crate) fn give(hwnd: HANDLE, w: usize, l: isize, listening: bool, listen: impl FnOnce()) -> Option<isize> {
    // (The object id is the low 32 bits.)
    let meant = match l as i32 {
        BY_AUTOMATION => true,
        THE_OLDER_WAY => listening || reader_runs(),
        _ => false,
    };
    let automation = meant.then(automation)??;
    let known = window_tree(hwnd);
    let tree = match &known {
        Some(tree) => tree.clone(),
        None => register(hwnd, automation)?,
    };
    // (The provider exists before the run: a run ends by asking whether anybody is left, and
    // somebody is.)
    let root = Provider::of(&tree, 0, THING);
    if known.is_none() || !listening {
        READING.with_borrow_mut(|reading| reading.ours |= !listening);
        listen();
    }
    // SAFETY: Windows gets the window's provider by its first face and holds it as long as it
    // wants; this function's reference ends here.
    unsafe {
        let answer = (automation.give)(hwnd, w, l, root);
        Provider::release(root);
        Some(answer)
    }
}

/// Starts keeping what Windows is told of a window. With a thread's first window, the thread
/// becomes a single-threaded apartment (2), so Windows calls providers there. Nothing where the
/// thread is set up otherwise already.
fn register(hwnd: HANDLE, automation: &'static Automation) -> Option<Rc<WindowTree>> {
    let first = READING.with_borrow(|reading| reading.windows.is_empty());
    // SAFETY: initializes COM on this thread; uninitialized when the last asked window is gone
    // (`gone`).
    if first && unsafe { CoInitializeEx(std::ptr::null_mut(), 2) } < 0 {
        return None;
    }
    // SAFETY: asks the current thread's id.
    let thread = unsafe { GetCurrentThreadId() };
    let tree = Rc::new(WindowTree {
        automation,
        thread,
        hwnd: Cell::new(hwnd),
        scale: Cell::new(1.0),
        keyboard: Cell::new(false),
        fresh: Cell::new(true),
        nodes: RefCell::default(),
        turn: Cell::new(0),
        out: RefCell::default(),
        asked: RefCell::default(),
    });
    READING.with_borrow_mut(|reading| reading.windows.push(tree.clone()));
    Some(tree)
}

/// After a run: Windows' copy of the window's elements is updated, and listeners hear what
/// changed: focus moved, a name, value or state changed, elements came or went. Nothing for a
/// window never asked.
pub(crate) fn tell(hwnd: HANDLE, ui: &Ui) {
    if !ui.listening {
        return;
    }
    let Some(tree) = window_tree(hwnd) else { return };
    let (news, keyboard) = (ui.access_nodes(), ui.window_focused);
    let had_keyboard = tree.keyboard.replace(keyboard);
    tree.scale.set(ui.scale());
    let mut was = tree.nodes.borrow_mut();
    // (A first telling changes nothing anybody could have known. And the copy is made only
    // where it differs: copying a long page is most of the cost. A list of the same turn is
    // not even compared.)
    let (turn, from) = (ui.access_version(), ui.access_moved_from());
    // (Moved only, as by a scroll: what shows is all that changed.)
    let moved = from == Some(tree.turn.get()) && was.len() == news.len() && had_keyboard == keyboard && !tree.fresh.get();
    let kept = tree.turn.replace(turn) == turn && was.len() == news.len();
    if moved {
        let shows = |node: &AccessNode| Value::Truth(!node.visible);
        let flips = was.iter().zip(news).filter(|(was, now)| was.visible != now.visible);
        let changes: Vec<Change> = flips.map(|(was, now)| Change::Property(now.id, OFF_THE_SCREEN, shows(was), shows(now))).collect();
        accessibility::follow_move(&mut was, news);
        drop(was);
        // SAFETY: a call without arguments.
        if !changes.is_empty() && unsafe { (tree.automation.listening)() } != 0 {
            changes.into_iter().for_each(|change| raise(&tree, change));
        }
        return;
    }
    let (fresh, unchanged) = (tree.fresh.replace(false), kept || *was == news);
    let same = fresh || (unchanged && had_keyboard == keyboard);
    let changes = if same { Vec::new() } else { changes(&was, had_keyboard, news, keyboard) };
    if !unchanged {
        was.clear();
        was.extend_from_slice(news);
    }
    drop(was);
    // SAFETY: a call without arguments. (Nobody listens: nobody is told.)
    if changes.is_empty() || unsafe { (tree.automation.listening)() } == 0 {
        return;
    }
    changes.into_iter().for_each(|change| raise(&tree, change));
}

/// After a run: listeners say what the app announced, as a notification of the window (4: other
/// kind; 2: all, queued). Returns Windows' answer to the last one: 0 on success.
pub(crate) fn announce(hwnd: HANDLE, ui: &Ui) -> i32 {
    let Some((tree, say)) = window_tree(hwnd).and_then(|tree| Some((tree.clone(), tree.automation.say?))) else { return 0 };
    let mut answer = 0;
    for words in ui.announced() {
        let (about, [spoken, by]) = (Provider::of(&tree, 0, THING), [words.as_str(), "taut"].map(|text| tree.automation.text(text)));
        // SAFETY: Windows is shown the window's provider, held meanwhile, and two `BSTR`s this
        // function frees after.
        unsafe {
            answer = say(about, 4, 2, spoken, by);
            [spoken, by].into_iter().for_each(|text| (tree.automation.free_text)(text));
            Provider::release(about);
        }
    }
    answer
}

/// An event for screen readers.
#[derive(Debug, PartialEq)]
enum Change {
    /// An element's children changed (0: the window's).
    Children(Id),
    /// A property changed: which, old value, new value.
    Property(Id, i32, Value, Value),
    /// One of several was selected.
    Picked(Id),
    /// Focus went to an element (0: back to the window).
    Focus(Id),
    /// A text box's text, or its selection, changed.
    Text(Id),
    Caret(Id),
}

/// What changed between what Windows was told of a window (`was`) and what it holds `now`.
fn changes(was: &[AccessNode], had_keyboard: bool, now: &[AccessNode], keyboard: bool) -> Vec<Change> {
    let mut changes = Vec::new();
    // Each element (id and role) with its parent (0: the window): equal lists mean nothing came
    // or went.
    let lineage = |list: &[AccessNode]| list.iter().map(|node| (node.parent.map_or(0, |group| list[group].id), (node.id, node.role))).collect::<Vec<_>>();
    let (before, after) = (lineage(was), lineage(now));
    let inside = |lineage: &[(Id, (Id, Role))], group: Id| lineage.iter().filter(|(within, _)| *within == group).map(|(_, thing)| *thing).collect::<Vec<_>>();
    // (A new group is reported by its parent.)
    let groups = now.iter().filter(|node| node.role.is_container() && before.iter().any(|(_, thing)| *thing == (node.id, node.role))).map(|node| node.id);
    let altered = std::iter::once(0).chain(groups).filter(|group| before != after && inside(&before, *group) != inside(&after, *group));
    changes.extend(altered.map(Change::Children));
    for (at, node) in now.iter().enumerate() {
        let same = |old: &&AccessNode| (old.id, old.role) == (node.id, node.role);
        let old = was.get(at).filter(same).or_else(|| was.iter().find(same));
        if let Some(old) = old.filter(|old| *old != node) {
            compare(old, node, &mut changes);
        }
    }
    let on = |list: &[AccessNode], keyboard: bool| list.iter().find(|node| node.focused && keyboard).map(|node| node.id);
    match (on(was, had_keyboard), on(now, keyboard)) {
        (before, Some(id)) if before != Some(id) => changes.push(Change::Focus(id)),
        (Some(_), None) if keyboard => changes.push(Change::Focus(0)),
        _ => {}
    }
    changes
}

/// What of one element differs from what Windows was told.
fn compare(old: &AccessNode, new: &AccessNode, changes: &mut Vec<Change>) {
    let text = |text: &String| Value::Text(text.clone());
    let amount = |node: &AccessNode| node.value_range.map_or(Value::None, |amount| Value::Real(amount[0] as f64));
    let on = |node: &AccessNode| node.on == Some(true);
    let counts = |node: &AccessNode, which: usize| node.grid.filter(|_| node.role == Role::Table).map_or(Value::None, |grid| Value::Number(grid[which] as i32));
    // (A tree item's expand state, which it has besides selection.)
    let opened = |node: &AccessNode| node.expanded.filter(|_| node.role == Role::TreeItem).map_or(Value::None, |open| Value::Number(open as i32));
    // State is reported as the property its role has for it.
    let (state, stands): (i32, fn(bool) -> Value) = match new.role {
        Role::CheckBox | Role::Switch | Role::MenuItem | Role::Button => (TICKED, |on| Value::Number(on as i32)),
        Role::Fold | Role::ComboBox | Role::Submenu => (UNFOLDED, |on| Value::Number(on as i32)),
        _ => (IS_SELECTED, Value::Truth),
    };
    let properties = [
        (NAME, text(&old.name), text(&new.name)),
        (VALUE, text(&old.value), text(&new.value)),
        (AMOUNT, amount(old), amount(new)),
        (state, stands(on(old)), stands(on(new))),
        (UNFOLDED, opened(old), opened(new)),
        (ENABLED, Value::Truth(old.enabled), Value::Truth(new.enabled)),
        (STATUS, sort_status(old), sort_status(new)),
        (ROW_COUNT, counts(old, 0), counts(new, 0)),
        (OFF_THE_SCREEN, Value::Truth(!old.visible), Value::Truth(!new.visible)),
        (COLUMN_COUNT, counts(old, 1), counts(new, 1)),
    ];
    let id = new.id;
    changes.extend(properties.into_iter().filter(|(_, was, is)| was != is).map(|(property, was, is)| Change::Property(id, property, was, is)));
    if matches!(new.role, Role::RadioButton | Role::Tab | Role::Row | Role::TableRow | Role::MenuItemRadio | Role::TreeItem) && on(new) && !on(old) {
        changes.push(Change::Picked(id));
    }
    // A text box is read through the text pattern: its text, and its caret; other text, by its
    // selection (a document's).
    if matches!(new.role, Role::TextBox | Role::PasswordBox) {
        changes.extend((old.value != new.value).then_some(Change::Text(id)));
    }
    if new.reads_text() {
        changes.extend((old.caret != new.caret).then_some(Change::Caret(id)));
    }
}

/// For the column heading the rows are sorted by, the direction in words a screen reader says;
/// nothing for anything else.
fn sort_status(node: &AccessNode) -> Value {
    node.sort_word().map_or(Value::None, |way| Value::Text(format!("sorted {way}")))
}

/// Raises one change to listeners.
fn raise(tree: &Rc<WindowTree>, change: Change) {
    let automation = tree.automation;
    let (Change::Children(id) | Change::Property(id, ..) | Change::Picked(id) | Change::Focus(id) | Change::Text(id) | Change::Caret(id)) = &change;
    let about = Provider::of(tree, *id, THING);
    // SAFETY: Windows is shown the provider concerned, held meanwhile, and for a property the
    // old and new values, which this function frees (`VT_BSTR` ones, 8). Then the reference
    // ends. (Whether anyone listened is Windows' affair.)
    unsafe {
        match &change {
            Change::Children(_) => (automation.raise_structure)(about, ALL_INSIDE, std::ptr::null(), 0),
            Change::Picked(_) => (automation.raise)(about, ELEMENT_SELECTED),
            Change::Focus(_) => (automation.raise)(about, FOCUS_CHANGED),
            Change::Text(_) => (automation.raise)(about, TEXT_CHANGED),
            Change::Caret(_) => (automation.raise)(about, SELECTION_MOVED),
            Change::Property(_, property, was, is) => {
                let (was, is) = (automation.variant(was), automation.variant(is));
                let result = (automation.raise_change)(about, *property, was, is);
                [was, is].into_iter().filter(|value| value.kind == 8).for_each(|value| (automation.free_text)(value.held.text));
                result
            }
        };
        Provider::release(about);
    }
}

/// Does to a window's interface what a provider asked for (the window got `WM_READER`); then
/// the interface runs.
pub(crate) fn work(hwnd: HANDLE, ui: &mut Ui) {
    match window_tree(hwnd).and_then(|tree| tree.asked.take()) {
        Some(Ask::Press(id)) => ui.press_for_reader(id),
        Some(Ask::Set(id, to)) => ui.set_for_reader(id, to),
        Some(Ask::Type(id, text)) => ui.type_for_reader(id, &text),
        Some(Ask::Focus(id)) => ui.set_focus(id),
        Some(Ask::Select(id, ends)) => ui.select_for_reader(id, ends),
        Some(Ask::Open(id, open)) => ui.open_for_reader(id, open),
        Some(Ask::Pick(id)) => ui.pick_for_reader(id),
        Some(Ask::Show(id)) => ui.show_for_reader(id),
        Some(Ask::Row(table, row, pick)) => ui.row_for_reader(table, row, pick),
        None => {}
    }
}

impl Reading {
    /// Whether nobody still wants the list kept for Windows: it is kept because Windows asked,
    /// no provider is held, and no client `listens`. Then the windows stop keeping it, and the
    /// copies go.
    fn left(&mut self, listens: impl FnOnce() -> bool) -> bool {
        let left = self.ours && self.windows.iter().all(|tree| tree.out.borrow().is_empty()) && !listens();
        if left {
            self.windows.iter().for_each(|tree| (*tree.nodes.borrow_mut(), _) = (Vec::new(), tree.fresh.set(true)));
            self.ours = false;
        }
        left
    }
}

/// After a run: whether nobody still wants the list kept for Windows. Windows says whether any
/// client listens (asked only where this thread's windows were asked before: its libraries are
/// loaded then).
pub(crate) fn nobody_left() -> bool {
    // SAFETY: a call without arguments.
    READING.with_borrow_mut(|reading| reading.left(|| automation().is_some_and(|automation| unsafe { (automation.listening)() } != 0)))
}

/// A window is being destroyed: Windows releases what it holds of it, and any provider still
/// held knows its window is gone.
pub(crate) fn gone(hwnd: HANDLE) {
    let Some(tree) = window_tree(hwnd) else { return };
    let last = READING.with_borrow_mut(|reading| {
        reading.windows.retain(|other| !Rc::ptr_eq(other, &tree));
        reading.windows.is_empty()
    });
    // SAFETY: each provider out is alive, and all are referenced here before Windows releases
    // the first. (That call lets others in, and one may release the last reference to a
    // provider further down: referenced, it stays.)
    let out: Vec<*mut Face> = tree.out.borrow().iter().map(|provider| unsafe { (**provider).held(THING) }).collect();
    // SAFETY: Windows disconnects each while it still answers for its window, and the reference
    // ends. Then Windows is told the window has no providers (handing it none says so); and COM
    // is uninitialized with the last asked window.
    unsafe {
        for face in out {
            (tree.automation.disconnect)(face);
            Provider::release(face);
        }
        (tree.automation.give)(hwnd, 0, 0, std::ptr::null_mut());
        tree.hwnd.set(0);
        *tree.nodes.borrow_mut() = Vec::new();
        if last {
            CoUninitialize();
        }
    }
}

/// One of a provider's interface pointers. COM knows an object by a pointer to a vtable
/// pointer, and one with several interfaces by several. Each sits beside a pointer back to the
/// object, so whichever one a call comes through, the object is found.
#[repr(C)]
struct Face {
    table: *const c_void,
    of: *const Provider,
}

/// The provider's faces, by interface: `IRawElementProviderSimple`,
/// `IRawElementProviderFragment`, `IRawElementProviderFragmentRoot`, `IInvokeProvider`,
/// `IToggleProvider`, `ISelectionItemProvider`, `IValueProvider`, `IRangeValueProvider`,
/// `IExpandCollapseProvider`, `ITextProvider`; for tables `IGridProvider`, `ITableProvider`,
/// and for cells `IGridItemProvider`, `ITableItemProvider`; for trees `ISelectionProvider`; and
/// for anything `IScrollItemProvider`.
const THING: usize = 0;
const PLACE: usize = 1;
const WINDOW: usize = 2;
const PRESS: usize = 3;
const TICK: usize = 4;
const CHOICE: usize = 5;
const TYPED: usize = 6;
const SLIDE: usize = 7;
const FOLD: usize = 8;
const TEXT: usize = 9;
const GRID: usize = 10;
const IN_GRID: usize = 11;
const HEADED: usize = 12;
const UNDER_HEADING: usize = 13;
const SELECTION: usize = 14;
const SCROLL_ITEM: usize = 15;
const FACES: usize = 16;

/// Those interfaces' IIDs, in order; and last `IUnknown`'s.
const NAMES: [[u32; 4]; FACES + 1] = [
    [0xD6DD_68D1, 0x4332_86FD, 0xBE9A_6686, 0x4CD2_A2DE],
    [0xF706_3DA8, 0x439C_8359, 0xC5BB_9792, 0x877D_9A29],
    [0x620C_E2A5, 0x40A9_AB8F, 0x3CDE_CB86, 0x589B_5975],
    [0x54FC_B24B, 0x47A2_E18E, 0xCBEC_D3B4, 0xA299_75E7],
    [0x56D0_0BD0, 0x433C_C4F4, 0x521A_36A8, 0x9208_7EA5],
    [0x2ACA_D808, 0x452D_B2D4, 0xFF91_07A4, 0xB267_D11A],
    [0xC793_5180, 0x4201_6FB3, 0xF77D_74B1, 0x4AF6_DB3A],
    [0x36DC_7AEF, 0x4691_33E6, 0xE72B_E1AF, 0x333D_4B27],
    [0xD847_D3A5, 0x4A98_CAB0, 0xB4EC_328C, 0x24AD_595C],
    [0x3589_C92C, 0x4367_63F3, 0xA6AD_BB99, 0xF27C_B753],
    [0xB17D_6187, 0x464B_0907, 0xF10E_68A1, 0xB172_157A],
    [0xD025_41F1, 0x4D64_FB81, 0x20F5_32AE, 0xD1DB_A6F8],
    [0x9C86_0395, 0x490A_97B3, 0x8C85_2AB5, 0x66F1_2AC2],
    [0xB973_4FA6, 0x4D78_771F, 0x1725_909C, 0xCD49_9399],
    [0xFB8B_03AF, 0x48D4_3BDF, 0x651A_36BD, 0x68E1_3B79],
    [0x2360_C714, 0x4B26_4BF1, 0x219B_65BA, 0xEB27_6131],
    [0, 0, 0xC0, 0x4600_0000],
];
/// `ITextRangeProvider`'s IID.
const A_STRETCH: [u32; 4] = [0x5347_AD7B, 0x46F8_C355, 0x9090_F5AF, 0x632F_5833];

/// The vtable function types: an action; an answer written through the last pointer; and an
/// answer about something named by a number.
type Do = unsafe extern "system" fn(*mut Face) -> i32;
type Get<T> = unsafe extern "system" fn(*mut Face, *mut T) -> i32;
type GetBy<T> = unsafe extern "system" fn(*mut Face, i32, *mut T) -> i32;

/// `IUnknown`: `QueryInterface`, `AddRef`, `Release`.
#[repr(C)]
struct Unknown {
    query: unsafe extern "system" fn(*mut Face, *const [u32; 4], *mut *mut Face) -> i32,
    hold: unsafe extern "system" fn(*mut Face) -> u32,
    release: unsafe extern "system" fn(*mut Face) -> u32,
}
const UNKNOWN: Unknown = Unknown { query: Provider::query, hold: Provider::hold, release: Provider::release };

/// Each interface's vtable, in the order Windows declares it.
#[repr(C)]
struct ThingTable {
    unknown: Unknown,
    options: Get<i32>,
    pattern: GetBy<*mut Face>,
    property: GetBy<Variant>,
    host: Get<*mut Face>,
}
#[repr(C)]
struct PlaceTable {
    unknown: Unknown,
    navigate: GetBy<*mut Face>,
    runtime_id: Get<*mut c_void>,
    bounds: Get<[f64; 4]>,
    roots: Get<*mut c_void>,
    focus: Do,
    root: Get<*mut Face>,
}
#[repr(C)]
struct WindowTable {
    unknown: Unknown,
    at_point: unsafe extern "system" fn(*mut Face, f64, f64, *mut *mut Face) -> i32,
    focused: Get<*mut Face>,
}
#[repr(C)]
struct PressTable {
    unknown: Unknown,
    press: Do,
}
#[repr(C)]
struct TickTable {
    unknown: Unknown,
    tick: Do,
    ticked: Get<i32>,
}
#[repr(C)]
struct ChoiceTable {
    unknown: Unknown,
    pick: Do,
    add: Do,
    remove: Do,
    picked: Get<i32>,
    among: Get<*mut Face>,
}
#[repr(C)]
struct TypedTable {
    unknown: Unknown,
    put: unsafe extern "system" fn(*mut Face, *const u16) -> i32,
    value: Get<Text>,
    read_only: Get<i32>,
}
#[repr(C)]
struct SlideTable {
    unknown: Unknown,
    put: unsafe extern "system" fn(*mut Face, f64) -> i32,
    value: Get<f64>,
    read_only: Get<i32>,
    most: Get<f64>,
    least: Get<f64>,
    large_step: Get<f64>,
    small_step: Get<f64>,
}
#[repr(C)]
struct FoldTable {
    unknown: Unknown,
    unfold: Do,
    fold: Do,
    unfolded: Get<i32>,
}
#[repr(C)]
struct GridTable {
    unknown: Unknown,
    item: unsafe extern "system" fn(*mut Face, i32, i32, *mut *mut Face) -> i32,
    rows: Get<i32>,
    columns: Get<i32>,
}
#[repr(C)]
struct InGridTable {
    unknown: Unknown,
    row: Get<i32>,
    column: Get<i32>,
    rows_spanned: Get<i32>,
    columns_spanned: Get<i32>,
    grid: Get<*mut Face>,
}
#[repr(C)]
struct HeadedTable {
    unknown: Unknown,
    row_headings: Get<*mut c_void>,
    column_headings: Get<*mut c_void>,
    by_rows: Get<i32>,
}
#[repr(C)]
struct UnderHeadingTable {
    unknown: Unknown,
    row_headings: Get<*mut c_void>,
    column_headings: Get<*mut c_void>,
}
#[repr(C)]
struct ScrollItemTable {
    unknown: Unknown,
    show: Do,
}
#[repr(C)]
struct SelectionTable {
    unknown: Unknown,
    selection: Get<*mut c_void>,
    many: Get<i32>,
    required: Get<i32>,
}
#[repr(C)]
struct TextTable {
    unknown: Unknown,
    selection: Get<*mut c_void>,
    in_view: Get<*mut c_void>,
    of_child: unsafe extern "system" fn(*mut Face, *mut Face, *mut *mut Stretch) -> i32,
    at_spot: unsafe extern "system" fn(*mut Face, Spot, *mut *mut Stretch) -> i32,
    all: Get<*mut Stretch>,
    selects: Get<i32>,
}
/// A screen point, passed by value (`UiaPoint`).
#[repr(C)]
#[derive(Clone, Copy)]
struct Spot {
    x: f64,
    y: f64,
}

static AS_THING: ThingTable = ThingTable { unknown: UNKNOWN, options: Provider::options, pattern: Provider::pattern, property: Provider::property, host: Provider::host };
static AS_PLACE: PlaceTable =
    PlaceTable { unknown: UNKNOWN, navigate: Provider::navigate, runtime_id: Provider::runtime_id, bounds: Provider::bounds, roots: Provider::roots, focus: Provider::focus, root: Provider::root };
static AS_WINDOW: WindowTable = WindowTable { unknown: UNKNOWN, at_point: Provider::at_point, focused: Provider::focused };
static AS_PRESS: PressTable = PressTable { unknown: UNKNOWN, press: Provider::press };
static AS_TICK: TickTable = TickTable { unknown: UNKNOWN, tick: Provider::press, ticked: Provider::on };
static AS_CHOICE: ChoiceTable = ChoiceTable { unknown: UNKNOWN, pick: Provider::pick, add: Provider::add_pick, remove: Provider::unpick, picked: Provider::on, among: Provider::among };
static AS_TYPED: TypedTable = TypedTable { unknown: UNKNOWN, put: Provider::type_in, value: Provider::typed, read_only: Provider::read_only };
static AS_SLIDE: SlideTable = SlideTable {
    unknown: UNKNOWN,
    put: Provider::slide,
    value: Provider::amount::<0>,
    read_only: Provider::read_only,
    most: Provider::amount::<2>,
    least: Provider::amount::<1>,
    large_step: Provider::step,
    small_step: Provider::step,
};
static AS_FOLD: FoldTable = FoldTable { unknown: UNKNOWN, unfold: Provider::unfold, fold: Provider::fold, unfolded: Provider::unfolded };
static AS_GRID: GridTable = GridTable { unknown: UNKNOWN, item: Provider::item, rows: Provider::grid_number::<0>, columns: Provider::grid_number::<1> };
static AS_IN_GRID: InGridTable =
    InGridTable { unknown: UNKNOWN, row: Provider::grid_number::<0>, column: Provider::grid_number::<1>, rows_spanned: Provider::one, columns_spanned: Provider::one, grid: Provider::grid_of };
static AS_HEADED: HeadedTable = HeadedTable { unknown: UNKNOWN, row_headings: Provider::no_headings, column_headings: Provider::headings, by_rows: Provider::by_rows };
static AS_UNDER_HEADING: UnderHeadingTable = UnderHeadingTable { unknown: UNKNOWN, row_headings: Provider::no_headings, column_headings: Provider::headings };
static AS_SELECTION: SelectionTable = SelectionTable { unknown: UNKNOWN, selection: Provider::selected_items, many: Provider::many, required: Provider::required };
static AS_SCROLL_ITEM: ScrollItemTable = ScrollItemTable { unknown: UNKNOWN, show: Provider::bring };
static AS_TEXT: TextTable = TextTable {
    unknown: UNKNOWN,
    selection: Provider::selection,
    in_view: Provider::in_view,
    of_child: Provider::of_child,
    at_spot: Provider::at_spot,
    all: Provider::all_text,
    selects: Provider::selects,
};

/// A window or one of its elements, towards Windows: a reference-counted COM object that frees
/// itself on the last release. It knows its element by id and role (the window is 0, with no
/// role) and answers from what Windows was last told. (An id is a place in the app's
/// description: when the app shows another page, an element of another role may take it, and to
/// a holder of the first provider that one is gone.)
struct Provider {
    faces: [Face; FACES],
    held: Cell<u32>,
    tree: Rc<WindowTree>,
    id: Id,
    role: Option<Role>,
}

/// Each role's control type: text, button (also a switch, toggled), check box, radio button,
/// tab item, list item, slider, edit, combo box, menu item, header item, progress bar, group,
/// pane (for a dialog), menu, menu bar, tab, hyperlink, tool bar (and menu item for a choice);
/// a table, its rows and cells as data items (as Windows' own programs and browsers do); a tree
/// and its items; a document, with text for its headings, paragraphs and code, a list and its
/// items, and a group for a quote.
fn control(role: Role) -> i32 {
    match role {
        Role::Text => 50020,
        Role::Button | Role::Fold | Role::Switch => 50000,
        Role::CheckBox => 50002,
        Role::RadioButton => 50013,
        Role::Tab => 50019,
        Role::Row => 50007,
        Role::Slider => 50015,
        Role::SpinButton => 50016,
        Role::TextBox | Role::PasswordBox => 50004,
        Role::ComboBox => 50003,
        Role::MenuItem | Role::MenuItemRadio | Role::Submenu => 50011,
        Role::ColumnHeader => 50035,
        Role::Progress => 50012,
        Role::Group => 50026,
        Role::Dialog => 50033,
        Role::Menu => 50009,
        Role::MenuBar => 50010,
        Role::Tabs => 50018,
        Role::Link => 50005,
        Role::Table => 50036,
        Role::TableRow | Role::Cell => 50029,
        Role::Document => 50030,
        Role::Heading | Role::Paragraph | Role::Code => 50020,
        Role::List => 50008,
        Role::ListItem => 50007,
        Role::Quote => 50026,
        Role::ToolBar => 50021,
        Role::Tree => 50023,
        Role::TreeItem => 50024,
        Role::Picture => 50006,
    }
}

/// The face through which an element supports a pattern: invoke (10000), value (10002), range
/// value (10003), expand-collapse (10005), selection item (10010), text (10014), toggle
/// (10015); for a table grid (10006) and table (10012), for a cell grid item (10007) and table
/// item (10013); for a tree selection (10001); and for anything scroll item (10017). A tree
/// item expands where it has children and is selectable where it can be. Nothing for what its
/// role does not support.
fn way(node: &AccessNode, pattern: i32) -> Option<usize> {
    match (pattern, node.role) {
        (10000, Role::Button | Role::MenuItem | Role::MenuItemRadio | Role::Submenu | Role::ColumnHeader | Role::Link) => Some(PRESS),
        (10002, Role::TextBox | Role::PasswordBox | Role::ComboBox | Role::SpinButton) => Some(TYPED),
        (10003, Role::Slider | Role::Progress | Role::SpinButton) => Some(SLIDE),
        (10005, Role::Fold | Role::ComboBox | Role::Submenu) => Some(FOLD),
        (10005, Role::TreeItem) if node.expanded.is_some() => Some(FOLD),
        (10010, Role::TreeItem) if node.on.is_some() => Some(CHOICE),
        (10001, Role::Tree | Role::Table) => Some(SELECTION),
        (10010, Role::RadioButton | Role::Tab | Role::Row | Role::MenuItemRadio) => Some(CHOICE),
        (10010, Role::TableRow) if node.on.is_some() => Some(CHOICE),
        (10006, Role::Table) => Some(GRID),
        (10012, Role::Table) => Some(HEADED),
        (10007, Role::Cell) => Some(IN_GRID),
        (10013, Role::Cell) => Some(UNDER_HEADING),
        (10017, _) => Some(SCROLL_ITEM),
        (10015, Role::CheckBox | Role::Switch) => Some(TICK),
        (10015, Role::Button) if node.on.is_some() => Some(TICK),
        (10015, Role::MenuItem) if node.on.is_some() => Some(TICK),
        (10014, _) if node.reads_text() => Some(TEXT),
        _ => None,
    }
}

impl Provider {
    /// A face of the provider for an element of a window (0: the window), with one more
    /// reference for the caller. One provider per element at a time: made on first ask, found
    /// again while anybody holds it.
    fn of(tree: &Rc<WindowTree>, id: Id, face: usize) -> *mut Face {
        let role = tree.nodes.borrow().iter().find(|node| node.id == id).map(|node| node.role);
        // SAFETY: the providers out are alive: each removes itself from the list before it is
        // freed.
        let known = tree.out.borrow().iter().copied().find(|provider| unsafe { ((**provider).id, (**provider).role) } == (id, role));
        let provider = known.unwrap_or_else(|| {
            let tables: [*const c_void; FACES] = [
                (&raw const AS_THING).cast(),
                (&raw const AS_PLACE).cast(),
                (&raw const AS_WINDOW).cast(),
                (&raw const AS_PRESS).cast(),
                (&raw const AS_TICK).cast(),
                (&raw const AS_CHOICE).cast(),
                (&raw const AS_TYPED).cast(),
                (&raw const AS_SLIDE).cast(),
                (&raw const AS_FOLD).cast(),
                (&raw const AS_TEXT).cast(),
                (&raw const AS_GRID).cast(),
                (&raw const AS_IN_GRID).cast(),
                (&raw const AS_HEADED).cast(),
                (&raw const AS_UNDER_HEADING).cast(),
                (&raw const AS_SELECTION).cast(),
                (&raw const AS_SCROLL_ITEM).cast(),
            ];
            let provider = Box::into_raw(Box::new(Provider { faces: tables.map(|table| Face { table, of: std::ptr::null() }), held: Cell::new(0), tree: tree.clone(), id, role }));
            // SAFETY: the provider was just made and nothing else knows it.
            unsafe { (*provider).faces.iter_mut().for_each(|face| face.of = provider) };
            tree.out.borrow_mut().push(provider);
            provider
        });
        // SAFETY: as above.
        unsafe { (*provider).held(face) }
    }

    /// One of this provider's faces, with one more reference.
    fn held(&self, face: usize) -> *mut Face {
        self.held.set(self.held.get() + 1);
        (&raw const self.faces[face]).cast_mut()
    }

    /// Where this provider's element is in its window's list; nothing for the window. (An error
    /// once the element or window is gone.)
    fn at(&self, nodes: &[AccessNode]) -> Result<Option<usize>, i32> {
        if self.tree.hwnd.get() == 0 {
            return Err(GONE);
        }
        if self.id == 0 {
            return Ok(None);
        }
        nodes.iter().position(|node| (node.id, Some(node.role)) == (self.id, self.role)).map(Some).ok_or(GONE)
    }

    /// The visible part of this provider's element: all of it, but in a table only as far as
    /// the table shows. (A row is told whole, however far the table is scrolled sideways.)
    fn visible_bounds(&self) -> Result<Rect, i32> {
        let nodes = self.tree.nodes.borrow();
        let at = self.at(&nodes)?.ok_or(NO_SUCH)?;
        Ok(accessibility::view(&nodes, at))
    }
    /// The element of this provider's table that `find` picks from the window's list and the
    /// table's index in it: its id. (Nothing where none; an error for an element not in a
    /// table.)
    fn in_table(&self, find: impl FnOnce(&[AccessNode], usize) -> Option<usize>) -> Result<Option<Id>, i32> {
        let nodes = self.tree.nodes.borrow();
        let at = self.at(&nodes)?.ok_or(NO_SUCH)?;
        let table = if nodes[at].role == Role::Table { Some(at) } else { accessibility::table_of(&nodes, at) };
        let table = table.ok_or(NOT_DONE)?;
        Ok(find(&nodes, table).map(|found| nodes[found].id))
    }
    /// The provider of this table's cell at a row and column, from 0. A row of a long table
    /// not told (it tells rows in view) is scrolled into view first: the window is asked. None
    /// where it still is not; an error for a row or column the table lacks.
    fn cell(&self, row: i32, column: i32) -> Result<*mut Face, i32> {
        let [rows, columns] = self.thing(|node| node.grid.unwrap_or_default())?;
        let place = [row as u32, column as u32];
        if row < 0 || column < 0 || place[0] >= rows || place[1] >= columns {
            return Err(NO_SUCH);
        }
        let find = |nodes: &[AccessNode], table: usize| accessibility::cell_at(nodes, table, place);
        if self.in_table(find)?.is_none() {
            // (Not done, the window was busy: none.)
            let table = self.in_table(|_, table| Some(table))?.unwrap_or(self.id);
            _ = self.tree.ask(Ask::Row(table, place[0], None));
        }
        Ok(self.in_table(find)?.map_or(std::ptr::null_mut(), |id| Provider::of(&self.tree, id, THING)))
    }
    /// A `SAFEARRAY` of the column headings: for a table, all in order; for a cell, its
    /// column's.
    fn headings_over(&self) -> Result<*mut c_void, i32> {
        let column = self.thing(|node| (node.role == Role::Cell).then(|| node.grid.unwrap_or_default()[1]))?;
        let mut ids = Vec::new();
        self.in_table(|nodes, table| {
            let heads = |at: &usize| nodes[*at].role == Role::ColumnHeader && nodes[*at].parent == Some(table);
            ids.extend((table + 1..nodes.len()).filter(heads).map(|at| nodes[at].id).filter(|_| column.is_none()));
            ids.extend(column.and_then(|column| accessibility::heading_at(nodes, table, column)).map(|at| nodes[at].id));
            None
        })?;
        Ok(self.providers(&ids))
    }
    /// A `SAFEARRAY` of the providers of some elements, by id. (The array holds a reference to
    /// each.)
    fn providers(&self, ids: &[Id]) -> *mut c_void {
        let automation = self.tree.automation;
        // SAFETY: Windows makes an array of so many `VT_UNKNOWN` (13) and adds its own
        // reference to each element put in; the creation reference ends.
        unsafe {
            let row = (automation.numbers)(13, 0, ids.len() as u32);
            for (at, id) in (0..).zip(ids) {
                let face = Provider::of(&self.tree, *id, THING);
                (automation.put)(row, &at, face.cast());
                Provider::release(face);
            }
            row
        }
    }

    /// Reads something of this provider's element.
    fn thing<T>(&self, read: impl FnOnce(&AccessNode) -> T) -> Result<T, i32> {
        let nodes = self.tree.nodes.borrow();
        let at = self.at(&nodes)?.ok_or(NO_SUCH)?;
        Ok(read(&nodes[at]))
    }

    /// Asks the window to act on this provider's element, which must exist and be enabled: the
    /// window gets a message, and has done it when the message returns.
    fn ask(&self, ask: Ask) -> Result<(), i32> {
        if !self.thing(|node| node.enabled)? {
            return Err(NOT_NOW);
        }
        self.tree.ask(ask)
    }

    /// A `SAFEARRAY` holding one range of this box's text, between two characters. (The array
    /// holds a reference.)
    fn stretches(&self, ends: [u32; 2]) -> *mut c_void {
        let (automation, stretch) = (self.tree.automation, Stretch::made(&self.tree, self.id, ends));
        // SAFETY: Windows makes an array of one `VT_UNKNOWN` (13) and adds its own reference to
        // the range put in; the creation reference ends.
        unsafe {
            let row = (automation.numbers)(13, 0, 1);
            (automation.put)(row, &0, stretch.cast());
            Stretch::release(stretch);
            row
        }
    }
    /// The selection in this box, in characters: both at the caret where nothing is selected,
    /// and at the start while unfocused.
    fn selected(&self) -> Result<[u32; 2], i32> {
        self.thing(|node| node.caret.map_or([0, 0], |[from, to]| [from.min(to), from.max(to)]))
    }
    /// How many characters the box holds.
    fn length(&self) -> Result<u32, i32> {
        self.thing(|node| node.text().chars().count() as u32)
    }

    /// A property of the element, by id. (The window's own come from Windows' host provider.)
    fn value_of(&self, property: i32) -> Result<Value, i32> {
        let nodes = self.tree.nodes.borrow();
        let Some(at) = self.at(&nodes)? else { return Ok(Value::None) };
        let node = &nodes[at];
        Ok(match property {
            CONTROL_TYPE => Value::Number(control(node.role)),
            NAME => Value::Text(node.name.clone()),
            HAS_KEYBOARD => Value::Truth(node.focused && self.tree.keyboard.get()),
            TAKES_KEYBOARD => Value::Truth(node.enabled && node.takes_keyboard()),
            ENABLED => Value::Truth(node.enabled),
            AUTOMATION_ID => Value::Text(format!("{:016x}", node.id)),
            KEYS if !node.shortcut.is_empty() => Value::Text(node.shortcut.clone()),
            HELP | DESCRIPTION if !node.description.is_empty() => Value::Text(node.description.clone()),
            HIDES_TEXT => Value::Truth(node.role == Role::PasswordBox),
            OFF_THE_SCREEN => Value::Truth(!node.visible),
            IS_DIALOG => Value::Truth(node.role == Role::Dialog),
            HEADING_LEVEL if node.role == Role::Heading => Value::Number(80050 + node.level.min(9) as i32),
            LEVEL if node.level > 0 => Value::Number(node.level as i32),
            NTH | OF_SO_MANY => node.position.map_or(Value::None, |of| Value::Number(of[(property == OF_SO_MANY) as usize] as i32)),
            // (The sorted column's heading says so, and the direction: in words, and as ARIA
            // has it.)
            STATUS => sort_status(node),
            ARIA if node.sorted.is_some() => Value::Text(format!("sort={}", node.sort_word().unwrap_or_default())),
            _ => Value::None,
        })
    }

    /// The element in a navigate direction: parent (0), next sibling (1), previous sibling (2),
    /// first child (3), last child (4). Its id (0: the window), or nothing where none.
    fn beside(&self, direction: i32) -> Result<Option<Id>, i32> {
        let nodes = self.tree.nodes.borrow();
        let me = self.at(&nodes)?;
        let within = |at: usize| nodes[at].parent;
        let mut all = 0..nodes.len();
        let found = match (direction, me) {
            (0, Some(at)) => return Ok(Some(within(at).map_or(0, |group| nodes[group].id))),
            (1, Some(at)) => (at + 1..nodes.len()).find(|other| within(*other) == within(at)),
            (2, Some(at)) => (0..at).rev().find(|other| within(*other) == within(at)),
            (3, _) => all.find(|other| within(*other) == me),
            (4, _) => all.rev().find(|other| within(*other) == me),
            _ => None,
        };
        Ok(found.map(|at| nodes[at].id))
    }

    /// Sets a slider to a value it can take. (Left alone where it is there already.)
    fn slide_to(&self, to: f64) -> Result<(), i32> {
        let [at, least, most] = self.thing(|node| node.value_range.unwrap_or_default().map(f64::from))?;
        if !matches!(self.thing(|node| node.role)?, Role::Slider | Role::SpinButton) {
            return Err(NOT_DONE);
        }
        if !(least..=most).contains(&to) {
            return Err(NO_SUCH);
        }
        if at == to { Ok(()) } else { self.ask(Ask::Set(self.id, to as f32)) }
    }

    /// The face through which this element supports a pattern (see `way`); none for unsupported
    /// ones, and for the window.
    fn face_for(&self, pattern: i32) -> Result<*mut Face, i32> {
        let nodes = self.tree.nodes.borrow();
        let face = self.at(&nodes)?.and_then(|at| way(&nodes[at], pattern));
        Ok(face.map_or(std::ptr::null_mut(), |face| self.held(face)))
    }

    /// Windows' own provider for the window, on which the window's provider builds: it knows
    /// the title, frame and place among windows. (None for an element.)
    fn windows_own(&self) -> *mut Face {
        let (hwnd, mut own) = (self.tree.hwnd.get(), std::ptr::null_mut());
        if self.id == 0 && hwnd != 0 {
            // SAFETY: asks Windows' provider for an existing window, with room for it.
            unsafe { (self.tree.automation.host)(hwnd, &mut own) };
        }
        own
    }

    /// The runtime id that tells this element from all others: `UiaAppendRuntimeId` (3), the
    /// id's two halves, and the role. (The window uses Windows': no array.)
    fn runtime_id_array(&self) -> *mut c_void {
        let (automation, numbers) = (self.tree.automation, [3, self.id as i32, (self.id >> 32) as i32, self.role.map_or(0, |role| role as i32)]);
        if self.id == 0 {
            return std::ptr::null_mut();
        }
        // SAFETY: Windows makes an array of four `VT_I4` (3) and copies one in at a time from
        // where pointed.
        unsafe {
            let row = (automation.numbers)(3, 0, 4);
            (0..4).for_each(|at: i32| _ = (automation.put)(row, &at, (&raw const numbers[at as usize]).cast()));
            row
        }
    }

    /// The element's screen rectangle in pixels: left, top, width, height. (The window's,
    /// Windows knows: none.)
    fn on_screen(&self) -> Result<[f64; 4], i32> {
        if self.id == 0 {
            return Ok([0.0; 4]);
        }
        let (rect, scale, corner) = (self.visible_bounds()?, self.tree.scale.get(), self.tree.corner());
        Ok([corner.x as f32 + rect.x * scale, corner.y as f32 + rect.y * scale, rect.w * scale, rect.h * scale].map(f64::from))
    }

    /// The element at a screen point: the topmost there (the last in the list: what lies over
    /// the page comes after it, a group's content after the group), or the window.
    fn thing_at(&self, x: f64, y: f64) -> Result<*mut Face, i32> {
        let (nodes, scale, corner) = (self.tree.nodes.borrow(), self.tree.scale.get() as f64, self.tree.corner());
        self.at(&nodes)?;
        let at = pt(((x - corner.x as f64) / scale) as f32, ((y - corner.y as f64) / scale) as f32);
        let id = (0..nodes.len()).rev().find(|n| accessibility::view(&nodes, *n).contains(at)).map_or(0, |n| nodes[n].id);
        Ok(Provider::of(&self.tree, id, PLACE))
    }

    /// The character at a screen point in this text, else the caret.
    fn letter_at(&self, spot: Spot) -> Result<u32, i32> {
        let (scale, corner) = (self.tree.scale.get() as f64, self.tree.corner());
        let at = pt(((spot.x - corner.x as f64) / scale) as f32, ((spot.y - corner.y as f64) / scale) as f32);
        let caret = self.selected()?[1];
        self.thing(|node| node.letter_at(at).unwrap_or(caret))
    }

    /// The focused element, if any.
    fn with_keyboard(&self) -> *mut Face {
        let on = self.tree.nodes.borrow().iter().find(|node| node.focused).map(|node| node.id);
        on.map_or(std::ptr::null_mut(), |id| Provider::of(&self.tree, id, PLACE))
    }

    /// Selects the element alone: a tree item as a click does, whatever else is selected;
    /// anything else as a click, unless selected already.
    fn pick_alone(&self) -> Result<(), i32> {
        match self.thing(|node| (node.role, node.on == Some(true)))? {
            (Role::TreeItem, _) => self.ask(Ask::Press(self.id)),
            (_, true) => Ok(()),
            (_, false) => self.ask(Ask::Press(self.id)),
        }
    }
    /// Adds the element to the selection (`more`) or removes it, unless it is so already: a
    /// tree item as Control+Space does, anything else as a click. (One of a single-selection
    /// set cannot be unselected: another is selected instead.)
    fn pick_too(&self, more: bool) -> Result<(), i32> {
        match self.thing(|node| (node.role, node.on == Some(true), node.position))? {
            (_, on, _) if on == more => Ok(()),
            (Role::TreeItem, ..) => self.ask(Ask::Pick(self.id)),
            (Role::TableRow, _, Some([nth, _])) => self.ask(Ask::Row(self.in_table(|_, table| Some(table))?.unwrap_or(0), nth - 1, Some(false))),
            _ if more => self.ask(Ask::Press(self.id)),
            _ => Err(NOT_DONE),
        }
    }
    /// Expands (`open`) or collapses the element, unless it is so already: a tree item as the
    /// arrow keys do, anything else as a click. (An element with nothing to expand cannot.)
    fn open_to(&self, open: bool) -> Result<(), i32> {
        match self.thing(|node| (node.role, node.unfolded()))? {
            (_, Some(now)) if now == open => Ok(()),
            (_, None) => Err(NOT_DONE),
            (Role::TreeItem, _) => self.ask(Ask::Open(self.id, open)),
            _ => self.ask(Ask::Press(self.id)),
        }
    }
    /// The provider of a tree item's tree, or a table row's table. (None for anything else.)
    fn item_tree(&self) -> Result<*mut Face, i32> {
        let nodes = self.tree.nodes.borrow();
        let at = self.at(&nodes)?.ok_or(NO_SUCH)?;
        let table = nodes[at].parent.filter(|table| nodes[at].role == Role::TableRow && nodes[*table].role == Role::Table);
        let found = accessibility::tree_of(&nodes, at).or(table);
        Ok(found.map_or(std::ptr::null_mut(), |found| Provider::of(&self.tree, nodes[found].id, THING)))
    }
    /// A `SAFEARRAY` of the providers of this tree's selected items, or this table's selected
    /// rows, among those told.
    fn tree_selection(&self) -> Result<*mut c_void, i32> {
        let nodes = self.tree.nodes.borrow();
        let at = self.at(&nodes)?.ok_or(NO_SUCH)?;
        let rows = &nodes[at].selected_rows;
        let picked = |item: &AccessNode| match item.role {
            Role::TableRow => item.position.is_some_and(|[nth, _]| rows.iter().any(|[from, to]| (*from..*to).contains(&(nth - 1)))),
            _ => item.on == Some(true),
        };
        let ids: Vec<Id> = nodes.iter().filter(|item| item.parent == Some(at) && picked(item)).map(|item| item.id).collect();
        Ok(self.providers(&ids))
    }

    /// `QueryInterface`. Every provider has all the faces above; only the window's is a
    /// fragment root. Asked for `IUnknown` (the last IID), it answers with its first face,
    /// always the same, as COM requires.
    unsafe extern "system" fn query(this: *mut Face, name: *const [u32; 4], out: *mut *mut Face) -> i32 {
        // SAFETY: Windows passes the IID and room for the answer; the rest is as for `answer`.
        unsafe {
            let name = name.read_unaligned();
            let face = NAMES.iter().position(|known| *known == name).map(|face| face % FACES);
            answer(this, out, |provider| face.filter(|face| *face != WINDOW || provider.id == 0).map(|face| provider.held(face)).ok_or(NO_INTERFACE))
        }
    }

    /// The provider whose function Windows called, by the face it came through. Windows calls
    /// only on the provider's window's thread: the provider says so (`options`), and its thread
    /// is a single-threaded apartment (`register`). Nothing here is thread-safe.
    ///
    /// # Safety
    /// `this` is a face of a held provider.
    unsafe fn called<'a>(this: *mut Face) -> &'a Provider {
        // SAFETY: as the caller promises; a face points back at its provider, alive while held.
        // (And the current thread is asked.)
        let provider = unsafe { &*(*this).of };
        debug_assert_eq!(unsafe { GetCurrentThreadId() }, provider.tree.thread, "a provider was called from a thread that is not its window's");
        provider
    }

    /// `AddRef`.
    unsafe extern "system" fn hold(this: *mut Face) -> u32 {
        // SAFETY: the caller holds the provider.
        let held = unsafe { &Provider::called(this).held };
        held.set(held.get() + 1);
        held.get()
    }

    /// `Release`; frees the provider on the last.
    unsafe extern "system" fn release(this: *mut Face) -> u32 {
        // SAFETY: the caller holds the provider, so it is alive. It was made by `Box::into_raw`
        // in `of`, and nothing uses it after the last release: it is out of the list it is
        // found in.
        unsafe {
            let (at, provider) = ((*this).of, Provider::called(this));
            let left = provider.held.get() - 1;
            provider.held.set(left);
            if left == 0 {
                provider.tree.out.borrow_mut().retain(|out| !std::ptr::eq(*out, at));
                drop(Box::from_raw(at.cast_mut()));
            }
            left
        }
    }

    /// The provider's options: server side (2), called on its own thread (0x20,
    /// `UseComThreading`).
    unsafe extern "system" fn options(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |_| Ok(0x22)) }
    }
    unsafe extern "system" fn pattern(this: *mut Face, pattern: i32, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.face_for(pattern)) }
    }
    unsafe extern "system" fn property(this: *mut Face, property: i32, out: *mut Variant) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.tree.automation.variant(&provider.value_of(property)?))) }
    }
    unsafe extern "system" fn host(this: *mut Face, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.windows_own())) }
    }

    unsafe extern "system" fn navigate(this: *mut Face, direction: i32, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.beside(direction)?.map_or(std::ptr::null_mut(), |id| Provider::of(&provider.tree, id, PLACE)))) }
    }
    unsafe extern "system" fn runtime_id(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.runtime_id_array())) }
    }
    unsafe extern "system" fn bounds(this: *mut Face, out: *mut [f64; 4]) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.on_screen()) }
    }
    /// Embedded fragment roots: none.
    unsafe extern "system" fn roots(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |_| Ok(std::ptr::null_mut())) }
    }
    /// Focus goes to the element. (To the window, Windows has done it.)
    unsafe extern "system" fn focus(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| if provider.id == 0 { Ok(()) } else { provider.ask(Ask::Focus(provider.id)) }) }
    }
    unsafe extern "system" fn root(this: *mut Face, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(Provider::of(&provider.tree, 0, WINDOW))) }
    }

    unsafe extern "system" fn at_point(this: *mut Face, x: f64, y: f64, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing_at(x, y)) }
    }
    unsafe extern "system" fn focused(this: *mut Face, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.with_keyboard())) }
    }

    /// `ScrollIntoView`: its container scrolls so it shows (focus stays). Any element, enabled
    /// or not.
    unsafe extern "system" fn bring(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.thing(|_| ()).and_then(|()| provider.tree.ask(Ask::Show(provider.id)))) }
    }
    /// `Invoke`: a button, menu row, heading; or a check box, which toggles.
    unsafe extern "system" fn press(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.ask(Ask::Press(provider.id))) }
    }
    /// Toggle, selection or expand state: on (1) or off (0).
    unsafe extern "system" fn on(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| (node.on == Some(true)) as i32)) }
    }
    /// Selects it alone (see `pick_alone`).
    unsafe extern "system" fn pick(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.pick_alone()) }
    }
    /// Adds it to the selection, or removes it (see `pick_too`).
    unsafe extern "system" fn add_pick(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.pick_too(true)) }
    }
    unsafe extern "system" fn unpick(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.pick_too(false)) }
    }
    /// Its selection container: a tree item's tree, a table row's table. (Not told for anything
    /// else.)
    unsafe extern "system" fn among(this: *mut Face, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.item_tree()) }
    }
    /// Expands and collapses it (see `open_to`).
    unsafe extern "system" fn unfold(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.open_to(true)) }
    }
    unsafe extern "system" fn fold(this: *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.open_to(false)) }
    }
    /// Expanded (1) or collapsed (0); a tree item with no children is a leaf (3).
    unsafe extern "system" fn unfolded(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| node.unfolded().map_or(3, |open| open as i32))) }
    }
    /// A tree's selected items, among those told.
    unsafe extern "system" fn selected_items(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.tree_selection()) }
    }
    /// Whether a tree allows multiple selection; a table does.
    unsafe extern "system" fn many(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| (node.on == Some(true) || node.role == Role::Table) as i32)) }
    }
    /// Whether selection is required: never.
    unsafe extern "system" fn required(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|_| 0)) }
    }

    /// `SetValue`: the given text replaces the box's. (Not for a box that only shows a choice
    /// from a list.)
    unsafe extern "system" fn type_in(this: *mut Face, text: *const u16) -> i32 {
        // SAFETY: as for `answer`; the text is NUL-terminated and read up to there.
        unsafe {
            let len = (0..).take_while(|at| !text.is_null() && *text.add(*at) != 0).count();
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(if len == 0 { std::ptr::dangling() } else { text }, len));
            answer(this, &mut (), |provider| if provider.thing(|node| node.role)? == Role::ComboBox { Err(NOT_DONE) } else { provider.ask(Ask::Type(provider.id, text)) })
        }
    }
    unsafe extern "system" fn typed(this: *mut Face, out: *mut Text) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.tree.automation.text(&provider.thing(|node| node.value.clone())?))) }
    }
    /// Read-only: what is disabled, and progress bars.
    unsafe extern "system" fn read_only(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| (!node.enabled || matches!(node.role, Role::Progress | Role::ComboBox)) as i32)) }
    }

    unsafe extern "system" fn slide(this: *mut Face, to: f64) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, &mut (), |provider| provider.slide_to(to)) }
    }
    /// The value (`WHICH` 0), minimum (1) or maximum (2).
    unsafe extern "system" fn amount<const WHICH: usize>(this: *mut Face, out: *mut f64) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| node.value_range.unwrap_or_default()[WHICH] as f64)) }
    }
    /// The small change, as the part says (0 for what does not step).
    unsafe extern "system" fn step(this: *mut Face, out: *mut f64) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| node.step as f64)) }
    }

    /// A table's cell at a row and column, from 0. Nothing for a row out of view (a long table
    /// tells only visible rows); an error for a row or column the table lacks.
    unsafe extern "system" fn item(this: *mut Face, row: i32, column: i32, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.cell(row, column)) }
    }
    /// A table's row count (`WHICH` 0) and visible column count (1); a cell's row (0) and
    /// column (1), from 0.
    unsafe extern "system" fn grid_number<const WHICH: usize>(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|node| node.grid.unwrap_or_default()[WHICH] as i32)) }
    }
    /// Row or column span: 1.
    unsafe extern "system" fn one(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|_| 1)) }
    }
    /// A cell's table.
    unsafe extern "system" fn grid_of(this: *mut Face, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.in_table(|_, table| Some(table))?.map_or(std::ptr::null_mut(), |id| Provider::of(&provider.tree, id, THING)))) }
    }
    /// The column headings: a table's, in order; a cell's own.
    unsafe extern "system" fn headings(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.headings_over()) }
    }
    /// Row headings: none.
    unsafe extern "system" fn no_headings(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|_| ()).map(|_| provider.providers(&[]))) }
    }
    /// Row or column major: row (0).
    unsafe extern "system" fn by_rows(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| provider.thing(|_| 0)) }
    }

    /// A text box's selection: one range, empty at the caret where nothing is selected.
    unsafe extern "system" fn selection(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.stretches(provider.selected()?))) }
    }
    /// The visible range: from the first character that shows to the last.
    unsafe extern "system" fn in_view(this: *mut Face, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(provider.stretches(provider.thing(AccessNode::shown_letters)?))) }
    }
    /// `RangeFromChild`: a text box has no children.
    unsafe extern "system" fn of_child(this: *mut Face, _child: *mut Face, out: *mut *mut Stretch) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |_| Err(NO_SUCH)) }
    }
    /// `RangeFromPoint`: empty, before the character at a screen point (the nearest on its
    /// line); the caret where letters are not known.
    unsafe extern "system" fn at_spot(this: *mut Face, spot: Spot, out: *mut *mut Stretch) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(Stretch::made(&provider.tree, provider.id, [provider.letter_at(spot)?; 2]))) }
    }
    unsafe extern "system" fn all_text(this: *mut Face, out: *mut *mut Stretch) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |provider| Ok(Stretch::made(&provider.tree, provider.id, [0, provider.length()?]))) }
    }
    /// Supported text selection: single (1).
    unsafe extern "system" fn selects(this: *mut Face, out: *mut i32) -> i32 {
        // SAFETY: as for `answer`.
        unsafe { answer(this, out, |_| Ok(1)) }
    }
}

impl WindowTree {
    /// Asks the window to act: it gets a message, and has done it when the message returns.
    fn ask(&self, ask: Ask) -> Result<(), i32> {
        self.asked.replace(Some(ask));
        // SAFETY: a message to the window, as input sends it.
        unsafe { SendMessageW(self.hwnd.get(), WM_READER, 0, 0) };
        // (Still asked means not done: the window was busy with something else.)
        self.asked.take().map_or(Ok(()), |_| Err(NOT_NOW))
    }
}

/// What a text's units are found by besides its characters: its misspelled words, and where
/// its lines start (as wrapped).
#[derive(Default)]
struct Marks {
    wrong: Vec<[u32; 2]>,
    lines: Vec<u32>,
}

/// Whether a text unit starts at character `at`: character (0), format (1: where a misspelled
/// word starts or ends), word (2: after whitespace; trailing space belongs to the word), line
/// (3: as wrapped), paragraph (4: after a line break). Of every other unit (page, document)
/// there is one: all the text. Both ends of the text start every unit.
fn starts(text: &[char], unit: i32, at: usize, marks: &Marks) -> bool {
    match unit {
        _ if at == 0 || at >= text.len() => true,
        0 => true,
        1 => marks.wrong.iter().any(|word| word.contains(&(at as u32))),
        2 => text[at - 1] == '\n' || text[at - 1].is_whitespace() && !text[at].is_whitespace(),
        3 => marks.lines.binary_search(&(at as u32)).is_ok(),
        4 => text[at - 1] == '\n',
        _ => false,
    }
}

/// Whether a value sought among annotation types holds the spelling error: one number
/// (`VT_I4`, 3), or an array of them (`VT_ARRAY | VT_I4`, 0x2003).
fn asks_wrong(value: &Variant) -> bool {
    let Some(automation) = automation() else { return false };
    match value.kind {
        // SAFETY: a `VT_I4` holds a number.
        3 => (unsafe { value.held.number }) == SPELT_WRONG,
        // SAFETY: a `VT_ARRAY | VT_I4` holds a one-dimensional array of numbers, read one at
        // a time into room for one.
        0x2003 => unsafe {
            let (row, mut last) = (value.held.object, -1);
            (automation.last)(row, 1, &mut last);
            (0..=last).any(|at| {
                let mut number = 0_i32;
                (automation.get)(row, &at, (&raw mut number).cast()) == 0 && number == SPELT_WRONG
            })
        },
        _ => false,
    }
}

/// A range's annotation types, between two characters: spelling error where all of it lies in a
/// misspelled word of `wrong` (for an empty range, the next character does); nothing where none
/// does; and mixed (`None`) where part does.
fn marked(wrong: &[[u32; 2]], [from, to]: [u32; 2]) -> Option<&'static [i32]> {
    let overlaps = |[start, end]: &[u32; 2]| if from == to { *start <= from && from < *end } else { *start < to && from < *end };
    match wrong.iter().find(|word| overlaps(word)) {
        None => Some(&[]),
        Some([start, end]) if *start <= from && (to <= *end || from == to) => Some(&[SPELT_WRONG]),
        Some(_) => None,
    }
}
/// The start of the unit `at` lies in, and of the next.
fn start_before(text: &[char], unit: i32, at: usize, marks: &Marks) -> usize {
    (0..=at.min(text.len())).rev().find(|at| starts(text, unit, *at, marks)).unwrap_or(0)
}
fn start_after(text: &[char], unit: i32, at: usize, marks: &Marks) -> usize {
    (at + 1..=text.len()).find(|at| starts(text, unit, *at, marks)).unwrap_or(text.len())
}
/// Moves `count` units from `at` (back for a negative count), as far as there are: where it
/// ends, and how many it moved.
fn gone_by(text: &[char], unit: i32, at: usize, count: i32, marks: &Marks) -> (usize, i32) {
    let (mut at, mut went) = (at.min(text.len()), 0);
    while went < count && at < text.len() {
        (at, went) = (start_after(text, unit, at, marks), went + 1);
    }
    while went > count && at > 0 {
        (at, went) = (start_before(text, unit, at - 1, marks), went - 1);
    }
    (at, went)
}

/// A text range of a text box, as Windows holds one: between two characters, which a screen
/// reader moves to read by character, word and line. A reference-counted COM object of its own.
/// It reads the text as it is when asked, its ends kept inside; once the box or window is gone,
/// it says so.
#[repr(C)]
struct Stretch {
    table: *const StretchTable,
    held: Cell<u32>,
    tree: Rc<WindowTree>,
    id: Id,
    ends: Cell<[u32; 2]>,
}

/// The range vtable, in the order Windows declares it.
#[repr(C)]
struct StretchTable {
    query: unsafe extern "system" fn(*mut Stretch, *const [u32; 4], *mut *mut Stretch) -> i32,
    hold: unsafe extern "system" fn(*mut Stretch) -> u32,
    release: unsafe extern "system" fn(*mut Stretch) -> u32,
    copy: unsafe extern "system" fn(*mut Stretch, *mut *mut Stretch) -> i32,
    same: unsafe extern "system" fn(*mut Stretch, *mut Stretch, *mut i32) -> i32,
    compare_ends: unsafe extern "system" fn(*mut Stretch, i32, *mut Stretch, i32, *mut i32) -> i32,
    widen: unsafe extern "system" fn(*mut Stretch, i32) -> i32,
    find_look: unsafe extern "system" fn(*mut Stretch, i32, Variant, i32, *mut *mut Stretch) -> i32,
    find_text: unsafe extern "system" fn(*mut Stretch, *const u16, i32, i32, *mut *mut Stretch) -> i32,
    look: unsafe extern "system" fn(*mut Stretch, i32, *mut Variant) -> i32,
    lies: unsafe extern "system" fn(*mut Stretch, *mut *mut c_void) -> i32,
    within: unsafe extern "system" fn(*mut Stretch, *mut *mut Face) -> i32,
    text: unsafe extern "system" fn(*mut Stretch, i32, *mut Text) -> i32,
    go: unsafe extern "system" fn(*mut Stretch, i32, i32, *mut i32) -> i32,
    move_end: unsafe extern "system" fn(*mut Stretch, i32, i32, i32, *mut i32) -> i32,
    move_end_to: unsafe extern "system" fn(*mut Stretch, i32, *mut Stretch, i32) -> i32,
    select: unsafe extern "system" fn(*mut Stretch) -> i32,
    add: unsafe extern "system" fn(*mut Stretch) -> i32,
    remove: unsafe extern "system" fn(*mut Stretch) -> i32,
    show: unsafe extern "system" fn(*mut Stretch, i32) -> i32,
    inside: unsafe extern "system" fn(*mut Stretch, *mut *mut c_void) -> i32,
}

static AS_STRETCH: StretchTable = StretchTable {
    query: Stretch::query,
    hold: Stretch::hold,
    release: Stretch::release,
    copy: Stretch::copy,
    same: Stretch::same,
    compare_ends: Stretch::compare_ends,
    widen: Stretch::widen,
    find_look: Stretch::find_look,
    find_text: Stretch::find_text,
    look: Stretch::look,
    lies: Stretch::lies,
    within: Stretch::within,
    text: Stretch::text,
    go: Stretch::go,
    move_end: Stretch::move_end,
    move_end_to: Stretch::move_end_to,
    select: Stretch::select,
    add: Stretch::not_done,
    remove: Stretch::not_done,
    show: Stretch::show,
    inside: Stretch::inside,
};

/// What every range function does: as `answer` for a provider.
///
/// # Safety
/// `this` is a held range, and `out` has room for an answer.
unsafe fn about<T>(this: *mut Stretch, out: *mut T, ask: impl FnOnce(&Stretch) -> Result<T, i32>) -> i32 {
    // SAFETY: as the caller promises. (All zeros is valid for every answer type.) The range is
    // referenced while it answers, as a provider is (see `answer`), and released when nothing
    // of it is borrowed.
    unsafe {
        Stretch::hold(this);
        let (answer, code) = match ask(&*this) {
            Ok(answer) => (answer, 0),
            Err(code) => (std::mem::zeroed(), code),
        };
        out.write(answer);
        Stretch::release(this);
        code
    }
}

impl Stretch {
    /// Another range Windows hands over, if it is one of this library's: a COM object's first
    /// word is its vtable pointer, and these have theirs. (Another's, or null, is not read past
    /// that word.)
    ///
    /// # Safety
    /// `other` is a held COM object, or null.
    unsafe fn ours<'a>(other: *mut Stretch) -> Option<&'a Stretch> {
        // SAFETY: as the caller promises, every COM object starts with that word.
        let ours = !other.is_null() && unsafe { other.cast::<*const StretchTable>().read() } == &raw const AS_STRETCH;
        // SAFETY: an object with that vtable is a whole range made in `made`, and held.
        ours.then(|| unsafe { &*other })
    }
    /// A range of box `id`'s text, referenced once.
    fn made(tree: &Rc<WindowTree>, id: Id, ends: [u32; 2]) -> *mut Stretch {
        Box::into_raw(Box::new(Stretch { table: &raw const AS_STRETCH, held: Cell::new(1), tree: tree.clone(), id, ends: Cell::new(ends) }))
    }
    /// Reads something of its box. (An error once the box or window is gone.)
    fn of_box<T>(&self, read: impl FnOnce(&AccessNode) -> T) -> Result<T, i32> {
        let nodes = self.tree.nodes.borrow();
        let found = nodes.iter().find(|node| node.id == self.id && node.reads_text());
        found.filter(|_| self.tree.hwnd.get() != 0).map(read).ok_or(GONE)
    }
    /// Its box's text now, in characters; and its two ends inside that.
    fn letters(&self) -> Result<(Vec<char>, [usize; 2]), i32> {
        let letters: Vec<char> = self.of_box(|node| node.text().chars().collect())?;
        let ends = self.ends.get().map(|end| (end as usize).min(letters.len()));
        Ok((letters, ends))
    }
    fn put(&self, ends: [usize; 2]) {
        self.ends.set(ends.map(|end| end as u32));
    }

    /// `ExpandToEnclosingUnit`.
    fn widened(&self, unit: i32) -> Result<(), i32> {
        let (letters, [from, _]) = self.letters()?;
        let marks = self.marks()?;
        let from = start_before(&letters, unit, from, &marks);
        self.put([from, start_after(&letters, unit, from, &marks).max(from)]);
        Ok(())
    }
    /// Moves by `count` units: an empty range as it is, any other as the one unit starting
    /// where it lands. Returns how many it moved.
    fn went(&self, unit: i32, count: i32) -> Result<i32, i32> {
        let (letters, [from, to]) = self.letters()?;
        let marks = self.marks()?;
        let (mut at, mut went) = gone_by(&letters, unit, from, count, &marks);
        // (A non-empty range stops at the last unit: the text's end is a caret position, not a
        // unit.)
        if from != to && went > 0 && at >= letters.len() {
            (at, went) = (start_before(&letters, unit, at - 1, &marks), went - 1);
        }
        if count != 0 {
            self.put([at, if from == to { at } else { start_after(&letters, unit, at, &marks).max(at) }]);
        }
        Ok(went)
    }
    /// Moves one end by `count` units; an end passing the other takes it along.
    fn end_went(&self, end: i32, unit: i32, count: i32) -> Result<i32, i32> {
        let (letters, mut ends) = self.letters()?;
        let marks = self.marks()?;
        let (at, went) = gone_by(&letters, unit, ends[(end != 0) as usize], count, &marks);
        ends[(end != 0) as usize] = at;
        self.put(if ends[0] > ends[1] { [at, at] } else { ends });
        Ok(went)
    }
    /// Moves one end to another range's end.
    fn end_put(&self, end: i32, to: u32) {
        let mut ends = self.ends.get();
        ends[(end != 0) as usize] = to;
        self.ends.set(if ends[0] > ends[1] { [to, to] } else { ends });
    }
    /// Its text, or up to `most` characters (negative: all).
    fn read(&self, most: i32) -> Result<Text, i32> {
        let (letters, [from, to]) = self.letters()?;
        let text: String = letters[from..to].iter().take(if most < 0 { usize::MAX } else { most as usize }).collect();
        Ok(self.tree.automation.text(&text))
    }
    /// The first occurrence of a text in it (the last, backward), as a range of its own; none
    /// where absent.
    fn found(&self, sought: &str, backward: bool, any_case: bool) -> Result<*mut Stretch, i32> {
        let (letters, [from, to]) = self.letters()?;
        let small = |letter: &char| if any_case { letter.to_lowercase().next().unwrap_or(*letter) } else { *letter };
        let sought: Vec<char> = sought.chars().map(|letter| small(&letter)).collect();
        let fits = |at: &usize| letters[*at..].iter().map(small).take(sought.len()).eq(sought.iter().copied());
        let mut places = (from..(to + 1).saturating_sub(sought.len())).filter(fits);
        let at = if backward { places.next_back() } else { places.next() };
        let stretch = |at: usize| Stretch::made(&self.tree, self.id, [at as u32, (at + sought.len()) as u32]);
        Ok(at.filter(|_| !sought.is_empty()).map_or(std::ptr::null_mut(), stretch))
    }
    /// A text attribute: read-only, and annotation types (see `marked`); anything else is
    /// unsupported.
    fn looks(&self, look: i32) -> Result<Variant, i32> {
        let (automation, usable) = (self.tree.automation, self.of_box(|node| node.enabled)?);
        if look == READ_ONLY {
            return Ok(automation.variant(&Value::Truth(!usable)));
        }
        let marks = if look == MARKS { Some(marked(&self.marks()?.wrong, self.ends.get())) } else { None };
        if let Some(Some(marks)) = marks {
            // SAFETY: Windows makes an array of so many `VT_I4` (3) and copies one in at a time
            // from where pointed; it goes in a `VT_ARRAY | VT_I4` (0x2003) value for the
            // receiver.
            let row = unsafe {
                let row = (automation.numbers)(3, 0, marks.len() as u32);
                (0..marks.len() as i32).for_each(|at| _ = (automation.put)(row, &at, (&raw const marks[at as usize]).cast()));
                row
            };
            return Ok(Variant { kind: 0x2003, unused: [0; 3], held: Held { object: row } });
        }
        let mut answer = std::ptr::null_mut();
        // SAFETY: Windows hands out its mixed value (part annotated, part not) or its
        // not-supported value, a `VT_UNKNOWN` (13), referenced once for the receiver.
        unsafe { if marks.is_some() { (automation.mixed)(&mut answer) } else { (automation.unknown)(&mut answer) } };
        Ok(Variant { kind: if answer.is_null() { 0 } else { 13 }, unused: [0; 3], held: Held { object: answer } })
    }
    /// Its box's misspelled words ([`AccessNode::misspelled`]) and lines.
    fn marks(&self) -> Result<Marks, i32> {
        self.of_box(|node| Marks { wrong: node.misspelled.clone(), lines: node.line_starts() })
    }
    /// Its bounding rectangles as a `SAFEARRAY` (left, top, width, height, each in a row): one
    /// per line it shows on, as drawn; an empty range's, the cursor's place there.
    fn on_screen(&self) -> Result<*mut c_void, i32> {
        let lines = self.of_box(|node| node.range_rects(self.ends.get()))?;
        let (scale, corner, automation) = (self.tree.scale.get(), self.tree.corner(), self.tree.automation);
        let numbers: Vec<f64> = lines.iter().flat_map(|line| [corner.x as f32 + line.x * scale, corner.y as f32 + line.y * scale, line.w * scale, line.h * scale]).map(f64::from).collect();
        // SAFETY: Windows makes an array of `VT_R8` (5) and copies one in at a time from where
        // pointed.
        unsafe {
            let row = (automation.numbers)(5, 0, numbers.len() as u32);
            (0..numbers.len() as i32).for_each(|at| _ = (automation.put)(row, &at, (&raw const numbers[at as usize]).cast()));
            Ok(row)
        }
    }
    /// `FindAttribute` for misspelled words (annotation types holding a spelling error): the
    /// first wholly in the range, or the last, backward. Nothing for other attributes.
    fn found_wrong(&self, look: i32, value: &Variant, backward: bool) -> Result<*mut Stretch, i32> {
        let [from, to] = self.ends.get();
        let mut words = self.marks()?.wrong.into_iter().filter(|[start, end]| from <= *start && *end <= to && look == MARKS && asks_wrong(value));
        let word = if backward { words.next_back() } else { words.next() };
        Ok(word.map_or(std::ptr::null_mut(), |word| Stretch::made(&self.tree, self.id, word)))
    }
    /// Has its box select it: the window is asked. (Not a disabled box, nor a password box.)
    fn selects(&self) -> Result<(), i32> {
        if !self.of_box(|node| node.enabled && node.role == Role::TextBox)? {
            return Err(NOT_NOW);
        }
        self.tree.ask(Ask::Select(self.id, self.ends.get()))
    }

    /// `QueryInterface`: a range, and `IUnknown`.
    unsafe extern "system" fn query(this: *mut Stretch, name: *const [u32; 4], out: *mut *mut Stretch) -> i32 {
        // SAFETY: Windows passes the IID; the rest is as for `about`.
        unsafe {
            let known = [A_STRETCH, NAMES[FACES]].contains(&name.read_unaligned());
            about(this, out, |stretch| if known { Ok((stretch.held.set(stretch.held.get() + 1), this).1) } else { Err(NO_INTERFACE) })
        }
    }
    unsafe extern "system" fn hold(this: *mut Stretch) -> u32 {
        // SAFETY: the caller holds the range.
        let held = unsafe { &(*this).held };
        held.set(held.get() + 1);
        held.get()
    }
    /// `Release`; frees the range on the last.
    unsafe extern "system" fn release(this: *mut Stretch) -> u32 {
        // SAFETY: the caller holds the range, so it is alive; it was made by `Box::into_raw` in
        // `made`, and nobody knows it after the last release.
        unsafe {
            let left = (*this).held.get() - 1;
            (*this).held.set(left);
            if left == 0 {
                drop(Box::from_raw(this));
            }
            left
        }
    }
    unsafe extern "system" fn copy(this: *mut Stretch, out: *mut *mut Stretch) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| Ok(Stretch::made(&stretch.tree, stretch.id, stretch.ends.get()))) }
    }
    /// Whether another range covers the same characters of the same box.
    unsafe extern "system" fn same(this: *mut Stretch, other: *mut Stretch, out: *mut i32) -> i32 {
        // SAFETY: as for `about`; the other is a held COM object, or null.
        unsafe {
            let other = Stretch::ours(other).map(|other| (other.id, other.ends.get()));
            about(this, out, |stretch| other.map(|other| (other == (stretch.id, stretch.ends.get())) as i32).ok_or(NO_SUCH))
        }
    }
    /// How far an endpoint of this one (0: start, 1: end) lies after another's.
    unsafe extern "system" fn compare_ends(this: *mut Stretch, end: i32, other: *mut Stretch, other_end: i32, out: *mut i32) -> i32 {
        // SAFETY: as for `same`.
        unsafe {
            let there = Stretch::ours(other).map(|other| other.ends.get()[(other_end != 0) as usize] as i32);
            about(this, out, |stretch| there.map(|there| stretch.ends.get()[(end != 0) as usize] as i32 - there).ok_or(NO_SUCH))
        }
    }
    unsafe extern "system" fn widen(this: *mut Stretch, unit: i32) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, &mut (), |stretch| stretch.widened(unit)) }
    }
    /// `FindAttribute`: misspelled words (see `found_wrong`).
    unsafe extern "system" fn find_look(this: *mut Stretch, look: i32, value: Variant, backward: i32, out: *mut *mut Stretch) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.found_wrong(look, &value, backward != 0)) }
    }
    unsafe extern "system" fn find_text(this: *mut Stretch, text: *const u16, backward: i32, any_case: i32, out: *mut *mut Stretch) -> i32 {
        // SAFETY: as for `about`; the text is NUL-terminated and read up to there.
        unsafe {
            let len = (0..).take_while(|at| !text.is_null() && *text.add(*at) != 0).count();
            let sought = String::from_utf16_lossy(std::slice::from_raw_parts(if len == 0 { std::ptr::dangling() } else { text }, len));
            about(this, out, |stretch| stretch.found(&sought, backward != 0, any_case != 0))
        }
    }
    unsafe extern "system" fn look(this: *mut Stretch, look: i32, out: *mut Variant) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.looks(look)) }
    }
    unsafe extern "system" fn lies(this: *mut Stretch, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.on_screen()) }
    }
    /// `GetEnclosingElement`: its box.
    unsafe extern "system" fn within(this: *mut Stretch, out: *mut *mut Face) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.of_box(|_| ()).map(|_| Provider::of(&stretch.tree, stretch.id, THING))) }
    }
    unsafe extern "system" fn text(this: *mut Stretch, most: i32, out: *mut Text) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.read(most)) }
    }
    unsafe extern "system" fn go(this: *mut Stretch, unit: i32, count: i32, out: *mut i32) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.went(unit, count)) }
    }
    unsafe extern "system" fn move_end(this: *mut Stretch, end: i32, unit: i32, count: i32, out: *mut i32) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, out, |stretch| stretch.end_went(end, unit, count)) }
    }
    unsafe extern "system" fn move_end_to(this: *mut Stretch, end: i32, other: *mut Stretch, other_end: i32) -> i32 {
        // SAFETY: as for `same`.
        unsafe {
            let there = Stretch::ours(other).map(|other| other.ends.get()[(other_end != 0) as usize]);
            about(this, &mut (), |stretch| there.map(|there| stretch.end_put(end, there)).ok_or(NO_SUCH))
        }
    }
    unsafe extern "system" fn select(this: *mut Stretch) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, &mut (), |stretch| stretch.selects()) }
    }
    /// `AddToSelection`, `RemoveFromSelection`: a box has one selection.
    unsafe extern "system" fn not_done(this: *mut Stretch) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, &mut (), |_| Err(NOT_DONE)) }
    }
    /// `ScrollIntoView`: the box keeps its caret in view itself.
    unsafe extern "system" fn show(this: *mut Stretch, _at_top: i32) -> i32 {
        // SAFETY: as for `about`.
        unsafe { about(this, &mut (), |stretch| stretch.of_box(|_| ())) }
    }
    /// `GetChildren`: none (an empty array).
    unsafe extern "system" fn inside(this: *mut Stretch, out: *mut *mut c_void) -> i32 {
        // SAFETY: as for `about`; Windows makes an empty array.
        unsafe { about(this, out, |stretch| stretch.of_box(|_| (stretch.tree.automation.numbers)(13, 0, 0))) }
    }
}

/// What every provider function does: the provider is found by the face called and gives its
/// answer, written where Windows wants it. No answer is all zeros ("none" for every answer
/// type), and the return says why.
///
/// # Safety
/// `this` is a face of a held provider, and `out` has room for an answer: so whenever Windows
/// calls a provider function.
unsafe fn answer<T>(this: *mut Face, out: *mut T, ask: impl FnOnce(&Provider) -> Result<T, i32>) -> i32 {
    // SAFETY: as the caller promises. (All zeros is valid for every answer type here: a number,
    // a pointer, a rectangle, a `VT_EMPTY` `Variant`.) And the provider is referenced while it
    // answers: what it asks of its window may close the window (a button closing a second
    // window), which releases all its providers. So the provider, and what was told of its
    // window, stay until the answer is written; then the reference ends, when nothing of the
    // provider is borrowed.
    unsafe {
        Provider::hold(this);
        let (answer, code) = match ask(Provider::called(this)) {
            Ok(answer) => (answer, 0),
            Err(code) => (std::mem::zeroed(), code),
        };
        out.write(answer);
        Provider::release(this);
        code
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::accessibility::Letters;
    use crate::geom::rect;
    use std::sync::Arc;

    #[link(name = "oleaut32")]
    unsafe extern "system" {
        fn SysStringLen(text: Text) -> u32;
        fn SafeArrayGetElement(row: *mut c_void, at: *const i32, out: *mut c_void) -> i32;
        fn SafeArrayDestroy(row: *mut c_void) -> i32;
    }

    /// Pattern ids and navigate directions.
    const TO_PRESS: i32 = 10000;
    const TO_TYPE: i32 = 10002;
    const TO_SLIDE: i32 = 10003;
    const TO_FOLD: i32 = 10005;
    const TO_PICK: i32 = 10010;
    const TO_TICK: i32 = 10015;
    const UP: i32 = 0;
    const NEXT: i32 = 1;
    const BACK: i32 = 2;
    const FIRST: i32 = 3;
    const LAST: i32 = 4;

    /// An element as the interface tells it, with no state. It lies in a list with the others,
    /// by id.
    fn thing(id: Id, role: Role, name: &str, within: Option<usize>) -> AccessNode {
        let rect = rect(id as f32 * 20.0, 10.0, 16.0, 8.0);
        AccessNode::plain(id, role, name.into(), rect, within)
    }

    /// A window with a table: 40 rows of which two show (the 11th and 12th), two columns; their
    /// headings, the first sorting the rows; and the two rows, the first selected, each with
    /// its cells. The second row's second cell holds a button and no text.
    fn ledger() -> Vec<AccessNode> {
        let cell = |id: Id, name: &str, row: usize, place: [u32; 2]| AccessNode { grid: Some(place), ..thing(id, Role::Cell, name, Some(row)) };
        vec![
            AccessNode { grid: Some([40, 2]), rect: rect(0.0, 0.0, 200.0, 100.0), selected_rows: vec![[10, 11]], ..thing(1, Role::Table, "", None) },
            AccessNode { position: Some([1, 2]), sorted: Some(false), ..thing(2, Role::ColumnHeader, "Name", Some(0)) },
            AccessNode { position: Some([2, 2]), ..thing(3, Role::ColumnHeader, "Size", Some(0)) },
            AccessNode { position: Some([11, 40]), on: Some(true), ..thing(4, Role::TableRow, "a.txt, 3 KB", Some(0)) },
            cell(5, "a.txt", 3, [10, 0]),
            cell(6, "3 KB", 3, [10, 1]),
            AccessNode { position: Some([12, 40]), on: Some(false), ..thing(7, Role::TableRow, "b.txt, Open", Some(0)) },
            cell(8, "b.txt", 6, [11, 0]),
            cell(9, "Open", 6, [11, 1]),
            thing(10, Role::Button, "Open", Some(8)),
        ]
    }

    /// A window with one element of every kind: a line of text, a group with a button and a
    /// check box, and the rest beside them.
    fn everything() -> Vec<AccessNode> {
        vec![
            thing(1, Role::Text, "Sound", None),
            AccessNode { rect: rect(56.0, 6.0, 44.0, 16.0), ..thing(2, Role::Group, "Tools", None) },
            thing(3, Role::Button, "Save", Some(1)),
            AccessNode { on: Some(true), ..thing(4, Role::CheckBox, "Loud", Some(1)) },
            AccessNode { value_range: Some([4.0, 0.0, 10.0]), step: 0.5, ..thing(5, Role::Slider, "", None) },
            AccessNode { value: "Ada".into(), ..thing(6, Role::TextBox, "Your name", None) },
            AccessNode { on: Some(false), enabled: false, ..thing(7, Role::RadioButton, "Never", None) },
            AccessNode { on: Some(false), ..thing(8, Role::Fold, "More", None) },
            AccessNode { on: Some(true), ..thing(9, Role::MenuItem, "Dark", None) },
            thing(10, Role::ColumnHeader, "Size", None),
            AccessNode { value_range: Some([0.5, 0.0, 1.0]), ..thing(11, Role::Progress, "", None) },
        ]
    }

    /// What Windows was told of a missing window (no window is 1), at two pixels per unit,
    /// focused.
    fn made_tree(things: Vec<AccessNode>) -> Rc<WindowTree> {
        let thread = unsafe { GetCurrentThreadId() };
        Rc::new(WindowTree {
            automation: automation().unwrap(),
            thread,
            hwnd: Cell::new(1),
            scale: Cell::new(2.0),
            keyboard: Cell::new(true),
            fresh: Cell::new(false),
            nodes: RefCell::new(things),
            turn: Cell::new(0),
            out: RefCell::default(),
            asked: RefCell::default(),
        })
    }

    /// A `BSTR`, read and freed.
    fn read_text(text: Text) -> String {
        unsafe {
            let read = String::from_utf16_lossy(std::slice::from_raw_parts(text, SysStringLen(text) as usize));
            (automation().unwrap().free_text)(text);
            read
        }
    }
    /// A `VARIANT`, read and freed.
    fn read(value: Variant) -> Value {
        unsafe {
            match value.kind {
                0 => Value::None,
                3 => Value::Number(value.held.number),
                5 => Value::Real(value.held.real),
                8 => Value::Text(read_text(value.held.text)),
                11 => Value::Truth(value.held.truth == -1),
                kind => panic!("a value of the kind {kind}"),
            }
        }
    }

    /// A provider as Windows holds one: one face, one reference, released on drop. Calls go the
    /// way Windows makes them: through the face's vtable.
    struct Held(*mut Face);

    impl Drop for Held {
        fn drop(&mut self) {
            unsafe { (self.table::<Unknown>().release)(self.0) };
        }
    }

    impl Held {
        fn of(tree: &Rc<WindowTree>, id: Id) -> Held {
            Held(Provider::of(tree, id, THING))
        }
        fn table<T>(&self) -> &'static T {
            unsafe { &*(*self.0).table.cast() }
        }
        /// The provider itself, to see its reference count.
        fn provider(&self) -> &Provider {
            unsafe { &*(*self.0).of }
        }
        /// `QueryInterface`.
        fn query(&self, face: usize) -> Result<Held, i32> {
            let mut out = std::ptr::null_mut();
            match unsafe { (self.table::<Unknown>().query)(self.0, &NAMES[face], &mut out) } {
                0 => Ok(Held(out)),
                code => Err(code),
            }
        }
        /// An answer, through one of this face's functions.
        fn get<T>(&self, get: Get<T>) -> Result<T, i32> {
            let mut out = std::mem::MaybeUninit::zeroed();
            match unsafe { get(self.0, out.as_mut_ptr()) } {
                0 => Ok(unsafe { out.assume_init() }),
                code => Err(code),
            }
        }
        fn property(&self, property: i32) -> Result<Value, i32> {
            let (thing, mut out) = (self.query(THING)?, std::mem::MaybeUninit::zeroed());
            match unsafe { (thing.table::<ThingTable>().property)(thing.0, property, out.as_mut_ptr()) } {
                0 => Ok(read(unsafe { out.assume_init() })),
                code => Err(code),
            }
        }
        fn name(&self) -> String {
            match self.property(NAME) {
                Ok(Value::Text(name)) => name,
                other => panic!("a name that is {other:?}"),
            }
        }
        /// The face for a pattern, if supported.
        fn way(&self, pattern: i32) -> Option<Held> {
            let (thing, mut out) = (self.query(THING).unwrap(), std::ptr::null_mut());
            assert_eq!(unsafe { (thing.table::<ThingTable>().pattern)(thing.0, pattern, &mut out) }, 0);
            (!out.is_null()).then(|| Held(out))
        }
        /// The provider in a navigate direction.
        fn go(&self, direction: i32) -> Result<Option<Held>, i32> {
            let (place, mut out) = (self.query(PLACE)?, std::ptr::null_mut());
            match unsafe { (place.table::<PlaceTable>().navigate)(place.0, direction, &mut out) } {
                0 => Ok((!out.is_null()).then(|| Held(out))),
                code => Err(code),
            }
        }
        /// The names of its children, in order.
        fn names_inside(&self) -> Vec<String> {
            let (mut names, mut at) = (Vec::new(), self.go(FIRST).unwrap());
            while let Some(inside) = at {
                names.push(inside.name());
                at = inside.go(NEXT).unwrap();
            }
            names
        }
        /// Its runtime id.
        fn runtime_id(&self) -> Vec<i32> {
            let place = self.query(PLACE).unwrap();
            let row = place.get(place.table::<PlaceTable>().runtime_id).unwrap();
            let mut numbers = vec![0; if row.is_null() { 0 } else { 4 }];
            for at in 0..numbers.len() as i32 {
                assert_eq!(unsafe { SafeArrayGetElement(row, &at, (&raw mut numbers[at as usize]).cast()) }, 0);
            }
            unsafe { SafeArrayDestroy(row) };
            numbers
        }
    }

    /// For the window tests, which see no providers: has the element of a name act on a pattern
    /// as Windows would. Its provider is asked for the pattern's face, and that face's first
    /// function is called: invoke, toggle, select or expand; with a text, set value; with an
    /// amount, set range value. Returns what Windows would get.
    pub(crate) fn act(hwnd: HANDLE, name: &str, pattern: i32, text: Option<&str>, amount: Option<f64>) -> i32 {
        let tree = window_tree(hwnd).expect("a window that was asked what is in it");
        let id = tree.nodes.borrow().iter().find(|node| node.name == name).map(|node| node.id).expect("a thing of that name");
        let way = Held::of(&tree, id).way(pattern).expect("a thing that does this");
        let text = text.map(|text| text.encode_utf16().chain([0]).collect::<Vec<u16>>());
        unsafe {
            match (&text, amount) {
                (Some(text), _) => (way.table::<TypedTable>().put)(way.0, text.as_ptr()),
                (_, Some(amount)) => (way.table::<SlideTable>().put)(way.0, amount),
                _ => (way.table::<PressTable>().press)(way.0),
            }
        }
    }

    /// For the window tests: has the text box of a name select between two characters as
    /// Windows would, through a text range. Returns what Windows would get.
    pub(crate) fn select(hwnd: HANDLE, name: &str, ends: [u32; 2]) -> i32 {
        let tree = window_tree(hwnd).expect("a window that was asked what is in it");
        let id = tree.nodes.borrow().iter().find(|node| node.name == name).map(|node| node.id).expect("a thing of that name");
        let stretch = Kept(Stretch::made(&tree, id, ends));
        unsafe { (stretch.table().select)(stretch.0) }
    }

    #[test]
    fn values_go_to_windows_and_back() {
        let automation = automation().expect("this Windows has UI Automation");
        let values = [Value::None, Value::Number(-7), Value::Real(2.5), Value::Truth(true), Value::Truth(false), Value::Text("späte Töne 𝄞".into()), Value::Text(String::new())];
        for value in values {
            assert_eq!(read(automation.variant(&value)), value);
        }
        // (On 64-bit: a `VARIANT` is 24 bytes.)
        assert_eq!(size_of::<Variant>(), 8 + 2 * size_of::<usize>());
    }

    #[test]
    fn a_window_is_walked_the_way_windows_walks_it() {
        let tree = made_tree(everything());
        let window = Held::of(&tree, 0);
        // The window: its kind comes from Windows' host provider; it has no parent or siblings;
        // its runtime id is Windows' to make.
        assert_eq!((window.property(CONTROL_TYPE), window.property(NAME)), (Ok(Value::None), Ok(Value::None)));
        assert!(window.go(UP).unwrap().is_none() && window.go(NEXT).unwrap().is_none() && window.go(BACK).unwrap().is_none());
        assert_eq!(window.runtime_id(), []);
        // Its children in order: what is in no group; and the group's own.
        assert_eq!(window.names_inside(), ["Sound", "Tools", "", "Your name", "Never", "More", "Dark", "Size", ""]);
        let group = window.go(FIRST).unwrap().unwrap().go(NEXT).unwrap().unwrap();
        assert_eq!((group.name(), group.names_inside()), ("Tools".into(), vec!["Save".to_string(), "Loud".to_string()]));
        // From the group's last child: previous, next (none), parent, and up to the window: the
        // very provider started from.
        let last = group.go(LAST).unwrap().unwrap();
        assert_eq!((last.name(), last.go(BACK).unwrap().unwrap().name(), last.go(NEXT).unwrap().is_none()), ("Loud".into(), "Save".into(), true));
        let up = last.go(UP).unwrap().unwrap();
        assert!(std::ptr::eq(up.provider(), group.provider()) && std::ptr::eq(up.go(UP).unwrap().unwrap().provider(), window.provider()));
        assert_eq!(window.go(LAST).unwrap().unwrap().property(CONTROL_TYPE), Ok(Value::Number(50012)));
        // Each runtime id holds the id and role (the button is role 1).
        assert_eq!((Held::of(&tree, 3).runtime_id(), Held::of(&tree, 0x1_0000_0005).runtime_id()), (vec![3, 3, 0, 1], vec![3, 5, 1, 0]));
        drop((window, group, last, up));
        assert!(tree.out.borrow().is_empty() && Rc::strong_count(&tree) == 1, "every provider was freed when the last let go of it");
    }

    #[test]
    fn each_kind_of_thing_is_what_windows_calls_it_and_does_what_it_can() {
        // A link is a hyperlink: invokable and focusable.
        let linked = made_tree(vec![thing(1, Role::Link, "Read more", None)]);
        let link = Held::of(&linked, 1);
        assert_eq!((link.property(CONTROL_TYPE), link.way(TO_PRESS).is_some(), link.way(TO_TICK).is_some()), (Ok(Value::Number(50005)), true, false));
        assert_eq!((link.property(TAKES_KEYBOARD), link.name()), (Ok(Value::Truth(true)), "Read more".to_string()));
        drop(link);
        let tree = made_tree(everything());
        // Control type, patterns, and focusable.
        let expected = [
            (1, 50020, vec![], false),
            (2, 50026, vec![], false),
            (3, 50000, vec![TO_PRESS], true),
            (4, 50002, vec![TO_TICK], true),
            (5, 50015, vec![TO_SLIDE], true),
            (6, 50004, vec![TO_TYPE], true),
            (7, 50013, vec![TO_PICK], false),
            (8, 50000, vec![TO_FOLD], true),
            (9, 50011, vec![TO_PRESS, TO_TICK], true),
            (10, 50035, vec![TO_PRESS], true),
            (11, 50012, vec![TO_SLIDE], false),
        ];
        for (id, control, ways, keys) in expected {
            let thing = Held::of(&tree, id);
            let has: Vec<i32> = [TO_PRESS, TO_TYPE, TO_SLIDE, TO_FOLD, TO_PICK, TO_TICK].into_iter().filter(|pattern| thing.way(*pattern).is_some()).collect();
            assert_eq!((thing.property(CONTROL_TYPE), has, thing.property(TAKES_KEYBOARD)), (Ok(Value::Number(control)), ways, Ok(Value::Truth(keys))), "the thing {id}");
            assert_eq!((thing.property(ENABLED), thing.property(AUTOMATION_ID)), (Ok(Value::Truth(id != 7)), Ok(Value::Text(format!("{id:016x}")))));
        }
        // States, read through the pattern faces.
        let on = |id: Id, pattern: i32, state: fn(&Held) -> Get<i32>| {
            let way = Held::of(&tree, id).way(pattern).unwrap();
            way.get(state(&way))
        };
        assert_eq!(on(4, TO_TICK, |way| way.table::<TickTable>().ticked), Ok(1));
        assert_eq!(on(9, TO_TICK, |way| way.table::<TickTable>().ticked), Ok(1));
        assert_eq!(on(7, TO_PICK, |way| way.table::<ChoiceTable>().picked), Ok(0));
        assert_eq!(on(8, TO_FOLD, |way| way.table::<FoldTable>().unfolded), Ok(0));
        let typed = Held::of(&tree, 6).way(TO_TYPE).unwrap();
        assert_eq!((typed.get(typed.table::<TypedTable>().value).map(read_text), typed.get(typed.table::<TypedTable>().read_only)), (Ok("Ada".into()), Ok(0)));
        let amounts = |id: Id| {
            let (slide, table) = (Held::of(&tree, id).way(TO_SLIDE).unwrap(), &AS_SLIDE);
            [table.value, table.least, table.most, table.small_step, table.large_step]
                .map(|get| slide.get(get).unwrap())
                .into_iter()
                .chain([slide.get(table.read_only).unwrap() as f64])
                .collect::<Vec<_>>()
        };
        assert_eq!((amounts(5), amounts(11)), (vec![4.0, 0.0, 10.0, 0.5, 0.5, 0.0], vec![0.5, 0.0, 1.0, 0.0, 0.0, 1.0]));
        // A menu row neither checked nor unchecked is only invoked.
        tree.nodes.borrow_mut()[8].on = None;
        assert!(Held::of(&tree, 9).way(TO_TICK).is_none() && Held::of(&tree, 9).way(TO_PRESS).is_some());
    }

    #[test]
    fn a_provider_says_where_its_thing_is_and_the_window_what_is_at_a_place() {
        let tree = made_tree(everything());
        let (window, button) = (Held::of(&tree, 0).query(WINDOW).unwrap(), Held::of(&tree, 3).query(PLACE).unwrap());
        // In pixels from the window corner (for a missing window, the screen's): the button
        // lies at 60, 10 and is 16 by 8 units.
        assert_eq!(button.get(button.table::<PlaceTable>().bounds), Ok([120.0, 20.0, 32.0, 16.0]));
        let at = |x: f64, y: f64| {
            let mut out = std::ptr::null_mut();
            assert_eq!(unsafe { (window.table::<WindowTable>().at_point)(window.0, x, y, &mut out) }, 0);
            Held(out).provider().id
        };
        // On the button, in the group; on the group beside the button; on nothing, which is the
        // window.
        assert_eq!((at(130.0, 30.0), at(114.0, 14.0), at(900.0, 900.0), at(-5.0, 30.0)), (3, 2, 0, 0));
        // Focus: nothing, then the slider.
        let focused = || window.get(window.table::<WindowTable>().focused).map(|on| (!on.is_null()).then(|| Held(on).provider().id));
        assert_eq!((focused(), Held::of(&tree, 5).property(HAS_KEYBOARD)), (Ok(None), Ok(Value::Truth(false))));
        tree.nodes.borrow_mut()[4].focused = true;
        assert_eq!((focused(), Held::of(&tree, 5).property(HAS_KEYBOARD)), (Ok(Some(5)), Ok(Value::Truth(true))));
        // (While another window has focus, the focused element is still reported.)
        tree.keyboard.set(false);
        assert_eq!((focused(), Held::of(&tree, 5).property(HAS_KEYBOARD)), (Ok(Some(5)), Ok(Value::Truth(false))));
        // Elements and the window are fragments; only the window answers hit tests.
        assert_eq!((button.query(WINDOW).err(), window.query(PLACE).is_ok(), button.get(button.table::<PlaceTable>().root).map(|root| Held(root).provider().id)), (Some(NO_INTERFACE), true, Ok(0)));
    }

    #[test]
    fn one_provider_stands_for_a_thing_and_is_counted() {
        let tree = made_tree(everything());
        let (first, again) = (Held::of(&tree, 3), Held::of(&tree, 3));
        assert!(std::ptr::eq(first.provider(), again.provider()) && first.provider().held.get() == 2 && tree.out.borrow().len() == 1);
        // Asked for `IUnknown` through any face, it returns the same one; and it refuses an
        // unknown IID.
        let unknown = |face: &Held| face.query(FACES).unwrap().0;
        let press = first.way(TO_PRESS).unwrap();
        assert!(unknown(&first) == unknown(&press) && unknown(&press) == first.0 && press.0 != first.0);
        let mut out = std::ptr::null_mut();
        assert_eq!(unsafe { (first.table::<Unknown>().query)(first.0, &[0x10e, 0, 0xC0, 0x4600_0000], &mut out) }, NO_INTERFACE);
        assert!(out.is_null() && first.provider().held.get() == 3);
        drop((again, press));
        assert_eq!((first.provider().held.get(), unsafe { (first.table::<Unknown>().hold)(first.0) }, unsafe { (first.table::<Unknown>().release)(first.0) }), (1, 2, 1));
        drop(first);
        assert!(tree.out.borrow().is_empty());
    }

    #[test]
    fn a_provider_whose_thing_or_window_is_gone_says_so() {
        let tree = made_tree(everything());
        let (window, button, tick) = (Held::of(&tree, 0), Held::of(&tree, 3), Held::of(&tree, 4));
        let (press, place) = (button.way(TO_PRESS).unwrap(), button.query(PLACE).unwrap());
        assert_eq!(button.name(), "Save");
        // The app shows another page: the button's id is now a slider's, and the check box is
        // gone.
        *tree.nodes.borrow_mut() = vec![thing(1, Role::Text, "Another page", None), AccessNode { value_range: Some([1.0, 0.0, 2.0]), ..thing(3, Role::Slider, "", None) }];
        for gone in [&button, &tick] {
            assert_eq!((gone.property(NAME), gone.go(UP).err(), gone.go(FIRST).err()), (Err(GONE), Some(GONE), Some(GONE)));
        }
        assert_eq!(
            (unsafe { (press.table::<PressTable>().press)(press.0) }, place.get(place.table::<PlaceTable>().bounds), unsafe { (place.table::<PlaceTable>().focus)(place.0) }),
            (GONE, Err(GONE), GONE)
        );
        // Its runtime id stays the same; what is there now is another element.
        let slider = Held::of(&tree, 3);
        assert!(!std::ptr::eq(slider.provider(), button.provider()));
        assert_eq!((button.runtime_id(), slider.runtime_id(), slider.property(CONTROL_TYPE)), (vec![3, 3, 0, Role::Button as i32], vec![3, 3, 0, Role::Slider as i32], Ok(Value::Number(50015))));
        assert_eq!(window.names_inside(), ["Another page", ""]);
        // The window is destroyed: whatever is asked, it is gone.
        tree.hwnd.set(0);
        assert_eq!((window.go(FIRST).err(), window.property(NAME), slider.property(NAME)), (Some(GONE), Err(GONE), Err(GONE)));
        let root = window.query(WINDOW).unwrap();
        assert_eq!(root.get(root.table::<WindowTable>().focused).map(|on| on.is_null()), Ok(true));
        let mut out = std::ptr::null_mut();
        assert_eq!((unsafe { (root.table::<WindowTable>().at_point)(root.0, 1.0, 1.0, &mut out) }, out.is_null()), (GONE, true));
        drop((window, button, tick, press, place, slider, root));
        assert!(tree.out.borrow().is_empty() && Rc::strong_count(&tree) == 1);
    }

    #[test]
    fn what_is_asked_of_a_thing_goes_to_its_window_and_no_further() {
        let tree = made_tree(everything());
        let act = |id: Id, pattern: i32, act: fn(&Held) -> Do| {
            let way = Held::of(&tree, id).way(pattern).unwrap();
            unsafe { act(&way)(way.0) }
        };
        // The missing window cannot act: nothing done, nothing left asked.
        assert_eq!(act(3, TO_PRESS, |way| way.table::<PressTable>().press), NOT_NOW);
        assert_eq!(act(4, TO_TICK, |way| way.table::<TickTable>().tick), NOT_NOW);
        assert_eq!(act(8, TO_FOLD, |way| way.table::<FoldTable>().unfold), NOT_NOW);
        assert!(tree.asked.borrow().is_none());
        // What is already as asked is left alone, at once.
        assert_eq!(act(8, TO_FOLD, |way| way.table::<FoldTable>().fold), 0);
        tree.nodes.borrow_mut()[6].on = Some(true);
        assert_eq!(act(7, TO_PICK, |way| way.table::<ChoiceTable>().pick), 0);
        // One of a single-selection set cannot be removed; an unselected one needs no removing.
        assert_eq!(act(7, TO_PICK, |way| way.table::<ChoiceTable>().remove), NOT_DONE);
        tree.nodes.borrow_mut()[6].on = Some(false);
        assert_eq!(act(7, TO_PICK, |way| way.table::<ChoiceTable>().remove), 0);
        // Disabled elements do not act (the window is not even asked).
        assert_eq!(act(7, TO_PICK, |way| way.table::<ChoiceTable>().pick), NOT_NOW);
        // A slider is not set past its ends; a progress bar not at all.
        let slide = |id: Id, to: f64| {
            let way = Held::of(&tree, id).way(TO_SLIDE).unwrap();
            unsafe { (way.table::<SlideTable>().put)(way.0, to) }
        };
        assert_eq!((slide(5, 11.0), slide(5, -1.0), slide(5, 4.0), slide(5, 6.0), slide(11, 0.7)), (NO_SUCH, NO_SUCH, 0, NOT_NOW, NOT_DONE));
        let typed = Held::of(&tree, 6).way(TO_TYPE).unwrap();
        let text: Vec<u16> = "Grace".encode_utf16().chain([0]).collect();
        assert_eq!(unsafe { ((typed.table::<TypedTable>().put)(typed.0, text.as_ptr()), (typed.table::<TypedTable>().put)(typed.0, std::ptr::null())) }, (NOT_NOW, NOT_NOW));
    }

    #[test]
    fn the_list_is_kept_for_as_long_as_anybody_wants_it() {
        let tree = made_tree(everything());
        let mut reading = Reading { windows: vec![tree.clone()], ours: true };
        // A provider is held: the list is wanted, and Windows is not asked.
        let held = Held::of(&tree, 3);
        assert!(!reading.left(|| panic!("Windows is not asked while a provider is held")));
        drop(held);
        // None held, but Windows says a client listens.
        assert!(!reading.left(|| true) && reading.ours && !tree.nodes.borrow().is_empty());
        // None held and nobody listens: the list and its copy go. Reported once.
        assert!(reading.left(|| false) && !reading.ours && tree.nodes.borrow().is_empty() && tree.fresh.get());
        assert!(!reading.left(|| panic!("a list that is not kept for Windows is not Windows' to stop")));
    }

    #[test]
    fn what_only_some_things_have_is_told_of_those() {
        let tree = made_tree(vec![
            AccessNode { shortcut: "Ctrl+O".into(), ..thing(1, Role::MenuItem, "Open", None) },
            AccessNode { position: Some([2, 5]), on: Some(true), ..thing(2, Role::Tab, "Two", None) },
            thing(3, Role::PasswordBox, "Password", None),
            thing(4, Role::Dialog, "Sure?", None),
            AccessNode { on: Some(true), ..thing(5, Role::Switch, "Loud", None) },
            AccessNode { on: Some(false), value: "Roomy".into(), ..thing(6, Role::ComboBox, "", None) },
        ]);
        let of = |id: Id, property: i32| Held::of(&tree, id).property(property);
        // A menu row's accelerator; a tab's position in set; a password box; a dialog. And
        // nothing for elements without these.
        assert_eq!((of(1, KEYS), of(2, KEYS)), (Ok(Value::Text("Ctrl+O".into())), Ok(Value::None)));
        assert_eq!((of(2, NTH), of(2, OF_SO_MANY), of(1, NTH)), (Ok(Value::Number(2)), Ok(Value::Number(5)), Ok(Value::None)));
        assert_eq!((of(3, HIDES_TEXT), of(1, HIDES_TEXT), of(4, IS_DIALOG), of(3, IS_DIALOG)), (Ok(Value::Truth(true)), Ok(Value::Truth(false)), Ok(Value::Truth(true)), Ok(Value::Truth(false))));
        // Control types: menu item, tab item, edit, pane, button (a switch is a toggled
        // button), combo box.
        assert_eq!([1, 2, 3, 4, 5, 6].map(|id| of(id, CONTROL_TYPE)), [50011, 50019, 50004, 50033, 50000, 50003].map(|kind| Ok(Value::Number(kind))));
        // A tab is selectable, a switch toggles, a combo box expands and reports its choice,
        // read-only.
        let does = |id: Id, pattern: i32| Held::of(&tree, id).way(pattern).is_some();
        assert!(does(2, TO_PICK) && does(5, TO_TICK) && does(6, TO_FOLD) && does(6, TO_TYPE) && !does(4, TO_PRESS) && !does(5, TO_PICK));
    }

    /// A text range as Windows holds one: one reference, released on drop, called through its
    /// vtable.
    struct Kept(*mut Stretch);

    impl Drop for Kept {
        fn drop(&mut self) {
            unsafe { (self.table().release)(self.0) };
        }
    }

    impl Kept {
        fn table(&self) -> &'static StretchTable {
            unsafe { &*(*self.0).table }
        }
        fn ends(&self) -> [u32; 2] {
            unsafe { (*self.0).ends.get() }
        }
        fn text(&self) -> Result<String, i32> {
            let mut out = std::ptr::null_mut();
            match unsafe { (self.table().text)(self.0, -1, &mut out) } {
                0 => Ok(read_text(out)),
                code => Err(code),
            }
        }
        fn copy(&self) -> Kept {
            let mut out = std::ptr::null_mut();
            assert_eq!(unsafe { (self.table().copy)(self.0, &mut out) }, 0);
            Kept(out)
        }
        /// Expands to the unit it starts in, and returns its text.
        fn widened(&self, unit: i32) -> String {
            assert_eq!(unsafe { (self.table().widen)(self.0, unit) }, 0);
            self.text().unwrap()
        }
        /// Moves by `count` units: how many it moved, and its text.
        fn gone(&self, unit: i32, count: i32) -> (i32, String) {
            let mut went = 0;
            assert_eq!(unsafe { (self.table().go)(self.0, unit, count, &mut went) }, 0);
            (went, self.text().unwrap())
        }
        fn end_gone(&self, end: i32, unit: i32, count: i32) -> (i32, String) {
            let mut went = 0;
            assert_eq!(unsafe { (self.table().move_end)(self.0, end, unit, count, &mut went) }, 0);
            (went, self.text().unwrap())
        }
        fn found(&self, text: &str, backward: bool, any_case: bool) -> Option<Kept> {
            let (text, mut out) = (text.encode_utf16().chain([0]).collect::<Vec<u16>>(), std::ptr::null_mut());
            assert_eq!(unsafe { (self.table().find_text)(self.0, text.as_ptr(), backward as i32, any_case as i32, &mut out) }, 0);
            (!out.is_null()).then(|| Kept(out))
        }
        /// How many numbers its rectangles have, and the first.
        fn lies(&self) -> (usize, f64) {
            let (mut row, mut first) = (std::ptr::null_mut(), 0.0f64);
            assert_eq!(unsafe { (self.table().lies)(self.0, &mut row) }, 0);
            let count = (0..5).take_while(|at: &i32| unsafe { SafeArrayGetElement(row, at, (&raw mut first).cast()) } == 0).count();
            unsafe { SafeArrayGetElement(row, &0, (&raw mut first).cast()) };
            unsafe { SafeArrayDestroy(row) };
            (count, first)
        }
    }

    /// A text box with two lines, its second word selected backward.
    fn notes() -> AccessNode {
        AccessNode { value: "one two\nthree".into(), caret: Some([7, 4]), focused: true, ..thing(6, Role::TextBox, "Notes", None) }
    }

    #[test]
    fn a_text_is_read_by_its_lines_as_wrapped_and_its_letters_where_drawn() {
        // "one two\nthree" wrapped after "one ": three lines of letters 10 wide and 20 high,
        // "three" spelt wrong.
        let line_of = |letter: usize| [0, 4, 8].iter().rposition(|start| *start <= letter).unwrap();
        let places = (0..13).map(|letter| rect(((letter - [0, 4, 8][line_of(letter)]) * 10) as f32, line_of(letter) as f32 * 20.0, 10.0, 20.0)).collect();
        let letters = Some(Arc::new(Letters { key: 0, lines: vec![0, 4, 8], places }));
        let (area, wrong) = (rect(0.0, 0.0, 200.0, 60.0), vec![[8, 13]]);
        let node = AccessNode { letters, text_rect: area, rect: area, seen: area, misspelled: wrong, ..notes() };
        let tree = made_tree(vec![node]);
        let text = Held::of(&tree, 6).way(10014).unwrap();
        // Lines as wrapped; paragraphs as typed.
        let piece = Kept(Stretch::made(&tree, 6, [1, 1]));
        assert_eq!([piece.widened(3), piece.gone(3, 1).1, piece.gone(3, 1).1, piece.widened(4)], ["one ", "two\n", "three", "three"]);
        // A range over two lines lies in two rectangles (at two pixels per unit).
        let mut row = std::ptr::null_mut();
        let span = Kept(Stretch::made(&tree, 6, [2, 6]));
        assert_eq!(unsafe { (span.table().lies)(span.0, &mut row) }, 0);
        let numbers: Vec<f64> = (0..)
            .map_while(|at: i32| {
                let mut number = 0.0f64;
                (unsafe { SafeArrayGetElement(row, &at, (&raw mut number).cast()) } == 0).then_some(number)
            })
            .collect();
        unsafe { SafeArrayDestroy(row) };
        assert_eq!(numbers, [40.0, 0.0, 40.0, 40.0, 0.0, 40.0, 40.0, 40.0]);
        // The letter at a point, as an empty range before it.
        let mut at = std::ptr::null_mut();
        assert_eq!(unsafe { (text.table::<TextTable>().at_spot)(text.0, Spot { x: 10.0, y: 100.0 }, &mut at) }, 0);
        assert_eq!(Kept(at).ends(), [8, 8]);
        // The misspelled word is found, asked for as a number or as an array of them.
        let whole = Kept(Stretch::made(&tree, 6, [0, 13]));
        let sought = Variant { kind: 3, unused: [0; 3], held: super::Held { number: SPELT_WRONG } };
        let mut found = std::ptr::null_mut();
        assert_eq!(unsafe { (whole.table().find_look)(whole.0, MARKS, sought, 0, &mut found) }, 0);
        assert_eq!(Kept(found).ends(), [8, 13]);
        let other = Variant { held: super::Held { number: 60002 }, ..sought };
        assert_eq!(unsafe { ((whole.table().find_look)(whole.0, MARKS, other, 1, &mut found), found.is_null()) }, (0, true));
        let array = unsafe { (tree.automation.numbers)(3, 0, 2) };
        for (at, number) in [(0, 60002), (1, SPELT_WRONG)] {
            unsafe { (tree.automation.put)(array, &at, (&raw const number).cast()) };
        }
        let among = Variant { kind: 0x2003, unused: [0; 3], held: super::Held { object: array } };
        assert_eq!(unsafe { (whole.table().find_look)(whole.0, MARKS, among, 1, &mut found) }, 0);
        assert_eq!(Kept(found).ends(), [8, 13]);
        unsafe { SafeArrayDestroy(array) };
        tree.hwnd.set(0);
    }

    #[test]
    fn what_is_out_of_view_is_told_as_off_the_screen_and_can_be_brought_into_view() {
        let mut things = everything();
        things[4].visible = false;
        let tree = made_tree(things);
        // The slider is scrolled out of view, the button is not.
        assert_eq!((Held::of(&tree, 5).property(OFF_THE_SCREEN), Held::of(&tree, 3).property(OFF_THE_SCREEN)), (Ok(Value::Truth(true)), Ok(Value::Truth(false))));
        // Anything can be scrolled into view, slider and text alike: the window is asked, which
        // here is missing, so "not now".
        for id in [5, 1] {
            let way = Held::of(&tree, id).way(10017).expect("anything is scrolled into view");
            assert_eq!(unsafe { (way.table::<ScrollItemTable>().show)(way.0) }, NOT_NOW);
        }
        tree.hwnd.set(0);
    }

    #[test]
    fn words_spelt_wrong_are_marked_where_a_stretch_of_text_is_asked_about() {
        // "the qwrty and zxcvb is": two misspelled words, 4 to 9 and 14 to 19.
        let wrong = [[4, 9], [14, 19]];
        let text: Vec<char> = "the qwrty and zxcvb is".chars().collect();
        // A range inside one is annotated; one touching none is not; one partly in one is mixed
        // (`None`); an empty one is as the next letter.
        let marks = [[5, 8], [4, 9], [0, 3], [9, 14], [2, 6], [8, 15], [4, 4], [9, 9]].map(|ends| marked(&wrong, ends));
        let (spelt, plain): (Option<&[i32]>, Option<&[i32]>) = (Some(&[SPELT_WRONG]), Some(&[]));
        assert_eq!(marks, [spelt, spelt, plain, plain, None, None, spelt, plain]);
        // By format units the text is five pieces: around the two words.
        let marks = Marks { wrong: wrong.to_vec(), ..Marks::default() };
        let starts_of = (0..=text.len()).filter(|at| starts(&text, 1, *at, &marks)).collect::<Vec<_>>();
        assert_eq!(starts_of, [0, 4, 9, 14, 19, 22]);
        // And so a screen reader is told, through a text box's range.
        let tree = made_tree(vec![AccessNode { value: text.iter().collect(), misspelled: wrong.to_vec(), ..thing(6, Role::TextBox, "Letter", None) }]);
        let look = |ends: [u32; 2]| {
            let (stretch, mut look) = (Kept(Stretch::made(&tree, 6, ends)), std::mem::MaybeUninit::<Variant>::zeroed());
            assert_eq!(unsafe { (stretch.table().look)(stretch.0, MARKS, look.as_mut_ptr()) }, 0);
            // (An array, read and freed; or the mixed value, released, returned as minus its
            // type.)
            let (look, mut kinds) = (unsafe { look.assume_init() }, [0i32; 2]);
            let held = unsafe { look.held.object };
            if look.kind != 0x2003 {
                unsafe { ((**held.cast::<*const Unknown>()).release)(held.cast()) };
                return vec![-(look.kind as i32)];
            }
            let count = (0..2).filter(|at| unsafe { SafeArrayGetElement(held, at, (&raw mut kinds[*at as usize]).cast()) } == 0).count();
            unsafe { SafeArrayDestroy(held) };
            kinds[..count].to_vec()
        };
        assert_eq!([look([5, 8]), look([0, 3]), look([2, 6])], [vec![SPELT_WRONG], vec![], vec![-13]], "marked, not marked, some of each");
        tree.hwnd.set(0);
    }

    #[test]
    fn a_text_is_gone_through_by_the_letter_the_word_and_the_line() {
        let text: Vec<char> = "one  two\nthree".chars().collect();
        // (Its lines as laid out: as typed.)
        let lines = Marks { lines: vec![0, 9], ..Marks::default() };
        let all = |unit: i32| (0..=text.len()).filter(|at| starts(&text, unit, *at, &lines)).collect::<Vec<_>>();
        // Every position starts a character; a word starts after whitespace, which belongs to
        // the word before; a line after a line break; the document once.
        assert_eq!((all(0).len(), all(2)), (15, vec![0, 5, 9, 14]));
        assert_eq!((all(3), all(4), all(6), all(1)), (vec![0, 9, 14], vec![0, 9, 14], vec![0, 14], vec![0, 14]));
        assert_eq!((start_before(&text, 2, 6, &lines), start_after(&text, 2, 6, &lines), start_before(&text, 3, 8, &lines), start_after(&text, 3, 9, &lines)), (5, 9, 0, 14));
        // Moving on and back by units, as far as there are, and how many.
        let gone = [
            gone_by(&text, 2, 0, 2, &lines),
            gone_by(&text, 2, 0, 9, &lines),
            gone_by(&text, 0, 14, -3, &lines),
            gone_by(&text, 3, 10, -1, &lines),
            gone_by(&text, 3, 9, -1, &lines),
            gone_by(&text, 2, 7, 0, &lines),
        ];
        assert_eq!(gone, [(9, 2), (14, 3), (11, -3), (9, -1), (0, -1), (7, 0)]);
        assert_eq!((gone_by(&[], 2, 0, 1, &lines), gone_by(&text, 0, 99, -1, &lines)), ((0, 0), (13, -1)));
    }

    #[test]
    fn a_text_box_is_read_piece_by_piece() {
        const TO_READ: i32 = 10014;
        let tree = made_tree(vec![thing(1, Role::Button, "Save", None), notes()]);
        // Only text boxes have the text pattern.
        assert!(Held::of(&tree, 1).way(TO_READ).is_none());
        let text = Held::of(&tree, 6).way(TO_READ).expect("a text box is read piece by piece");
        let table = text.table::<TextTable>();
        assert_eq!(text.get(table.selects), Ok(1));
        // All its text; and its selection, as an array of one range.
        let all = Kept(text.get(table.all).unwrap());
        assert_eq!((all.text(), all.ends()), (Ok("one two\nthree".to_string()), [0, 13]));
        let (row, mut selected) = (text.get(table.selection).unwrap(), std::ptr::null_mut::<Stretch>());
        assert_eq!(unsafe { SafeArrayGetElement(row, &0, (&raw mut selected).cast()) }, 0);
        unsafe { SafeArrayDestroy(row) };
        let selected = Kept(selected);
        assert_eq!((selected.text(), selected.ends()), (Ok("two".to_string()), [4, 7]));

        // A copy expands to its line, then its word; moves a word, a character, and to the last
        // line, as far as lines go.
        let piece = selected.copy();
        assert_eq!([piece.widened(3), piece.widened(2), piece.widened(6)], ["one two\n", "one ", "one two\nthree"]);
        piece.widened(2);
        assert_eq!([piece.gone(2, 1), piece.gone(0, 1), piece.gone(3, 5), piece.gone(3, -1)], [(1, "two\n".into()), (1, "w".into()), (1, "three".into()), (-1, "one two\n".into())]);
        assert_eq!((piece.gone(2, 0), selected.text()), ((0, "one two\n".into()), Ok("two".into())), "a copy goes its own way");
        // An empty range moves as a caret: by positions, to the very end.
        let cursor = Kept(Stretch::made(&tree, 6, [0, 0]));
        assert_eq!([cursor.gone(0, 2), cursor.gone(2, 1), cursor.gone(3, 9)], [(2, String::new()), (1, String::new()), (2, String::new())]);
        assert_eq!(cursor.ends(), [13, 13]);

        // One endpoint moves alone, taking the other along when it passes.
        let whole = all.copy();
        assert_eq!([whole.end_gone(1, 2, -1), whole.end_gone(0, 0, 4), whole.end_gone(1, 3, -5)], [(-1, "one two\n".into()), (4, "two\n".into()), (-1, String::new())]);
        assert_eq!(whole.ends(), [0, 0]);
        // Endpoints compared, and one moved to another.
        let (mut far, mut same) = (0, 0);
        unsafe {
            assert_eq!((all.table().compare_ends)(all.0, 1, selected.0, 0, &mut far), 0);
            assert_eq!((whole.table().move_end_to)(whole.0, 1, selected.0, 1), 0);
            assert_eq!((selected.table().same)(selected.0, selected.copy().0, &mut same), 0);
        }
        assert_eq!((far, whole.text(), same), (9, Ok("one two".into()), 1));

        // Text found from the front or back, case-insensitive or not.
        let found = |text: &str, backward: bool, any_case: bool| all.found(text, backward, any_case).map(|found| found.ends());
        assert_eq!(
            [found("TWO", false, true), found("TWO", false, false), found("e", false, false), found("e", true, false), found("", false, false)],
            [Some([4, 7]), None, Some([2, 3]), Some([12, 13]), None]
        );
        assert_eq!(selected.found("three", false, false).map(|found| found.ends()), None, "only in the stretch that is asked");
        // Its rectangle is its box's (at two pixels per unit), where its letters are not known;
        // an empty one's, the cursor's.
        assert_eq!((selected.lies(), cursor.lies().0), ((4, 240.0), 4));
        // Attributes: read-only is known; all else is unsupported.
        let mut looks = [std::mem::MaybeUninit::<Variant>::zeroed(); 2];
        unsafe {
            assert_eq!((selected.table().look)(selected.0, READ_ONLY, looks[0].as_mut_ptr()), 0);
            assert_eq!((selected.table().look)(selected.0, 40008, looks[1].as_mut_ptr()), 0);
            assert_eq!((read(looks[0].assume_init()), looks[1].assume_init().kind), (Value::Truth(false), 13));
        }
        // Its enclosing element is its box; it can be selected, not added to the selection.
        let mut within = std::ptr::null_mut();
        assert_eq!(unsafe { (selected.table().within)(selected.0, &mut within) }, 0);
        assert_eq!(Held(within).name(), "Notes");
        assert_eq!(unsafe { ((selected.table().add)(selected.0), (selected.table().remove)(selected.0), (selected.table().show)(selected.0, 1)) }, (NOT_DONE, NOT_DONE, 0));
        // (Selecting asks the window, which is missing.)
        assert_eq!(unsafe { (selected.table().select)(selected.0) }, NOT_NOW);

        // The text changes under a range: it reads what is there now, within its ends.
        tree.nodes.borrow_mut()[1].value = "one".into();
        assert_eq!((all.text(), selected.text(), all.ends()), (Ok("one".into()), Ok(String::new()), [0, 13]));
        // Once the box is gone, every range of it says so.
        tree.nodes.borrow_mut().truncate(1);
        assert_eq!((all.text(), unsafe { (all.table().widen)(all.0, 0) }), (Err(GONE), GONE));
        drop((text, all, selected, piece, cursor, whole));
        assert!(tree.out.borrow().is_empty() && Rc::strong_count(&tree) == 1, "every stretch was freed when the last let go of it");
    }

    /// Held by a test in which Windows' own UI Automation client asks, while it does: one
    /// program's client serves one such thread at a time (a second is refused with "failed").
    static WINDOWS_ASKS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The names of the providers in an array Windows got.
    fn names_in(row: *mut c_void) -> Vec<String> {
        let mut names = Vec::new();
        for at in 0.. {
            let mut face = std::ptr::null_mut::<Face>();
            if unsafe { SafeArrayGetElement(row, &at, (&raw mut face).cast()) } != 0 {
                break;
            }
            names.push(Held(face).name());
        }
        unsafe { SafeArrayDestroy(row) };
        names
    }

    #[test]
    fn a_table_is_told_as_rows_and_columns_under_headings() {
        const TO_GO_THROUGH: i32 = 10006;
        const TO_PLACE: i32 = 10007;
        const UNDER_HEADINGS: i32 = 10012;
        const UNDER_A_HEADING: i32 = 10013;
        let tree = made_tree(ledger());
        let (table, size) = (Held::of(&tree, 1), Held::of(&tree, 3));
        // Control types: table, data items (rows and cells), header item; and their patterns.
        let kinds = [1, 2, 4, 5, 9, 10].map(|id| Held::of(&tree, id).property(CONTROL_TYPE));
        assert_eq!(kinds, [50036, 50035, 50029, 50029, 50029, 50000].map(|kind| Ok(Value::Number(kind))));
        let ways = |id: Id| [TO_GO_THROUGH, UNDER_HEADINGS, TO_PLACE, UNDER_A_HEADING, TO_PICK].map(|pattern| Held::of(&tree, id).way(pattern).is_some());
        assert_eq!([ways(1), ways(5), ways(4)], [[true, true, false, false, false], [false, false, true, true, false], [false, false, false, false, true]]);
        // A selected row takes focus, and reports selected and its position.
        let row = Held::of(&tree, 4);
        assert_eq!((row.property(TAKES_KEYBOARD), row.property(NTH), row.property(OF_SO_MANY)), (Ok(Value::Truth(true)), Ok(Value::Number(11)), Ok(Value::Number(40))));
        assert_eq!(row.way(TO_PICK).map(|way| way.get(way.table::<ChoiceTable>().picked)), Some(Ok(1)));
        // The table lists its selected rows, and allows many; a row is selected among them.
        let picks = table.way(10001).unwrap();
        assert_eq!((names_in(picks.get(picks.table::<SelectionTable>().selection).unwrap()), picks.get(picks.table::<SelectionTable>().many)), (vec!["a.txt, 3 KB".to_string()], Ok(1)));
        let among = row.way(TO_PICK).map(|way| Held(way.get(way.table::<ChoiceTable>().among).unwrap()));
        assert!(among.is_some_and(|among| std::ptr::eq(among.provider(), table.provider())));
        // The table: row and column counts; a cell by row and column. (A cell of a row out of
        // view is shown first; here, with no window, it stays unavailable. One of a row or
        // column it lacks is an invalid argument.)
        let grid = table.way(TO_GO_THROUGH).unwrap();
        let (rows, columns) = (grid.get(grid.table::<GridTable>().rows), grid.get(grid.table::<GridTable>().columns));
        let item = |row: i32, column: i32| {
            let mut out = std::ptr::null_mut();
            let code = unsafe { (grid.table::<GridTable>().item)(grid.0, row, column, &mut out) };
            (code, (!out.is_null()).then(|| Held(out).name()))
        };
        assert_eq!((rows, columns), (Ok(40), Ok(2)));
        assert_eq!(
            [item(10, 1), item(11, 0), item(0, 0), item(40, 0), item(1, 2), item(-1, 0)],
            [(0, Some("3 KB".into())), (0, Some("b.txt".into())), (0, None), (NO_SUCH, None), (NO_SUCH, None), (NO_SUCH, None)]
        );
        // Its column headings in order; no row headings; row major.
        let headed = table.way(UNDER_HEADINGS).unwrap();
        let headings = |get: fn(&Held) -> Get<*mut c_void>, from: &Held| names_in(from.get(get(from)).unwrap());
        assert_eq!(headings(|way| way.table::<HeadedTable>().column_headings, &headed), ["Name", "Size"]);
        assert_eq!((headings(|way| way.table::<HeadedTable>().row_headings, &headed), headed.get(headed.table::<HeadedTable>().by_rows)), (Vec::<String>::new(), Ok(0)));
        // A cell: its row and column, spans of one, its table, its heading.
        let cell = Held::of(&tree, 9);
        let place = cell.way(TO_PLACE).unwrap();
        let numbers = [
            place.get(place.table::<InGridTable>().row),
            place.get(place.table::<InGridTable>().column),
            place.get(place.table::<InGridTable>().rows_spanned),
            place.get(place.table::<InGridTable>().columns_spanned),
        ];
        assert_eq!(numbers, [Ok(11), Ok(1), Ok(1), Ok(1)]);
        let in_grid = Held(place.get(place.table::<InGridTable>().grid).unwrap());
        assert!(std::ptr::eq(in_grid.provider(), table.provider()));
        let under = cell.way(UNDER_A_HEADING).unwrap();
        assert_eq!(headings(|way| way.table::<UnderHeadingTable>().column_headings, &under), ["Size"]);
        assert_eq!(headings(|way| way.table::<UnderHeadingTable>().row_headings, &under), Vec::<String>::new());
        // The button's parent is the cell, the cell's the row, the row's the table.
        let up = |held: &Held| held.go(UP).unwrap().unwrap().name();
        let button = Held::of(&tree, 10);
        assert_eq!([up(&button), up(&cell), up(&Held::of(&tree, 7))], ["Open", "b.txt, Open", ""]);
        // The sorted column's heading says its direction, in words and as ARIA.
        let name = Held::of(&tree, 2);
        assert_eq!((name.property(STATUS), name.property(ARIA)), (Ok(Value::Text("sorted ascending".into())), Ok(Value::Text("sort=ascending".into()))));
        assert_eq!((size.property(STATUS), size.property(ARIA)), (Ok(Value::None), Ok(Value::None)));
        // A cell no longer in the table (its row scrolled out) is gone.
        tree.nodes.borrow_mut().truncate(6);
        assert_eq!((place.get(place.table::<InGridTable>().row), item(11, 0)), (Err(GONE), (0, None)));
        drop((table, size, kinds, row, picks, grid, headed, cell, place, in_grid, under, button, name));
        assert!(tree.out.borrow().is_empty() && Rc::strong_count(&tree) == 1, "every provider was freed when the last let go of it");
    }

    #[test]
    fn windows_itself_goes_through_a_table_by_its_rows_and_columns() {
        // Windows' own UI Automation client, by its flat functions: a cell through the table's
        // grid pattern, and the table's and cell's properties, which it gets from the patterns.
        // (No window, no other program.)
        type Handle = *mut c_void;
        let core = library("uiautomationcore.dll").unwrap();
        let (node_of, pattern_of): (unsafe extern "system" fn(*mut Face, *mut Handle) -> i32, unsafe extern "system" fn(Handle, i32, *mut Handle) -> i32) =
            unsafe { (found(core, c"UiaNodeFromProvider").unwrap(), found(core, c"UiaGetPatternProvider").unwrap()) };
        let item_of: unsafe extern "system" fn(Handle, i32, i32, *mut Handle) -> i32 = unsafe { found(core, c"GridPattern_GetItem").unwrap() };
        let value_of: unsafe extern "system" fn(Handle, i32, *mut Variant) -> i32 = unsafe { found(core, c"UiaGetPropertyValue").unwrap() };
        let let_go: [unsafe extern "system" fn(Handle) -> i32; 2] = unsafe { [c"UiaPatternRelease", c"UiaNodeRelease"].map(|name| found(core, name).unwrap()) };
        let _alone = WINDOWS_ASKS.lock().unwrap_or_else(|held| held.into_inner());
        assert!(unsafe { CoInitializeEx(std::ptr::null_mut(), 2) } >= 0);
        let tree = made_tree(ledger());
        let (face, mut node, mut pattern, mut cell) = (Provider::of(&tree, 1, THING), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        let property = |node: Handle, property: i32| {
            let mut value = std::mem::MaybeUninit::<Variant>::zeroed();
            assert_eq!(unsafe { value_of(node, property, value.as_mut_ptr()) }, 0);
            read(unsafe { value.assume_init() })
        };
        unsafe {
            assert_eq!((node_of(face, &mut node), pattern_of(node, 10006, &mut pattern), pattern.is_null()), (0, 0, false));
            assert_eq!((item_of(pattern, 11, 1, &mut cell), cell.is_null()), (0, false));
        }
        // The table: row and column counts. The cell: name, control type, position.
        assert_eq!([property(node, 30062), property(node, 30063), property(node, CONTROL_TYPE)], [Value::Number(40), Value::Number(2), Value::Number(50036)]);
        assert_eq!(property(cell, NAME), Value::Text("Open".into()));
        assert_eq!([property(cell, CONTROL_TYPE), property(cell, 30064), property(cell, 30065), property(cell, 30066)], [50029, 11, 1, 1].map(Value::Number));
        unsafe {
            let_go[1](cell);
            let_go[0](pattern);
            let_go[1](node);
            Provider::release(face);
            CoUninitialize();
        }
    }

    #[test]
    fn windows_itself_reads_a_text_box_through_the_text_pattern() {
        // Windows' own UI Automation client, handed a text box's provider: the text pattern and
        // ranges, by the IIDs and vtable order it knows. (No window, no other program.)
        type Handle = *mut c_void;
        let core = library("uiautomationcore.dll").unwrap();
        let (node_of, pattern_of): (unsafe extern "system" fn(*mut Face, *mut Handle) -> i32, unsafe extern "system" fn(Handle, i32, *mut Handle) -> i32) =
            unsafe { (found(core, c"UiaNodeFromProvider").unwrap(), found(core, c"UiaGetPatternProvider").unwrap()) };
        let all_of: unsafe extern "system" fn(Handle, *mut Handle) -> i32 = unsafe { found(core, c"TextPattern_get_DocumentRange").unwrap() };
        let text_of: unsafe extern "system" fn(Handle, i32, *mut Text) -> i32 = unsafe { found(core, c"TextRange_GetText").unwrap() };
        let widen: unsafe extern "system" fn(Handle, i32) -> i32 = unsafe { found(core, c"TextRange_ExpandToEnclosingUnit").unwrap() };
        let go: unsafe extern "system" fn(Handle, i32, i32, *mut i32) -> i32 = unsafe { found(core, c"TextRange_Move").unwrap() };
        let move_end: unsafe extern "system" fn(Handle, i32, i32, i32, *mut i32) -> i32 = unsafe { found(core, c"TextRange_MoveEndpointByUnit").unwrap() };
        let let_go: [unsafe extern "system" fn(Handle) -> i32; 3] = unsafe { [c"UiaTextRangeRelease", c"UiaPatternRelease", c"UiaNodeRelease"].map(|name| found(core, name).unwrap()) };
        let _alone = WINDOWS_ASKS.lock().unwrap_or_else(|held| held.into_inner());
        assert!(unsafe { CoInitializeEx(std::ptr::null_mut(), 2) } >= 0);
        let tree = made_tree(vec![thing(1, Role::Button, "Save", None), notes()]);
        let (face, mut node, mut pattern, mut stretch, mut went) = (Provider::of(&tree, 6, THING), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), [0; 2]);
        let read = |stretch: Handle| {
            let mut text = std::ptr::null_mut();
            assert_eq!(unsafe { text_of(stretch, -1, &mut text) }, 0);
            read_text(text)
        };
        unsafe {
            assert_eq!((node_of(face, &mut node), pattern_of(node, 10014, &mut pattern), pattern.is_null()), (0, 0, false));
            assert_eq!((all_of(pattern, &mut stretch), stretch.is_null()), (0, false));
        }
        assert_eq!(read(stretch), "one two\nthree");
        // By word (2) and line (3), as a screen reader reads.
        unsafe {
            assert_eq!((widen(stretch, 2), read(stretch)), (0, "one ".to_string()));
            assert_eq!((go(stretch, 2, 1, &mut went[0]), went[0], read(stretch)), (0, 1, "two\n".to_string()));
            assert_eq!((go(stretch, 3, 1, &mut went[0]), went[0], read(stretch)), (0, 1, "three".to_string()));
            assert_eq!((move_end(stretch, 0, 0, -2, &mut went[1]), went[1], read(stretch)), (0, -2, "o\nthree".to_string()));
            [stretch, pattern, node].into_iter().zip(let_go).for_each(|(held, let_go)| _ = let_go(held));
            Provider::release(face);
            CoUninitialize();
        }
        // Every range Windows got is freed: only this test and the provider hold what was told.
        // (Windows keeps one reference to a provider it made a node of after the node is
        // released, as with a node alone; from now on that provider says its window is gone.)
        tree.hwnd.set(0);
        assert_eq!((tree.out.borrow().len(), Rc::strong_count(&tree)), (1, 2));
    }

    #[test]
    fn windows_itself_reads_the_part_of_a_selection_in_a_table_cell() {
        // Windows' own UI Automation client, only reading: a cell's text pattern and its
        // selection, a document's selection that runs through it. (No window, no other program.)
        type Handle = *mut c_void;
        let core = library("uiautomationcore.dll").unwrap();
        let (node_of, pattern_of): (unsafe extern "system" fn(*mut Face, *mut Handle) -> i32, unsafe extern "system" fn(Handle, i32, *mut Handle) -> i32) =
            unsafe { (found(core, c"UiaNodeFromProvider").unwrap(), found(core, c"UiaGetPatternProvider").unwrap()) };
        let picked_of: unsafe extern "system" fn(Handle, *mut *mut c_void) -> i32 = unsafe { found(core, c"TextPattern_GetSelection").unwrap() };
        let text_of: unsafe extern "system" fn(Handle, i32, *mut Text) -> i32 = unsafe { found(core, c"TextRange_GetText").unwrap() };
        let let_go: [unsafe extern "system" fn(Handle) -> i32; 3] = unsafe { [c"UiaTextRangeRelease", c"UiaPatternRelease", c"UiaNodeRelease"].map(|name| found(core, name).unwrap()) };
        let _alone = WINDOWS_ASKS.lock().unwrap_or_else(|held| held.into_inner());
        assert!(unsafe { CoInitializeEx(std::ptr::null_mut(), 2) } >= 0);
        let mut nodes = ledger();
        nodes[4].caret = Some([2, 5]);
        let tree = made_tree(nodes);
        let (face, mut node, mut pattern, mut row, mut text) = (Provider::of(&tree, 5, THING), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
        // (The selection comes as variants each holding a range's handle as a number, VT_I8.)
        let mut first = std::mem::MaybeUninit::<Variant>::zeroed();
        let stretch = unsafe {
            assert_eq!((node_of(face, &mut node), pattern_of(node, 10014, &mut pattern), pattern.is_null()), (0, 0, false), "a cell is read as text");
            assert_eq!((picked_of(pattern, &mut row), SafeArrayGetElement(row, &0, first.as_mut_ptr().cast())), (0, 0));
            let first = first.assume_init();
            assert_eq!(first.kind, 20);
            first.held.object
        };
        assert_eq!(unsafe { text_of(stretch, -1, &mut text) }, 0);
        assert_eq!(read_text(text), "txt", "the part of \"a.txt\" selected");
        unsafe {
            [stretch, pattern, node].into_iter().zip(let_go).for_each(|(held, let_go)| _ = let_go(held));
            SafeArrayDestroy(row);
            Provider::release(face);
            CoUninitialize();
        }
        tree.hwnd.set(0);
    }

    #[test]
    fn windows_itself_reads_what_is_off_the_screen_and_the_words_spelt_wrong() {
        // Windows' own UI Automation client, only reading: is offscreen, whether scroll item is
        // supported (without calling it), and annotations on a text box's ranges. (No window,
        // no other program.)
        type Handle = *mut c_void;
        let core = library("uiautomationcore.dll").unwrap();
        let (node_of, pattern_of): (unsafe extern "system" fn(*mut Face, *mut Handle) -> i32, unsafe extern "system" fn(Handle, i32, *mut Handle) -> i32) =
            unsafe { (found(core, c"UiaNodeFromProvider").unwrap(), found(core, c"UiaGetPatternProvider").unwrap()) };
        let value_of: unsafe extern "system" fn(Handle, i32, *mut Variant) -> i32 = unsafe { found(core, c"UiaGetPropertyValue").unwrap() };
        let all_of: unsafe extern "system" fn(Handle, *mut Handle) -> i32 = unsafe { found(core, c"TextPattern_get_DocumentRange").unwrap() };
        let widen: unsafe extern "system" fn(Handle, i32) -> i32 = unsafe { found(core, c"TextRange_ExpandToEnclosingUnit").unwrap() };
        let go: unsafe extern "system" fn(Handle, i32, i32, *mut i32) -> i32 = unsafe { found(core, c"TextRange_Move").unwrap() };
        let look: unsafe extern "system" fn(Handle, i32, *mut Variant) -> i32 = unsafe { found(core, c"TextRange_GetAttributeValue").unwrap() };
        let let_go: [unsafe extern "system" fn(Handle) -> i32; 3] = unsafe { [c"UiaTextRangeRelease", c"UiaPatternRelease", c"UiaNodeRelease"].map(|name| found(core, name).unwrap()) };
        let _alone = WINDOWS_ASKS.lock().unwrap_or_else(|held| held.into_inner());
        assert!(unsafe { CoInitializeEx(std::ptr::null_mut(), 2) } >= 0);
        let letter = AccessNode { value: "the qwrty and zxcvb".into(), misspelled: vec![[4, 9], [14, 19]], ..thing(6, Role::TextBox, "Letter", None) };
        let tree = made_tree(vec![thing(1, Role::Button, "Save", None), AccessNode { visible: false, ..thing(2, Role::Button, "Later", None) }, letter]);
        let node = |id: Id| {
            let (face, mut node) = (Provider::of(&tree, id, THING), std::ptr::null_mut());
            assert_eq!(unsafe { node_of(face, &mut node) }, 0);
            unsafe { Provider::release(face) };
            node
        };
        let (save, later, letter) = (node(1), node(2), node(6));
        let off = |node: Handle| {
            let mut value = std::mem::MaybeUninit::<Variant>::zeroed();
            assert_eq!(unsafe { value_of(node, OFF_THE_SCREEN, value.as_mut_ptr()) }, 0);
            read(unsafe { value.assume_init() })
        };
        let scrolls = |node: Handle| {
            let mut pattern = std::ptr::null_mut();
            assert_eq!(unsafe { pattern_of(node, 10017, &mut pattern) }, 0);
            (!pattern.is_null()).then(|| unsafe { let_go[1](pattern) }).is_some()
        };
        assert_eq!((off(save), off(later), scrolls(save), scrolls(later)), (Value::Truth(false), Value::Truth(true), true, true));
        // The text by format units: "the ", then "qwrty", annotated as misspelled.
        let (mut pattern, mut stretch, mut went) = (std::ptr::null_mut(), std::ptr::null_mut(), 0);
        let marks = |stretch: Handle| {
            let mut value = std::mem::MaybeUninit::<Variant>::zeroed();
            assert_eq!(unsafe { look(stretch, MARKS, value.as_mut_ptr()) }, 0);
            let (value, mut first) = (unsafe { value.assume_init() }, 0i32);
            let row = unsafe { value.held.object };
            let marked = value.kind == 0x2003 && unsafe { SafeArrayGetElement(row, &0, (&raw mut first).cast()) } == 0;
            (value.kind == 0x2003).then(|| unsafe { SafeArrayDestroy(row) });
            (value.kind, marked.then_some(first))
        };
        unsafe {
            assert_eq!((pattern_of(letter, 10014, &mut pattern), all_of(pattern, &mut stretch)), (0, 0));
            assert_eq!(widen(stretch, 1), 0);
            assert_eq!(marks(stretch), (0x2003, None), "\"the \" is not marked");
            assert_eq!((go(stretch, 1, 1, &mut went), went), (0, 1));
            assert_eq!(marks(stretch), (0x2003, Some(SPELT_WRONG)), "\"qwrty\" is spelt wrong");
            [stretch, pattern, letter].into_iter().zip(let_go).for_each(|(held, let_go)| _ = let_go(held));
            [save, later].into_iter().for_each(|node| _ = let_go[2](node));
            CoUninitialize();
        }
        tree.hwnd.set(0);
    }

    #[test]
    fn windows_is_told_what_changed_and_no_more() {
        let was = everything();
        let changed = |alter: fn(&mut Vec<AccessNode>)| {
            let mut now = everything();
            alter(&mut now);
            changes(&was, true, &now, true)
        };
        let property = |id: Id, property: i32, was: Value, is: Value| Change::Property(id, property, was, is);
        let text = |text: &str| Value::Text(text.into());
        assert_eq!(changed(|_| {}), []);
        // A name, a text value, a slider value, enabled.
        assert_eq!(changed(|now| now[2].name = "Save all".into()), [property(3, NAME, text("Save"), text("Save all"))]);
        assert_eq!(changed(|now| now[5].value = "Ada L".into()), [property(6, VALUE, text("Ada"), text("Ada L")), Change::Text(6)]);
        // In a text box, the caret moving is reported alone.
        assert_eq!(changed(|now| now[5].caret = Some([1, 3])), [Change::Caret(6)]);
        assert_eq!(changed(|now| now[4].value_range = Some([4.5, 0.0, 10.0])), [property(5, AMOUNT, Value::Real(4.0), Value::Real(4.5))]);
        assert_eq!(changed(|now| now[6].enabled = true), [property(7, ENABLED, Value::Truth(false), Value::Truth(true))]);
        // State, as its role has it: toggled, expanded, selected.
        assert_eq!(changed(|now| now[3].on = Some(false)), [property(4, TICKED, Value::Number(1), Value::Number(0))]);
        assert_eq!(changed(|now| now[8].on = Some(false)), [property(9, TICKED, Value::Number(1), Value::Number(0))]);
        assert_eq!(changed(|now| now[7].on = Some(true)), [property(8, UNFOLDED, Value::Number(0), Value::Number(1))]);
        assert_eq!(changed(|now| now[6].on = Some(true)), [property(7, IS_SELECTED, Value::Truth(false), Value::Truth(true)), Change::Picked(7)]);
        // Position is asked when wanted, not raised; scrolled out of view or into it is.
        assert_eq!(changed(|now| now[2].rect.y += 40.0), []);
        assert_eq!(changed(|now| now[2].visible = false), [property(3, OFF_THE_SCREEN, Value::Truth(false), Value::Truth(true))]);
        // Elements come and go: reported by their parent. A new element has no property
        // changes.
        assert_eq!(changed(|now| now.push(thing(12, Role::Button, "New", None))), [Change::Children(0)]);
        assert_eq!(changed(|now| now.insert(4, thing(12, Role::Button, "New", Some(1)))), [Change::Children(2)]);
        assert_eq!(changed(|now| _ = now.remove(3)), [Change::Children(2)]);
        assert_eq!(changed(|now| now.swap(2, 3)), [Change::Children(2)]);
        assert_eq!(changed(|now| now.truncate(1)), [Change::Children(0)]);
        let grouped = |now: &mut Vec<AccessNode>| now.extend([thing(12, Role::Group, "More tools", None), thing(13, Role::Button, "New", Some(11))]);
        assert_eq!(changed(grouped), [Change::Children(0)]);
        // The rows come to be sorted by a heading, or reversed; a table gets more rows or fewer
        // columns.
        let sorted = |down: Option<bool>| Value::Text(format!("sorted {}", if down == Some(true) { "descending" } else { "ascending" }));
        assert_eq!(changed(|now| now[9].sorted = Some(false)), [property(10, STATUS, Value::None, sorted(Some(false)))]);
        let mut by_size = everything();
        by_size[9].sorted = Some(false);
        let mut down = everything();
        down[9].sorted = Some(true);
        assert_eq!(changes(&by_size, true, &down, true), [property(10, STATUS, sorted(Some(false)), sorted(Some(true)))]);
        let mut grown = ledger();
        grown[0].grid = Some([41, 1]);
        let counts = [property(1, ROW_COUNT, Value::Number(40), Value::Number(41)), property(1, COLUMN_COUNT, Value::Number(2), Value::Number(1))];
        assert_eq!(changes(&ledger(), true, &grown, true), counts);
        // Another role under an existing id is another element.
        assert_eq!(changed(|now| (now[2].role, now[2].name) = (Role::CheckBox, "Other".into())), [Change::Children(2)]);
        // Focus goes to an element, on to another, and off all.
        assert_eq!(changed(|now| now[2].focused = true), [Change::Focus(3)]);
        let mut on_button = everything();
        on_button[2].focused = true;
        let then = |keyboard: [bool; 2], alter: fn(&mut Vec<AccessNode>)| {
            let mut now = on_button.clone();
            alter(&mut now);
            changes(&on_button, keyboard[0], &now, keyboard[1])
        };
        assert_eq!(then([true, true], |now| (now[2].focused, now[4].focused) = (false, true)), [Change::Focus(5)]);
        assert_eq!(then([true, true], |now| now[2].focused = false), [Change::Focus(0)]);
        // Another window takes focus: Windows reports that. Focus returns: the element focused
        // within has it again.
        assert_eq!((then([true, false], |_| {}), then([false, true], |_| {})), (vec![], vec![Change::Focus(3)]));
        // Property changes are raised before focus.
        assert_eq!(
            then([true, true], |now| (now[2].focused, now[4].focused, now[4].value_range) = (false, true, Some([5.0, 0.0, 10.0]))),
            [property(5, AMOUNT, Value::Real(4.0), Value::Real(5.0)), Change::Focus(5)]
        );
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HANDLE, menu: HANDLE, instance: HANDLE, with: *mut c_void) -> HANDLE;
        fn SetWindowLongPtrW(hwnd: HANDLE, at: i32, value: isize) -> isize;
        fn DefWindowProcW(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize;
        fn DestroyWindow(hwnd: HANDLE) -> i32;
    }

    thread_local! {
        /// What was told of the window that hears requests, and what it heard.
        static HEARD_BY: RefCell<Option<Rc<WindowTree>>> = const { RefCell::new(None) };
        static HEARD: RefCell<Vec<Ask>> = const { RefCell::new(Vec::new()) };
    }
    /// A window that only hears screen-reader requests: takes them, as the interface would, and
    /// notes them.
    unsafe extern "system" fn hears(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize {
        if msg != WM_READER {
            return unsafe { DefWindowProcW(hwnd, msg, w, l) };
        }
        let asked = HEARD_BY.with_borrow(|tree| tree.as_ref().and_then(|tree| tree.asked.take()));
        HEARD.with_borrow_mut(|heard| heard.extend(asked));
        0
    }
    /// Such a window, message-only (never shown or active), for what was told: providers ask it
    /// from then on. Ends with `DestroyWindow`.
    fn hearing(tree: &Rc<WindowTree>) -> HANDLE {
        let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
        let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, -3, 0, 0, std::ptr::null_mut()) };
        assert_ne!(hwnd, 0);
        unsafe { SetWindowLongPtrW(hwnd, -4, hears as *const () as isize) };
        tree.hwnd.set(hwnd);
        HEARD_BY.set(Some(tree.clone()));
        HEARD.with_borrow_mut(Vec::clear);
        hwnd
    }
    fn heard() -> Vec<Ask> {
        HEARD.with_borrow_mut(std::mem::take)
    }

    thread_local! {
        /// The reference a closing window releases, and how many of its providers were still
        /// out then.
        static LETTING_GO: Cell<*mut Face> = const { Cell::new(std::ptr::null_mut()) };
        static LEFT: Cell<usize> = const { Cell::new(usize::MAX) };
    }
    /// A window that closes when asked to act, as a second window does at its button: it
    /// releases a provider (as `gone` releases all) and notes how many providers are still out.
    unsafe extern "system" fn closes(hwnd: HANDLE, msg: u32, w: usize, l: isize) -> isize {
        if msg != WM_READER {
            return unsafe { DefWindowProcW(hwnd, msg, w, l) };
        }
        unsafe { Provider::release(LETTING_GO.take()) };
        LEFT.set(HEARD_BY.with_borrow(|tree| tree.as_ref().map_or(0, |tree| tree.out.borrow().len())));
        0
    }

    #[test]
    fn a_provider_lasts_for_as_long_as_it_answers() {
        // A client invokes a button through its provider, holding nothing else; the window
        // closes meanwhile and releases the provider. It lives until it has answered, and goes
        // after.
        let tree = made_tree(vec![thing(3, Role::Button, "Close", None)]);
        let window = hearing(&tree);
        unsafe { SetWindowLongPtrW(window, -4, closes as *const () as isize) };
        let press = Provider::of(&tree, 3, PRESS);
        LETTING_GO.set(press);
        let code = unsafe { (((*press).table as *const PressTable).read().press)(press) };
        assert_eq!((code, LEFT.get(), tree.out.borrow().len()), (NOT_NOW, 1, 0));
        unsafe { DestroyWindow(window) };
        HEARD_BY.set(None);
        assert_eq!(Rc::strong_count(&tree), 1);
    }

    #[test]
    fn a_stretch_that_is_not_one_of_these_is_not_read() {
        // Of another vendor's object only its first word, the vtable pointer, is known: here it
        // has nothing more. A provider is no range, nor is null. None is read past that word,
        // and each is refused.
        let tree = made_tree(vec![notes()]);
        let stretch = Kept(Stretch::made(&tree, 6, [0, 3]));
        let mut foreign = [(&raw const AS_THING) as usize];
        let provider = Provider::of(&tree, 6, THING);
        for other in [foreign.as_mut_ptr().cast::<Stretch>(), provider.cast(), std::ptr::null_mut()] {
            let (mut same, mut far) = (7, 7);
            let codes = unsafe {
                [(stretch.table().same)(stretch.0, other, &mut same), (stretch.table().compare_ends)(stretch.0, 0, other, 0, &mut far), (stretch.table().move_end_to)(stretch.0, 1, other, 0)]
            };
            assert_eq!((codes, same, far, stretch.ends()), ([NO_SUCH; 3], 0, 0, [0, 3]));
        }
        // One of these is read, and is equal.
        let mut same = 0;
        assert_eq!((unsafe { (stretch.table().same)(stretch.0, stretch.copy().0, &mut same) }, same), (0, 1));
        unsafe { Provider::release(provider) };
    }

    /// A window with a tree, as the library's parts tell it, and the id of the element of a
    /// name.
    fn grove() -> (Rc<WindowTree>, impl Fn(&str) -> Id) {
        let nodes = crate::tree_view::tests::grown().0.visible_access_nodes().to_vec();
        let ids: Vec<(String, Id)> = nodes.iter().map(|node| (node.name.clone(), node.id)).collect();
        (made_tree(nodes), move |name: &str| ids.iter().find(|(named, _)| named == name).map(|(_, id)| *id).unwrap_or_else(|| panic!("nothing is called {name}")))
    }

    #[test]
    fn a_tree_is_told_as_items_that_open_close_and_are_picked() {
        const TO_SELECT: i32 = 10001;
        let (tree, id) = grove();
        let root = tree.nodes.borrow()[0].id;
        let [folder, open, note, picked] = ["folder 0", "folder 1", "note 10", "note 11"].map(&id);
        // Control types and patterns: a tree reports its selected items; an item is selectable,
        // and expands where it has children.
        let of = |id: Id, property: i32| Held::of(&tree, id).property(property);
        assert_eq!([root, folder, note].map(|id| of(id, CONTROL_TYPE)), [50023, 50024, 50024].map(|kind| Ok(Value::Number(kind))));
        let ways = |id: Id| [TO_SELECT, TO_FOLD, TO_PICK, TO_PRESS].map(|pattern| Held::of(&tree, id).way(pattern).is_some());
        assert_eq!([ways(root), ways(folder), ways(note)], [[true, false, false, false], [false, true, true, false], [false, false, true, false]]);
        // Level and position of each; each focusable.
        assert_eq!([open, note].map(|id| [LEVEL, NTH, OF_SO_MANY].map(|property| of(id, property))), [[1, 2, 10], [2, 1, 3]].map(|numbers| numbers.map(|n| Ok(Value::Number(n)))));
        assert_eq!((of(root, LEVEL), of(note, TAKES_KEYBOARD)), (Ok(Value::None), Ok(Value::Truth(true))));
        // Expanded, collapsed; selected, not; each one's tree, and the tree's selection.
        let state = |id: Id| Held::of(&tree, id).way(TO_FOLD).map(|way| way.get(way.table::<FoldTable>().unfolded));
        assert_eq!((state(open), state(folder)), (Some(Ok(1)), Some(Ok(0))));
        let pick = |id: Id| Held::of(&tree, id).way(TO_PICK).unwrap();
        assert_eq!((pick(picked).get(pick(picked).table::<ChoiceTable>().picked), pick(note).get(pick(note).table::<ChoiceTable>().picked)), (Ok(1), Ok(0)));
        let among = Held(pick(note).get(pick(note).table::<ChoiceTable>().among).unwrap());
        assert!(std::ptr::eq(among.provider(), Held::of(&tree, root).provider()));
        let picks = Held::of(&tree, root).way(TO_SELECT).unwrap();
        assert_eq!(names_in(picks.get(picks.table::<SelectionTable>().selection).unwrap()), ["note 11"]);
        assert_eq!((picks.get(picks.table::<SelectionTable>().many), picks.get(picks.table::<SelectionTable>().required)), (Ok(1), Ok(0)));

        // A screen reader's requests reach the window as the keys that do them; what is as
        // asked already is left alone. (A hearing window stands in for the real one.)
        let window = hearing(&tree);
        let act = |id: Id, pattern: i32, act: fn(&Held) -> Do| {
            let way = Held::of(&tree, id).way(pattern).unwrap();
            unsafe { act(&way)(way.0) }
        };
        let codes = [
            act(folder, TO_FOLD, |way| way.table::<FoldTable>().unfold),
            act(open, TO_FOLD, |way| way.table::<FoldTable>().unfold),
            act(open, TO_FOLD, |way| way.table::<FoldTable>().fold),
            act(note, TO_PICK, |way| way.table::<ChoiceTable>().add),
            act(picked, TO_PICK, |way| way.table::<ChoiceTable>().add),
            act(picked, TO_PICK, |way| way.table::<ChoiceTable>().remove),
            act(note, TO_PICK, |way| way.table::<ChoiceTable>().remove),
            act(picked, TO_PICK, |way| way.table::<ChoiceTable>().pick),
        ];
        assert_eq!(codes, [0; 8]);
        assert_eq!(heard(), [Ask::Open(folder, true), Ask::Open(open, false), Ask::Pick(note), Ask::Pick(picked), Ask::Press(picked)]);
        unsafe { DestroyWindow(window) };
        tree.hwnd.set(0);
        assert_eq!(of(folder, CONTROL_TYPE), Err(GONE));
    }

    #[test]
    fn windows_itself_reads_a_tree_and_opens_and_picks_its_items() {
        // Windows' own UI Automation client, by its flat functions: a tree's item properties,
        // got from the patterns; and expanding and selecting through them. (No window but a
        // hearing one, no other program.)
        type Handle = *mut c_void;
        let core = library("uiautomationcore.dll").unwrap();
        let (node_of, pattern_of): (unsafe extern "system" fn(*mut Face, *mut Handle) -> i32, unsafe extern "system" fn(Handle, i32, *mut Handle) -> i32) =
            unsafe { (found(core, c"UiaNodeFromProvider").unwrap(), found(core, c"UiaGetPatternProvider").unwrap()) };
        let value_of: unsafe extern "system" fn(Handle, i32, *mut Variant) -> i32 = unsafe { found(core, c"UiaGetPropertyValue").unwrap() };
        let [expand, add]: [unsafe extern "system" fn(Handle) -> i32; 2] = unsafe { [c"ExpandCollapsePattern_Expand", c"SelectionItemPattern_AddToSelection"].map(|name| found(core, name).unwrap()) };
        let let_go: [unsafe extern "system" fn(Handle) -> i32; 2] = unsafe { [c"UiaPatternRelease", c"UiaNodeRelease"].map(|name| found(core, name).unwrap()) };
        let _alone = WINDOWS_ASKS.lock().unwrap_or_else(|held| held.into_inner());
        assert!(unsafe { CoInitializeEx(std::ptr::null_mut(), 2) } >= 0);
        let (tree, id) = grove();
        let window = hearing(&tree);
        let property = |node: Handle, property: i32| {
            let mut value = std::mem::MaybeUninit::<Variant>::zeroed();
            assert_eq!(unsafe { value_of(node, property, value.as_mut_ptr()) }, 0);
            read(unsafe { value.assume_init() })
        };
        let [root, folder, note] = [tree.nodes.borrow()[0].id, id("folder 2"), id("note 12")];
        let faces = [root, folder, note].map(|id| Provider::of(&tree, id, THING));
        let mut nodes = [std::ptr::null_mut(); 3];
        for (face, node) in faces.iter().zip(&mut nodes) {
            assert_eq!(unsafe { node_of(*face, node) }, 0);
        }
        // The tree: multiple selection, none required. A folder: tree item, level, position,
        // collapsed (0), unselected. A note: unselected.
        assert_eq!([property(nodes[0], CONTROL_TYPE), property(nodes[0], 30060), property(nodes[0], 30061)], [Value::Number(50023), Value::Truth(true), Value::Truth(false)]);
        let read_of = |node: Handle| [CONTROL_TYPE, LEVEL, NTH, OF_SO_MANY, UNFOLDED].map(|id| property(node, id));
        assert_eq!(read_of(nodes[1]), [50024, 1, 3, 10, 0].map(Value::Number));
        assert_eq!([property(nodes[2], 30079), property(nodes[2], LEVEL)], [Value::Truth(false), Value::Number(2)]);
        // Expanded and selected through the patterns Windows hands out.
        let mut patterns = [std::ptr::null_mut(); 2];
        unsafe {
            assert_eq!((pattern_of(nodes[1], 10005, &mut patterns[0]), pattern_of(nodes[2], 10010, &mut patterns[1])), (0, 0));
            assert_eq!((expand(patterns[0]), add(patterns[1])), (0, 0));
        }
        // (Windows focuses each before acting: as for every client that acts.)
        assert_eq!(heard(), [Ask::Focus(folder), Ask::Open(folder, true), Ask::Focus(note), Ask::Pick(note)]);
        unsafe {
            patterns.into_iter().for_each(|pattern| _ = let_go[0](pattern));
            nodes.into_iter().for_each(|node| _ = let_go[1](node));
            faces.into_iter().for_each(|face| _ = Provider::release(face));
            DestroyWindow(window);
            CoUninitialize();
        }
        tree.hwnd.set(0);
    }
}
