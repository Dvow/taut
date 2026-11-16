//! Reads the taut demo window as a screen reader does, and operates it as one does: through
//! libatspi, the library Orca uses, over a private accessibility bus. Nothing speaks and
//! nothing shows: it runs on a display and session bus made for it and gone with it, so no
//! one's desktop, settings or clipboard is touched.
//!
//!   cargo build -p taut --example gallery --example walk_reader --release
//!   xvfb-run -a -s "-screen 0 900x600x24" dbus-run-session -- \
//!     target/release/examples/walk_reader [<the gallery>]
//!
//! Without a gallery path, the demo built beside this program is walked. It prints "ok" or
//! "FAIL" per check, and exits with 1 if anything failed. Needs libatspi, at-spi-bus-launcher
//! and at-spi2-registryd (package at-spi2-core), gdbus (glib), libX11 and libXtst.
//!
//! The accessibility bus is the one at-spi-bus-launcher starts. To walk over another (one run
//! by dbus-broker, say, where the launcher would start dbus-daemon), set AT_SPI_BUS_ADDRESS, as
//! a desktop may, and list the processes running it in TAUT_WALK_BUS_PIDS.

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn main() {
    println!("the screen reader's walk goes over AT-SPI, which is how windows are read on Linux: there is nothing for it to walk on this system");
}

#[cfg(all(unix, not(target_os = "macos")))]
fn main() -> std::process::ExitCode {
    reader::walk()
}

#[cfg(all(unix, not(target_os = "macos")))]
mod reader {
    use std::cell::RefCell;
    use std::ffi::{CStr, c_char, c_int, c_uchar, c_uint, c_ulong, c_void};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, ExitCode, Stdio};
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use std::{env, fs};

    unsafe extern "C" {
        fn dlopen(name: *const c_char, flags: c_int) -> *mut c_void;
        fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
        fn kill(process: c_int, signal: c_int) -> c_int;
    }

    /// A thing a screen reader is told of: a program, a window, or something in a window. It
    /// is libatspi's, which keeps one for each thing there is; and none is handed back before
    /// the walk is over, so two that are the same thing are the same pointer. (Nothing: a
    /// pointer to nothing, which libatspi takes for a thing as well.)
    #[repr(transparent)]
    #[derive(Clone, Copy, PartialEq)]
    struct Thing(*mut c_void);

    impl Thing {
        const NONE: Thing = Thing(null_mut());

        fn is_none(self) -> bool {
            self.0.is_null()
        }
    }

    /// libatspi could not get an answer: which of its functions, and what it said was wrong.
    struct Gone(String);

    /// What went wrong, as glib hands it over: its words are what is read of it.
    #[repr(C)]
    struct Problem {
        domain: u32,
        code: c_int,
        message: *const c_char,
    }
    /// Where a thing is, as libatspi hands it over.
    #[repr(C)]
    struct Place {
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
    }
    /// A stretch of a text, as libatspi hands it over: from where to where, and its letters.
    #[repr(C)]
    struct Stretch {
        start: c_int,
        end: c_int,
        content: *mut c_char,
    }
    /// The start of what a listener is handed: the kind of news, of which thing, and two numbers.
    #[repr(C)]
    struct Event {
        kind: *const c_char,
        source: Thing,
        detail1: c_int,
        detail2: c_int,
    }

    /// A piece of news as it was noted: its kind, the thing it is about, and its first number.
    struct News {
        kind: String,
        of: Thing,
        number: c_int,
    }

    /// One of a library's functions, with the name it has there.
    struct Function<F> {
        name: &'static CStr,
        call: F,
    }

    /// The two commonest kinds of libatspi's functions: one that tells something about a
    /// thing, and one that tells it of one of the thing's parts or ways, by a number. Each
    /// has room for what went wrong as its last value.
    type Tells<T> = unsafe extern "C" fn(Thing, *mut *mut Problem) -> T;
    type TellsOf<T> = unsafe extern "C" fn(Thing, c_int, *mut *mut Problem) -> T;
    /// What libatspi calls with each piece of news, and with the pointer the listener was made with.
    type Heard = unsafe extern "C" fn(*mut Event, *mut c_void);

    /// What the walk asks of libatspi. Nothing is linked when this is built: the library is
    /// opened by name when the walk starts, so it builds on a machine without its headers.
    struct Atspi {
        atspi_init: Function<unsafe extern "C" fn() -> c_int>,
        atspi_get_desktop: Function<unsafe extern "C" fn(c_int) -> Thing>,
        atspi_accessible_set_cache_mask: Function<unsafe extern "C" fn(Thing, c_int)>,
        atspi_accessible_get_child_count: Function<Tells<c_int>>,
        atspi_accessible_get_child_at_index: Function<TellsOf<Thing>>,
        atspi_accessible_get_parent: Function<Tells<Thing>>,
        atspi_accessible_get_index_in_parent: Function<Tells<c_int>>,
        atspi_accessible_get_name: Function<Tells<*mut c_char>>,
        atspi_accessible_get_role: Function<Tells<c_int>>,
        atspi_accessible_get_role_name: Function<Tells<*mut c_char>>,
        atspi_accessible_get_toolkit_name: Function<Tells<*mut c_char>>,
        atspi_accessible_get_attributes: Function<Tells<*mut c_void>>,
        atspi_accessible_get_state_set: Function<unsafe extern "C" fn(Thing) -> *mut c_void>,
        atspi_state_set_contains: Function<unsafe extern "C" fn(*mut c_void, c_int) -> c_int>,
        atspi_accessible_get_action_iface: Function<unsafe extern "C" fn(Thing) -> *mut c_void>,
        atspi_accessible_get_value_iface: Function<unsafe extern "C" fn(Thing) -> *mut c_void>,
        atspi_component_get_extents: Function<TellsOf<*mut Place>>,
        atspi_component_get_accessible_at_point: Function<unsafe extern "C" fn(Thing, c_int, c_int, c_int, *mut *mut Problem) -> Thing>,
        atspi_component_grab_focus: Function<Tells<c_int>>,
        atspi_component_scroll_to: Function<TellsOf<c_int>>,
        atspi_action_get_n_actions: Function<Tells<c_int>>,
        atspi_action_get_action_name: Function<TellsOf<*mut c_char>>,
        atspi_action_get_key_binding: Function<TellsOf<*mut c_char>>,
        atspi_action_do_action: Function<TellsOf<c_int>>,
        atspi_table_get_n_rows: Function<Tells<c_int>>,
        atspi_table_get_n_columns: Function<Tells<c_int>>,
        atspi_table_get_n_selected_rows: Function<Tells<c_int>>,
        atspi_table_get_column_header: Function<TellsOf<Thing>>,
        atspi_table_get_accessible_at: Function<unsafe extern "C" fn(Thing, c_int, c_int, *mut *mut Problem) -> Thing>,
        atspi_table_cell_get_position: Function<unsafe extern "C" fn(Thing, *mut c_int, *mut c_int, *mut *mut Problem) -> c_int>,
        atspi_table_cell_get_table: Function<Tells<Thing>>,
        atspi_selection_get_n_selected_children: Function<Tells<c_int>>,
        atspi_selection_select_child: Function<TellsOf<c_int>>,
        atspi_selection_is_child_selected: Function<TellsOf<c_int>>,
        atspi_value_get_current_value: Function<Tells<f64>>,
        atspi_value_get_minimum_value: Function<Tells<f64>>,
        atspi_value_get_maximum_value: Function<Tells<f64>>,
        atspi_value_set_current_value: Function<unsafe extern "C" fn(Thing, f64, *mut *mut Problem) -> c_int>,
        atspi_text_get_character_count: Function<Tells<c_int>>,
        atspi_text_get_caret_offset: Function<Tells<c_int>>,
        atspi_text_get_text: Function<unsafe extern "C" fn(Thing, c_int, c_int, *mut *mut Problem) -> *mut c_char>,
        atspi_text_get_string_at_offset: Function<unsafe extern "C" fn(Thing, c_int, c_int, *mut *mut Problem) -> *mut Stretch>,
        atspi_editable_text_set_text_contents: Function<unsafe extern "C" fn(Thing, *const c_char, *mut *mut Problem) -> c_int>,
        atspi_editable_text_insert_text: Function<unsafe extern "C" fn(Thing, c_int, *const c_char, c_int, *mut *mut Problem) -> c_int>,
        atspi_event_listener_new: Function<unsafe extern "C" fn(Heard, *mut c_void, *mut c_void) -> *mut c_void>,
        atspi_event_listener_register: Function<unsafe extern "C" fn(*mut c_void, *const c_char, *mut *mut Problem) -> c_int>,
    }

    /// What the walk asks of glib, the library under libatspi: to run its loop, in which
    /// libatspi's news arrives, and to take back the memory libatspi hands out.
    struct Glib {
        g_main_context_iteration: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
        g_free: unsafe extern "C" fn(*mut c_void),
        g_error_free: unsafe extern "C" fn(*mut Problem),
        g_hash_table_lookup: unsafe extern "C" fn(*mut c_void, *const c_char) -> *const c_char,
        g_hash_table_unref: unsafe extern "C" fn(*mut c_void),
    }

    /// Opens one of the system's libraries by its name.
    fn library(name: &CStr) -> *mut c_void {
        // SAFETY: the name ends in a zero. (2: every name the library needs is found now.)
        let library = unsafe { dlopen(name.as_ptr(), 2) };
        assert!(!library.is_null(), "{} is not on this machine", name.to_string_lossy());
        library
    }

    /// One of a library's functions, by its name, as the kind of function `F` is.
    ///
    /// # Safety
    /// `F` has to be the kind of function the library's header declares under that name.
    unsafe fn function<F: Copy>(library: *mut c_void, name: &'static CStr) -> Function<F> {
        const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
        // SAFETY: the library is open, and the name ends in a zero.
        let found = unsafe { dlsym(library, name.as_ptr()) };
        assert!(!found.is_null(), "{} is not in its library", name.to_string_lossy());
        // SAFETY: a function is handed about as its address, and the caller says of which kind it is.
        Function { name, call: unsafe { std::mem::transmute_copy(&found) } }
    }

    /// Opens libatspi, and finds what the walk asks of it.
    fn open_atspi() -> Atspi {
        let atspi = library(c"libatspi.so.0");
        // SAFETY: each function is declared in `Atspi` as libatspi's headers declare it.
        unsafe {
            Atspi {
                atspi_init: function(atspi, c"atspi_init"),
                atspi_get_desktop: function(atspi, c"atspi_get_desktop"),
                atspi_accessible_set_cache_mask: function(atspi, c"atspi_accessible_set_cache_mask"),
                atspi_accessible_get_child_count: function(atspi, c"atspi_accessible_get_child_count"),
                atspi_accessible_get_child_at_index: function(atspi, c"atspi_accessible_get_child_at_index"),
                atspi_accessible_get_parent: function(atspi, c"atspi_accessible_get_parent"),
                atspi_accessible_get_index_in_parent: function(atspi, c"atspi_accessible_get_index_in_parent"),
                atspi_accessible_get_name: function(atspi, c"atspi_accessible_get_name"),
                atspi_accessible_get_role: function(atspi, c"atspi_accessible_get_role"),
                atspi_accessible_get_role_name: function(atspi, c"atspi_accessible_get_role_name"),
                atspi_accessible_get_toolkit_name: function(atspi, c"atspi_accessible_get_toolkit_name"),
                atspi_accessible_get_attributes: function(atspi, c"atspi_accessible_get_attributes"),
                atspi_accessible_get_state_set: function(atspi, c"atspi_accessible_get_state_set"),
                atspi_state_set_contains: function(atspi, c"atspi_state_set_contains"),
                atspi_accessible_get_action_iface: function(atspi, c"atspi_accessible_get_action_iface"),
                atspi_accessible_get_value_iface: function(atspi, c"atspi_accessible_get_value_iface"),
                atspi_component_get_extents: function(atspi, c"atspi_component_get_extents"),
                atspi_component_get_accessible_at_point: function(atspi, c"atspi_component_get_accessible_at_point"),
                atspi_component_grab_focus: function(atspi, c"atspi_component_grab_focus"),
                atspi_component_scroll_to: function(atspi, c"atspi_component_scroll_to"),
                atspi_action_get_n_actions: function(atspi, c"atspi_action_get_n_actions"),
                atspi_action_get_action_name: function(atspi, c"atspi_action_get_action_name"),
                atspi_action_get_key_binding: function(atspi, c"atspi_action_get_key_binding"),
                atspi_action_do_action: function(atspi, c"atspi_action_do_action"),
                atspi_table_get_n_rows: function(atspi, c"atspi_table_get_n_rows"),
                atspi_table_get_n_columns: function(atspi, c"atspi_table_get_n_columns"),
                atspi_table_get_n_selected_rows: function(atspi, c"atspi_table_get_n_selected_rows"),
                atspi_table_get_column_header: function(atspi, c"atspi_table_get_column_header"),
                atspi_table_get_accessible_at: function(atspi, c"atspi_table_get_accessible_at"),
                atspi_table_cell_get_position: function(atspi, c"atspi_table_cell_get_position"),
                atspi_table_cell_get_table: function(atspi, c"atspi_table_cell_get_table"),
                atspi_selection_get_n_selected_children: function(atspi, c"atspi_selection_get_n_selected_children"),
                atspi_selection_select_child: function(atspi, c"atspi_selection_select_child"),
                atspi_selection_is_child_selected: function(atspi, c"atspi_selection_is_child_selected"),
                atspi_value_get_current_value: function(atspi, c"atspi_value_get_current_value"),
                atspi_value_get_minimum_value: function(atspi, c"atspi_value_get_minimum_value"),
                atspi_value_get_maximum_value: function(atspi, c"atspi_value_get_maximum_value"),
                atspi_value_set_current_value: function(atspi, c"atspi_value_set_current_value"),
                atspi_text_get_character_count: function(atspi, c"atspi_text_get_character_count"),
                atspi_text_get_caret_offset: function(atspi, c"atspi_text_get_caret_offset"),
                atspi_text_get_text: function(atspi, c"atspi_text_get_text"),
                atspi_text_get_string_at_offset: function(atspi, c"atspi_text_get_string_at_offset"),
                atspi_editable_text_set_text_contents: function(atspi, c"atspi_editable_text_set_text_contents"),
                atspi_editable_text_insert_text: function(atspi, c"atspi_editable_text_insert_text"),
                atspi_event_listener_new: function(atspi, c"atspi_event_listener_new"),
                atspi_event_listener_register: function(atspi, c"atspi_event_listener_register"),
            }
        }
    }

    /// Opens glib, and finds what the walk asks of it.
    fn open_glib() -> Glib {
        let glib = library(c"libglib-2.0.so.0");
        // SAFETY: each function is declared in `Glib` as glib's headers declare it.
        unsafe {
            Glib {
                g_main_context_iteration: function(glib, c"g_main_context_iteration").call,
                g_free: function(glib, c"g_free").call,
                g_error_free: function(glib, c"g_error_free").call,
                g_hash_table_lookup: function(glib, c"g_hash_table_lookup").call,
                g_hash_table_unref: function(glib, c"g_hash_table_unref").call,
            }
        }
    }

    // The numbers AT-SPI gives roles and states (atspi-constants.h), as far as they are met here.
    const CHECK_BOX: c_int = 7;
    const CHECK_MENU_ITEM: c_int = 8;
    const FRAME: c_int = 23;
    const LABEL: c_int = 29;
    const MENU_ITEM: c_int = 35;
    const PAGE_TAB: c_int = 37;
    const PROGRESS_BAR: c_int = 42;
    const BUTTON: c_int = 43;
    const SLIDER: c_int = 51;
    const TABLE: c_int = 55;
    const TABLE_CELL: c_int = 56;
    const TABLE_ROW: c_int = 90;
    const TREE: c_int = 65;
    const TREE_ITEM: c_int = 91;
    const APPLICATION: c_int = 75;
    const ENTRY: c_int = 79;
    const SWITCH: c_int = 130;
    const CHECKED: c_int = 4;
    const EDITABLE: c_int = 7;
    const ENABLED: c_int = 8;
    const FOCUSABLE: c_int = 11;
    const FOCUSED: c_int = 12;
    const SELECTED: c_int = 23;
    const COLLAPSED: c_int = 5;
    const EXPANDABLE: c_int = 9;
    const EXPANDED: c_int = 10;
    const MULTISELECTABLE: c_int = 18;
    const SENSITIVE: c_int = 24;
    const SHOWING: c_int = 25;
    const VISIBLE: c_int = 30;
    /// The keys that are pressed, by the numbers X gives them.
    const TAB: c_ulong = 0xFF09;
    const ESCAPE: c_ulong = 0xFF1B;
    /// The tick box of the first page, and the text that counts the presses, by what they are called.
    const TICK_BOX: &str = "A tick box, which the keyboard can reach with Tab and flip with Space";
    const COUNTED: &str = "Gallery: 60 items, 0 presses counted";
    /// What the first page of the window shows, by kind and name: buttons, tabs, a tick box,
    /// text boxes, the rows of the list.
    const SHOWN: [(c_int, &str); 13] = [
        (BUTTON, "Start over"),
        (BUTTON, "Count one"),
        (BUTTON, "Add an item"),
        (BUTTON, "File"),
        (BUTTON, "View"),
        (ENTRY, "Filter items"),
        (ENTRY, "Notes"),
        (LABEL, "item 0"),
        (LABEL, "item 3"),
        (CHECK_BOX, TICK_BOX),
        (PAGE_TAB, "List"),
        (PAGE_TAB, "Parts"),
        (LABEL, COUNTED),
    ];
    /// What the second window shows, the same way.
    const SHOWN_IN_THE_SECOND: [(c_int, &str); 4] = [(BUTTON, "Close this window"), (ENTRY, "The name of the first window"), (SWITCH, "Light colours"), (SLIDER, "")];
    /// The kinds of news the walk asks to hear of, as a screen reader does.
    const NEWS: [&CStr; 9] = [
        c"object:state-changed",
        c"object:children-changed",
        c"object:property-change",
        c"object:text-changed",
        c"object:announcement",
        c"window:activate",
        c"window:deactivate",
        c"window:create",
        c"window:destroy",
    ];

    /// How many of the things that were checked did not hold.
    static FAILED: AtomicUsize = AtomicUsize::new(0);

    /// Says whether one thing held, and keeps count of what did not.
    fn check(what: &str, fine: bool, more: &str) -> bool {
        let more = if fine || more.is_empty() { String::new() } else { format!("   [{more}]") };
        println!("{}{what}{more}", if fine { "ok   " } else { "FAIL " });
        FAILED.fetch_add(!fine as usize, Ordering::Relaxed);
        fine
    }

    /// Nothing is done for so many thousandths of a second.
    fn rest(milliseconds: u64) {
        std::thread::sleep(Duration::from_millis(milliseconds));
    }

    /// A text as a program would write it down: in quotes, with what cannot be seen spelled out.
    fn quoted(text: &str) -> String {
        let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
        let mut written = String::from(quote);
        for letter in text.chars() {
            match letter {
                '\n' => written.push_str("\\n"),
                '\r' => written.push_str("\\r"),
                '\t' => written.push_str("\\t"),
                '\\' => written.push_str("\\\\"),
                _ if letter == quote => written.extend(['\\', letter]),
                _ if letter.is_control() => written.push_str(&format!("\\x{:02x}", letter as u32)),
                _ => written.push(letter),
            }
        }
        written.push(quote);
        written
    }

    /// Some texts as a program would write them down: a list of them, each in quotes.
    fn listed(texts: &[String]) -> String {
        format!("[{}]", texts.iter().map(|text| quoted(text)).collect::<Vec<_>>().join(", "))
    }

    /// Things by kind and name, as a program would write them down: a list of pairs.
    fn listed_pairs(things: &[(c_int, String)]) -> String {
        format!("[{}]", things.iter().map(|(role, name)| format!("({role}, {})", quoted(name))).collect::<Vec<_>>().join(", "))
    }

    /// Which of the things that are wanted, by kind and name, are not among those told.
    fn missing(wanted: &[(c_int, &str)], told: &[(c_int, String)]) -> Vec<(c_int, String)> {
        let is_told = |role: c_int, name: &str| told.iter().any(|(told_role, told_name)| *told_role == role && told_name == name);
        let mut missing: Vec<(c_int, String)> = wanted.iter().filter(|(role, name)| !is_told(*role, name)).map(|(role, name)| (*role, name.to_string())).collect();
        missing.sort();
        missing
    }

    /// A number in the fewest digits that tell it: six at the most, and no noughts at its end.
    /// (For numbers from one to a million, which is all this is asked for.)
    fn short(number: f64) -> String {
        let after_the_point = 6usize.saturating_sub(number.abs().max(1.0).log10().floor() as usize + 1);
        let written = format!("{number:.after_the_point$}");
        if written.contains('.') { written.trim_end_matches('0').trim_end_matches('.').to_string() } else { written }
    }

    // ---- the processes: the demo, and the buses it is on ----

    /// Whether a process the walk started is still running.
    fn lives(process: &mut Child) -> bool {
        matches!(process.try_wait(), Ok(None))
    }

    /// How many sockets a process has open: one to X, and one to each bus it is on.
    fn sockets(process: u32) -> usize {
        let open = fs::read_dir(format!("/proc/{process}/fd")).into_iter().flatten().flatten();
        open.filter(|file| fs::read_link(file.path()).is_ok_and(|to| to.to_string_lossy().starts_with("socket:"))).count()
    }

    /// How much memory a process has in use, and how much of that is its own alone.
    fn memory(process: u32) -> String {
        let told = fs::read_to_string(format!("/proc/{process}/smaps_rollup")).unwrap_or_default();
        let megabytes = |kind: &str| {
            let line = told.lines().find(|line| line.split(':').next() == Some(kind));
            line.and_then(|line| line.split_whitespace().nth(1)).and_then(|kilobytes| kilobytes.parse::<f64>().ok()).unwrap_or(0.0) / 1024.0
        };
        format!("{:.2} MB in use, {:.2} MB of its own", megabytes("Rss"), megabytes("Private_Dirty"))
    }

    /// What the system says of a process after its name: how it stands, who started it, and
    /// further on what it has used. (The name stands in brackets and may hold anything.)
    fn after_its_name(process: u32) -> Vec<String> {
        let told = fs::read_to_string(format!("/proc/{process}/stat")).unwrap_or_default();
        told.rsplit_once(')').map_or("", |(_, after)| after).split_whitespace().map(str::to_string).collect()
    }

    /// Milliseconds of processor a process has used so far, in itself and in the system.
    /// (The system counts them in hundredths of a second.)
    fn processor(process: u32) -> i64 {
        after_its_name(process).iter().skip(11).take(2).filter_map(|ticks| ticks.parse::<i64>().ok()).sum::<i64>() * 10
    }

    /// The processes that a process started and that are still there, lowest number first.
    fn started_by(parent: u32) -> Vec<String> {
        let all = fs::read_dir("/proc").into_iter().flatten().flatten().filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok());
        let mut found: Vec<u32> = all.filter(|process| after_its_name(*process).get(1).is_some_and(|by| *by == parent.to_string())).collect();
        found.sort_unstable();
        found.iter().map(u32::to_string).collect()
    }

    /// The processes that run the accessibility bus: the ones the launcher started; or, where
    /// the bus is one named from outside, the ones named with it.
    fn running_the_bus(started: &Started) -> Vec<String> {
        if started.given {
            return env::var("TAUT_WALK_BUS_PIDS").unwrap_or_default().split_whitespace().map(str::to_string).collect();
        }
        started_by(started.launcher.id())
    }

    /// Ends a process at once, by its number as the system writes it.
    fn end_at_once(process: &str) {
        let Ok(process) = process.parse::<c_int>() else { return };
        // SAFETY: a signal (9: end at once) is sent to a process of the walk's own buses.
        unsafe { kill(process, 9) };
    }

    /// The service of the session bus that says whether anyone listens, and the bus itself:
    /// each by its name and the path it is asked at.
    const STATUS: [&str; 2] = ["org.a11y.Bus", "/org/a11y/bus"];
    const SESSION_BUS: [&str; 2] = ["org.freedesktop.DBus", "/org/freedesktop/DBus"];

    /// Asks something of a service of the session bus, with the bus's own tool; gives back
    /// its answer.
    fn session(of: [&str; 2], method: &str, with: &[&str]) -> String {
        let answer = Command::new("gdbus").args(["call", "--session", "--dest", of[0], "--object-path", of[1], "--method", method]).args(with).output().expect("gdbus could not be started");
        String::from_utf8_lossy(&answer.stdout).trim().to_string()
    }

    /// Says on the session bus that someone listens, or that nobody does: the way Orca says
    /// it when it starts.
    fn listening(on: bool) {
        session(STATUS, "org.freedesktop.DBus.Properties.Set", &["org.a11y.Status", "IsEnabled", if on { "<true>" } else { "<false>" }]);
    }

    /// One of the programs of at-spi2-core, in whichever folder this system keeps them.
    fn of_at_spi(name: &str) -> Option<PathBuf> {
        ["/usr/libexec", "/usr/lib", "/usr/lib/at-spi2-core"].iter().map(|folder| Path::new(folder).join(name)).find(|file| file.exists())
    }

    /// The demo that was built beside this program.
    fn the_demo_beside() -> PathBuf {
        env::current_exe().expect("this program does not know where it is").with_file_name("gallery")
    }

    /// Without any session bus, and with one that is not there: the window opens all the
    /// same, with a socket to X and nothing else.
    fn alone_it_opens(gallery: &Path) {
        for (what, bus) in [("no session bus", None), ("a session bus that is not there", Some("unix:path=/nonexistent/taut/bus"))] {
            let mut lonely = Command::new(gallery);
            lonely.env_remove("DBUS_SESSION_BUS_ADDRESS").env_remove("XDG_RUNTIME_DIR").env_remove("AT_SPI_BUS_ADDRESS");
            lonely.envs(bus.map(|bus| ("DBUS_SESSION_BUS_ADDRESS", bus)));
            let mut alone = lonely.spawn().expect("the demo could not be started");
            rest(700);
            let alive = lives(&mut alone);
            check(&format!("with {what}, the window opens and has one socket, to X"), alive && sockets(alone.id()) == 1, &format!("alive {}", if alive { "True" } else { "False" }));
            let _ = alone.kill();
            let _ = alone.wait();
        }
    }

    /// The processes the walk started, and whether the accessibility bus is one that was
    /// named from outside.
    struct Started {
        demo: Child,
        launcher: Child,
        registry: Option<Child>,
        given: bool,
    }

    /// Starts the service that says whether anyone listens, with its settings kept in memory,
    /// so the person's own are neither read nor written; then the demo. (Nothing: the service
    /// is not on this machine.)
    fn start(gallery: &Path) -> Option<Started> {
        let launcher = Command::new(of_at_spi("at-spi-bus-launcher")?).env("GSETTINGS_BACKEND", "memory").stderr(Stdio::null()).spawn().expect("at-spi-bus-launcher could not be started");
        for _ in 0..100 {
            if session(STATUS, "org.freedesktop.DBus.Properties.GetAll", &["org.a11y.Status"]).contains("IsEnabled") {
                break;
            }
            rest(50);
        }
        listening(false);
        // (A bus that is named from outside has no registry on it until one is started there.)
        let given = env::var_os("AT_SPI_BUS_ADDRESS").is_some_and(|bus| !bus.is_empty());
        let registry = of_at_spi("at-spi2-registryd").filter(|_| given).map(|registry| Command::new(registry).stderr(Stdio::null()).spawn().expect("at-spi2-registryd could not be started"));
        let demo = Command::new(gallery).spawn().expect("the demo could not be started");
        rest(1000);
        Some(Started { demo, launcher, registry, given })
    }

    /// Ends what is left of what the walk started, and waits until each is gone.
    fn end(started: &mut Started) {
        for process in [Some(&mut started.demo), Some(&mut started.launcher), started.registry.as_mut()].into_iter().flatten() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }

    // ---- libatspi ----

    /// The screen reader's side of the walk: libatspi and the library under it, what the demo
    /// is called among the programs of the desktop, and the news that came since the list was
    /// last emptied. (The list stays in one place for the whole walk: libatspi is handed a
    /// pointer to it.)
    struct Reader {
        atspi: Atspi,
        glib: Glib,
        program: String,
        news: Box<RefCell<Vec<News>>>,
    }

    /// Notes a piece of news as it comes: libatspi calls this for each one, from inside `pump`.
    unsafe extern "C" fn note(event: *mut Event, noted: *mut c_void) {
        // SAFETY: libatspi hands over a whole event of its own, whose kind is a text that
        // ends in a zero, and the pointer the listener was made with: the list of news.
        let (kind, of, number, news) = unsafe { (CStr::from_ptr((*event).kind), (*event).source, (*event).detail1, &*(noted as *const RefCell<Vec<News>>)) };
        news.borrow_mut().push(News { kind: kind.to_string_lossy().into_owned(), of, number });
    }

    impl Reader {
        /// What libatspi answered; or, where it could not get an answer, what it said was wrong.
        fn answered<T>(&self, function: &CStr, answer: T, problem: *mut Problem) -> Result<T, Gone> {
            if problem.is_null() {
                return Ok(answer);
            }
            // SAFETY: what went wrong is glib's record of it, whose words end in a zero.
            let said = unsafe { CStr::from_ptr((*problem).message) }.to_string_lossy().into_owned();
            // SAFETY: the record is the caller's, and is given back now that its words are read.
            unsafe { (self.glib.g_error_free)(problem) };
            Err(Gone(format!("{}: {said}", function.to_string_lossy())))
        }

        /// Asks libatspi something about a thing; what it could not get an answer for is an
        /// error here.
        fn ask<T>(&self, asked: &Function<Tells<T>>, thing: Thing) -> Result<T, Gone> {
            let mut problem = null_mut();
            // SAFETY: the function is libatspi's, of this kind; the thing is one libatspi
            // handed out, or none; and `problem` is room for what went wrong.
            let answer = unsafe { (asked.call)(thing, &mut problem) };
            self.answered(asked.name, answer, problem)
        }

        /// Asks libatspi something about one of a thing's parts or ways, by its number.
        fn ask_of<T>(&self, asked: &Function<TellsOf<T>>, thing: Thing, of: c_int) -> Result<T, Gone> {
            let mut problem = null_mut();
            // SAFETY: as in `ask`; libatspi takes any number.
            let answer = unsafe { (asked.call)(thing, of, &mut problem) };
            self.answered(asked.name, answer, problem)
        }

        /// The words libatspi handed over, which are given back to it. (Nothing: no words.)
        fn words(&self, words: *mut c_char) -> String {
            if words.is_null() {
                return String::new();
            }
            // SAFETY: a text from libatspi ends in a zero.
            let read = unsafe { CStr::from_ptr(words) }.to_string_lossy().into_owned();
            // SAFETY: the text is the caller's to free with glib, and is not read after this.
            unsafe { (self.glib.g_free)(words.cast()) };
            read
        }

        fn name(&self, thing: Thing) -> Result<String, Gone> {
            Ok(self.words(self.ask(&self.atspi.atspi_accessible_get_name, thing)?))
        }

        /// What a thing is called, written down in quotes; and "None" for no thing.
        fn called(&self, thing: Thing) -> Result<String, Gone> {
            if thing.is_none() { Ok("None".to_string()) } else { Ok(quoted(&self.name(thing)?)) }
        }

        fn role(&self, thing: Thing) -> Result<c_int, Gone> {
            self.ask(&self.atspi.atspi_accessible_get_role, thing)
        }

        /// The states a thing is in, by their numbers, lowest first.
        fn states(&self, thing: Thing) -> Vec<c_int> {
            // SAFETY: libatspi hands out the states of a thing (and of none) as a set.
            let held = unsafe { (self.atspi.atspi_accessible_get_state_set.call)(thing) };
            // SAFETY: of that set, libatspi says for any number whether the state is in it.
            let holds = |state: &c_int| unsafe { (self.atspi.atspi_state_set_contains.call)(held, *state) } != 0;
            (0..44).filter(holds).collect()
        }

        /// Whether a thing is in each of these states.
        fn is(&self, thing: Thing, states: &[c_int]) -> bool {
            let held = self.states(thing);
            states.iter().all(|state| held.contains(state))
        }

        fn children(&self, thing: Thing) -> Result<Vec<Thing>, Gone> {
            let count = self.ask(&self.atspi.atspi_accessible_get_child_count, thing)?;
            (0..count).map(|at| self.ask_of(&self.atspi.atspi_accessible_get_child_at_index, thing, at)).collect()
        }

        /// Where a thing is: counted from the corner of the screen (0), or of its window (1).
        fn extents(&self, thing: Thing, counted_from: c_int) -> Result<(c_int, c_int, c_int, c_int), Gone> {
            let place = self.ask_of(&self.atspi.atspi_component_get_extents, thing, counted_from)?;
            if place.is_null() {
                return Ok((-1, -1, -1, -1));
            }
            // SAFETY: libatspi handed over four numbers of its own making.
            let (x, y, w, h) = unsafe { ((*place).x, (*place).y, (*place).w, (*place).h) };
            // SAFETY: they are the caller's to free with glib, and are not read after this.
            unsafe { (self.glib.g_free)(place.cast()) };
            Ok((x, y, w, h))
        }

        /// A thing and everything in it, each with how deep in it is.
        fn below(&self, thing: Thing) -> Result<Vec<(Thing, usize)>, Gone> {
            let (mut found, mut todo) = (Vec::new(), vec![(thing, 0)]);
            while let Some((one, depth)) = todo.pop() {
                found.push((one, depth));
                todo.extend(self.children(one)?.into_iter().rev().map(|child| (child, depth + 1)));
            }
            Ok(found)
        }

        /// A thing and everything in it, each by its kind and what it is called.
        fn told(&self, thing: Thing) -> Result<Vec<(c_int, String)>, Gone> {
            self.below(thing)?.into_iter().map(|(thing, _)| Ok((self.role(thing)?, self.name(thing)?))).collect()
        }

        /// The first thing of a kind, called something, in a window (or none).
        fn find(&self, within: Thing, wanted: c_int, called: &str) -> Result<Thing, Gone> {
            for (thing, _) in self.below(within)? {
                if self.role(thing)? == wanted && self.name(thing)? == called {
                    return Ok(thing);
                }
            }
            Ok(Thing::NONE)
        }

        /// Whether a thing of a kind, called something, is in a window.
        fn has(&self, within: Thing, wanted: c_int, called: &str) -> Result<bool, Gone> {
            Ok(!self.find(within, wanted, called)?.is_none())
        }

        fn press(&self, thing: Thing) -> Result<bool, Gone> {
            Ok(self.ask_of(&self.atspi.atspi_action_do_action, thing, 0)? != 0)
        }

        /// One of the things noted on a thing beside its kind and its name (or nothing).
        fn noted(&self, thing: Thing, what: &CStr) -> Result<Option<String>, Gone> {
            let all = self.ask(&self.atspi.atspi_accessible_get_attributes, thing)?;
            if all.is_null() {
                return Ok(None);
            }
            // SAFETY: libatspi handed over a table of texts by their names. What is found in
            // it ends in a zero, and is read while the table is there.
            let read = unsafe {
                let found = (self.glib.g_hash_table_lookup)(all, what.as_ptr());
                (!found.is_null()).then(|| CStr::from_ptr(found).to_string_lossy().into_owned())
            };
            // SAFETY: the table is the caller's, and is given back now that it is read.
            unsafe { (self.glib.g_hash_table_unref)(all) };
            Ok(read)
        }

        /// What is at a place in a window, counted from the window's corner.
        fn at_place(&self, within: Thing, x: c_int, y: c_int) -> Result<Thing, Gone> {
            let (asked, mut problem) = (&self.atspi.atspi_component_get_accessible_at_point, null_mut());
            // SAFETY: as in `ask`; the place is counted from the corner of the window (1).
            let found = unsafe { (asked.call)(within, x, y, 1, &mut problem) };
            self.answered(asked.name, found, problem)
        }

        /// Moves a slider to where the screen reader wants it.
        fn set_value(&self, slider: Thing, to: f64) -> Result<bool, Gone> {
            let (asked, mut problem) = (&self.atspi.atspi_value_set_current_value, null_mut());
            // SAFETY: as in `ask`; libatspi takes any amount.
            let done = unsafe { (asked.call)(slider, to, &mut problem) };
            Ok(self.answered(asked.name, done, problem)? != 0)
        }

        /// All the text of a text box.
        fn text(&self, of: Thing) -> Result<String, Gone> {
            let (asked, mut problem) = (&self.atspi.atspi_text_get_text, null_mut());
            // SAFETY: as in `ask`; from the first letter to the last (-1).
            let words = unsafe { (asked.call)(of, 0, -1, &mut problem) };
            Ok(self.words(self.answered(asked.name, words, problem)?))
        }

        /// The word of a text box that a letter is in: its letters, and from where to where it goes.
        fn word_at(&self, of: Thing, letter: c_int) -> Result<(String, c_int, c_int), Gone> {
            let (asked, mut problem) = (&self.atspi.atspi_text_get_string_at_offset, null_mut());
            // SAFETY: as in `ask`; what is asked for around the letter is its word (1).
            let stretch = unsafe { (asked.call)(of, letter, 1, &mut problem) };
            let stretch = self.answered(asked.name, stretch, problem)?;
            if stretch.is_null() {
                return Ok((String::new(), -1, -1));
            }
            // SAFETY: libatspi handed over a stretch of text of its own making.
            let (start, end, content) = unsafe { ((*stretch).start, (*stretch).end, (*stretch).content) };
            let word = self.words(content);
            // SAFETY: the stretch is the caller's to free with glib (its letters went back in
            // `words`), and is not read after this.
            unsafe { (self.glib.g_free)(stretch.cast()) };
            Ok((word, start, end))
        }

        /// Puts a text into a text box in place of what is there.
        fn put_text(&self, into: Thing, text: &CStr) -> Result<bool, Gone> {
            let (asked, mut problem) = (&self.atspi.atspi_editable_text_set_text_contents, null_mut());
            // SAFETY: as in `ask`; the text ends in a zero.
            let done = unsafe { (asked.call)(into, text.as_ptr(), &mut problem) };
            Ok(self.answered(asked.name, done, problem)? != 0)
        }

        /// Puts a text into a text box before one of its letters.
        fn insert_text(&self, into: Thing, before: c_int, text: &CStr) -> Result<bool, Gone> {
            let (asked, mut problem) = (&self.atspi.atspi_editable_text_insert_text, null_mut());
            // SAFETY: as in `ask`; the text ends in a zero, and is as long as it is said to be.
            let done = unsafe { (asked.call)(into, before, text.as_ptr(), text.count_bytes() as c_int, &mut problem) };
            Ok(self.answered(asked.name, done, problem)? != 0)
        }

        /// The programs of the desktop that are the demo.
        fn apps(&self, desktop: Thing) -> Result<Vec<Thing>, Gone> {
            let mut apps = self.children(desktop)?;
            for at in (0..apps.len()).rev() {
                if apps[at].is_none() || self.name(apps[at])? != self.program {
                    apps.remove(at);
                }
            }
            Ok(apps)
        }

        /// What the programs of the desktop are called, written down as a list.
        fn programs(&self, desktop: Thing) -> Result<String, Gone> {
            let names: Result<Vec<String>, Gone> = self.children(desktop)?.into_iter().map(|app| self.name(app)).collect();
            Ok(listed(&names?))
        }

        /// Lets libatspi take in what has come for it: its news arrives here.
        fn pump(&self, seconds: f64) {
            let until = Instant::now() + Duration::from_secs_f64(seconds);
            loop {
                // SAFETY: glib's own loop is run, without waiting, for as long as it finds something to do.
                while unsafe { (self.glib.g_main_context_iteration)(null_mut(), 0) } != 0 {}
                if Instant::now() >= until {
                    return;
                }
                rest(10);
            }
        }

        /// Waits until something holds, for some seconds at the most; says whether it came to
        /// hold. (What libatspi could not get an answer for is asked again.)
        fn wait(&self, seconds: f64, holds: impl Fn() -> Result<bool, Gone>) -> bool {
            let until = Instant::now() + Duration::from_secs_f64(seconds);
            while Instant::now() < until {
                self.pump(0.0);
                if holds().unwrap_or(false) {
                    return true;
                }
                rest(20);
            }
            false
        }

        /// Asks for news, as a screen reader asks for it: each piece is noted as it comes.
        fn listen(&self) -> Result<(), Gone> {
            let (asked, noted): (_, *const RefCell<Vec<News>>) = (&self.atspi.atspi_event_listener_register, &*self.news);
            // SAFETY: `note` is what libatspi calls with each piece of news and with the
            // pointer given here: the list of news, which stays where it is for the whole walk.
            let listener = unsafe { (self.atspi.atspi_event_listener_new.call)(note, noted as *mut c_void, null_mut()) };
            for kind in NEWS {
                let mut problem = null_mut();
                // SAFETY: the listener was made above, and the kind of news ends in a zero.
                let done = unsafe { (asked.call)(listener, kind.as_ptr(), &mut problem) };
                self.answered(asked.name, done, problem)?;
            }
            Ok(())
        }

        /// Empties the list of news: what is heard from here on came after this.
        fn forget_news(&self) {
            self.news.borrow_mut().clear();
        }

        /// Whether news of a kind has come since the list was last emptied (about one thing,
        /// or about any where none is named; with one number, or with any).
        fn heard(&self, kind: &str, of: Thing, number: Option<c_int>) -> bool {
            self.news.borrow().iter().any(|told| told.kind.starts_with(kind) && (of.is_none() || told.of == of) && number.is_none_or(|number| told.number == number))
        }

        /// The thing the first news of a kind, with one number, is about (or none).
        fn first_heard_of(&self, kind: &str, number: c_int) -> Thing {
            self.news.borrow().iter().find(|told| told.kind.starts_with(kind) && told.number == number).map_or(Thing::NONE, |told| told.of)
        }
    }

    /// The things of the demo the walk comes back to: the desktop it is on, the program, its
    /// window, and in the window a button, the tick box, and the text that counts the presses.
    #[derive(Clone, Copy)]
    struct Seen {
        desktop: Thing,
        app: Thing,
        frame: Thing,
        count_one: Thing,
        tick: Thing,
        count: Thing,
    }

    /// Whether every thing in a window has a place, and what shows is in the window with
    /// some of itself at the least. (The first of them is the window. What is scrolled out
    /// of view is told all the same, as not showing.)
    fn each_is_in_the_window(reader: &Reader, tree: &[(Thing, usize)]) -> Result<bool, Gone> {
        for (thing, _) in &tree[1..] {
            let (x, y, w, h) = reader.extents(*thing, 1)?;
            let inside = x < 900 && y < 600 && x + w > 0 && y + h > 0;
            if !(w > 0 && h > 0) || reader.is(*thing, &[SHOWING]) && !inside {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether every thing is in what it says it is in, at the place it says.
    fn each_is_where_it_says(reader: &Reader, tree: &[(Thing, usize)]) -> Result<bool, Gone> {
        let mut pairs = Vec::new();
        for (thing, _) in tree {
            pairs.extend(reader.children(*thing)?.into_iter().enumerate().map(|(at, child)| (*thing, at as c_int, child)));
        }
        for (thing, at, child) in pairs {
            if reader.ask(&reader.atspi.atspi_accessible_get_index_in_parent, child)? != at || reader.ask(&reader.atspi.atspi_accessible_get_parent, child)? != thing {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // ---- someone listens ----

    /// Someone listens: the demo comes among the programs of the desktop, as an application
    /// with one window. Gives back the program.
    fn someone_listens(reader: &Reader, desktop: Thing, started: &mut Started) -> Result<Thing, Gone> {
        let (atspi, demo) = (&reader.atspi, started.demo.id());
        let began = Instant::now();
        listening(true);
        let arrived = reader.wait(5.0, || Ok(reader.apps(desktop)?.len() == 1));
        check("someone listens: the demo comes among the programs of the desktop", arrived, &reader.programs(desktop)?);
        if !arrived {
            end(started);
            std::process::exit(1);
        }
        let run_by: Vec<String> = running_the_bus(started).iter().map(|process| fs::read_to_string(format!("/proc/{process}/comm")).unwrap_or_default().trim().to_string()).collect();
        println!("     it was there {:.2} s after the word, on a bus run by {}; it has {} sockets now", began.elapsed().as_secs_f64(), run_by.join(" and "), sockets(demo));
        check("  it is on the accessibility bus as well now (three sockets)", sockets(demo) == 3, "");
        let app = reader.apps(desktop)?[0];
        // (libatspi remembers what it was told of a program, as it does for Orca: so what is read
        // from here on is only right if the demo sends the news of what changes.)
        // SAFETY: the program is a thing libatspi handed out; 0x7f is "remember everything".
        unsafe { (atspi.atspi_accessible_set_cache_mask.call)(app, 0x7f) };
        let (role, toolkit, windows) = (reader.role(app)?, reader.words(reader.ask(&atspi.atspi_accessible_get_toolkit_name, app)?), reader.children(app)?.len());
        check("the program: an application of the toolkit taut, with one window in it", (role, toolkit.as_str(), windows) == (APPLICATION, "taut", 1), &format!("{role} {toolkit} {windows}"));
        Ok(app)
    }

    /// The window of the demo, and all that is in it, is told of as it is. Gives back the
    /// things of it the walk comes back to.
    fn the_window_is_told(reader: &Reader, desktop: Thing, app: Thing) -> Result<Seen, Gone> {
        let atspi = &reader.atspi;
        let frame = reader.children(app)?.first().copied().unwrap_or(Thing::NONE);
        let (role, name, place) = (reader.role(frame)?, reader.name(frame)?, reader.extents(frame, 1)?);
        let as_big_as_it_is = (role, name.as_str()) == (FRAME, "taut gallery") && (place.2, place.3) == (900, 600);
        check("the window: a frame with its title, as big as it is", as_big_as_it_is, &format!("{role} {} {place:?}", quoted(&name)));
        let on_screen = reader.extents(frame, 0)?;
        check("the window is where X has it on the screen", (on_screen.0, on_screen.1) == (0, 0), &format!("{on_screen:?}"));
        check("the window is showing and can be used", reader.is(frame, &[ENABLED, SENSITIVE, SHOWING, VISIBLE]), &format!("{:?}", reader.states(frame)));

        // The whole tree: every thing has a name or a kind worth telling, is inside the window, and
        // is in what it says it is in, at the place it says.
        let tree = reader.below(frame)?;
        println!("     {} objects in the window; the first of them:", tree.len());
        for (thing, depth) in tree.iter().take(28) {
            let kind = reader.words(reader.ask(&atspi.atspi_accessible_get_role_name, *thing)?);
            println!("       {}{kind}: {} at {:?}", "  ".repeat(*depth), quoted(&reader.name(*thing)?), reader.extents(*thing, 1)?);
        }
        check("every thing in it has a place, and what shows is in the window with some of itself at the least", each_is_in_the_window(reader, &tree)?, "");
        // The last row of the list is far below the window's fold: told all the same, as
        // there to be seen but not showing; scrolled to, it shows. (And back to the first.)
        let (last, first) = (reader.find(frame, LABEL, "item 59")?, reader.find(frame, LABEL, "item 0")?);
        check("what is out of view is told, as not showing", reader.is(last, &[VISIBLE]) && !reader.is(last, &[SHOWING]), "");
        let scrolled = reader.ask_of(&reader.atspi.atspi_component_scroll_to, last, 6)? != 0 && reader.wait(5.0, || Ok(reader.is(last, &[SHOWING])));
        check("  and a screen reader has it scrolled into view", scrolled && !reader.is(first, &[SHOWING]), "");
        let back = reader.ask_of(&reader.atspi.atspi_component_scroll_to, first, 6)? != 0 && reader.wait(5.0, || Ok(reader.is(first, &[SHOWING])));
        check("  and back", back, "");
        check("every thing is in what it says it is in, at the place it says", each_is_where_it_says(reader, &tree)?, "");
        let not_told = missing(&SHOWN, &reader.told(frame)?);
        check("what the window shows is told: buttons, tabs, a tick box, text boxes, the rows of the list", not_told.is_empty(), &format!("missing {}", listed_pairs(&not_told)));
        let (count_one, tick, count) = (reader.find(frame, BUTTON, "Count one")?, reader.find(frame, CHECK_BOX, TICK_BOX)?, reader.find(frame, LABEL, COUNTED)?);
        let (ways, way) = (&atspi.atspi_action_get_n_actions, &atspi.atspi_action_get_action_name);
        let by_a_click = || Ok::<bool, Gone>(reader.ask(ways, count_one)? == 1 && reader.words(reader.ask_of(way, count_one, 0)?) == "click");
        check("a button can be pressed, and the keyboard can be on it", reader.is(count_one, &[ENABLED, SENSITIVE, SHOWING, VISIBLE, FOCUSABLE]) && by_a_click()?, "");
        let (list_tab, parts_tab) = (reader.find(frame, PAGE_TAB, "List")?, reader.find(frame, PAGE_TAB, "Parts")?);
        check("the tab that is picked says so, and the others say they are not", reader.is(list_tab, &[SELECTED]) && !reader.is(parts_tab, &[SELECTED]), "");
        let which = (reader.noted(parts_tab, c"posinset")?, reader.noted(parts_tab, c"setsize")?, reader.noted(count_one, c"posinset")?);
        let one_of = format!("{} of {}", which.0.as_deref().unwrap_or("None"), which.1.as_deref().unwrap_or("None"));
        check("  and each says which one of how many it is", which == (Some("3".to_string()), Some("9".to_string()), None), &one_of);
        let (x, y, w, h) = reader.extents(count_one, 1)?;
        let at_place = reader.at_place(frame, x + w / 2, y + h / 2)?;
        check("what is at a place in the window is found", at_place == count_one, &reader.called(at_place)?);
        // SAFETY: both are things libatspi handed out; it says of the one whether it can be
        // pressed, and of the other whether it holds an amount.
        let nothing_to_press = unsafe { (atspi.atspi_accessible_get_action_iface.call)(count).is_null() && (atspi.atspi_accessible_get_value_iface.call)(count_one).is_null() };
        check("a text has nothing to press", nothing_to_press, "");
        Ok(Seen { desktop, app, frame, count_one, tick, count })
    }

    // ---- working it ----

    /// A button and a tick box are pressed for the screen reader, and what that changed is
    /// told of.
    fn buttons_are_pressed(reader: &Reader, seen: &Seen) -> Result<(), Gone> {
        let Seen { frame, count_one, tick, count, .. } = *seen;
        reader.forget_news();
        check("a button is pressed for the screen reader", reader.press(count_one)?, "");
        let renamed = reader.wait(5.0, || Ok(reader.heard("object:property-change:accessible-name", count, None)));
        check("  what it changed is told of: a text is called otherwise", renamed && reader.name(count)? == "Gallery: 60 items, 1 presses counted", &quoted(&reader.name(count)?));

        reader.forget_news();
        reader.press(reader.find(frame, BUTTON, "Start over")?)?;
        let said = reader.wait(5.0, || Ok(reader.heard("object:announcement", frame, None)));
        check("what the app has said about something it did is said", said && reader.name(count)? == COUNTED, &quoted(&reader.name(count)?));
        reader.press(count_one)?;
        reader.wait(5.0, || Ok(reader.name(count)? == "Gallery: 60 items, 1 presses counted"));

        reader.forget_news();
        check("a tick box is ticked for the screen reader", !reader.is(tick, &[CHECKED]) && reader.press(tick)?, "");
        let ticked = reader.wait(5.0, || Ok(reader.heard("object:state-changed:checked", tick, Some(1))));
        check("  that it is ticked now arrives as news, and is how it stands", ticked && reader.is(tick, &[CHECKED]), &format!("{:?}", reader.states(tick)));
        reader.forget_news();
        reader.press(tick)?;
        let unticked = reader.wait(5.0, || Ok(reader.heard("object:state-changed:checked", tick, Some(0))));
        check("  and that it is not, when it is pressed again", unticked && !reader.is(tick, &[CHECKED]), "");
        Ok(())
    }

    /// Another page: what was on the first one goes, what is on this one comes. Its slider is
    /// moved for the screen reader.
    fn another_page_is_picked(reader: &Reader, seen: &Seen) -> Result<(), Gone> {
        let (atspi, frame) = (&reader.atspi, seen.frame);
        reader.forget_news();
        reader.press(reader.find(frame, PAGE_TAB, "Parts")?)?;
        let came = reader.wait(5.0, || reader.has(frame, SLIDER, "A slider"));
        let news_of_it = || reader.heard("object:children-changed:add", frame, None) && reader.heard("object:children-changed:remove", frame, None);
        check("another page is picked: its things come, and the news of that", came && news_of_it() && !reader.has(frame, ENTRY, "Notes")?, "");
        check("  the tab says it is the one picked now", reader.is(reader.find(frame, PAGE_TAB, "Parts")?, &[SELECTED]) && !reader.is(reader.find(frame, PAGE_TAB, "List")?, &[SELECTED]), "");
        let slider = reader.find(frame, SLIDER, "A slider")?;
        let stands = || reader.ask(&atspi.atspi_value_get_current_value, slider);
        let (low, high, stood) = (reader.ask(&atspi.atspi_value_get_minimum_value, slider)?, reader.ask(&atspi.atspi_value_get_maximum_value, slider)?, stands()?);
        check("a slider says where it stands, and between what", (low, high) == (50.0, 100.0) && low <= stood && stood <= high, &format!("{low:?} {stood:?} {high:?}"));
        // (The keyboard moves this slider a twentieth of its way at a step, which is 2.5 here: the
        // screen reader's wish is met as near as those steps come, and no further than its ends.)
        for to in [80.0, 61.0, 500.0, 50.0, 71.0] {
            reader.forget_news();
            let stood = stands()?;
            reader.set_value(slider, to)?;
            let reachable = (-20..=20).map(|steps| (stood + 2.5 * steps as f64).max(low).min(high));
            let nearest = reachable.min_by(|one, other| (one - to).abs().total_cmp(&(other - to).abs())).unwrap_or(stood);
            let is_there = || Ok((stands()? - nearest).abs() < 0.01);
            let there = reader.wait(8.0, is_there);
            reader.pump(0.3);
            let stays = is_there()?;
            let moved = there && stays && reader.heard("object:property-change:accessible-value", slider, None);
            check(&format!("  it is moved to {} for the screen reader, as near as its steps come ({})", short(to), short(nearest)), moved, &format!("{:?}", stands()?));
        }
        let bar = reader.find(frame, PROGRESS_BAR, "")?;
        let as_far = || Ok::<bool, Gone>((reader.ask(&atspi.atspi_value_get_current_value, bar)? - (stands()? - 50.0) / 50.0).abs() < 0.001);
        check("how far something has come is told as an amount too", !bar.is_none() && as_far()?, "");
        Ok(())
    }

    /// The page with the table: libatspi, by its own functions for tables, reads it as a
    /// table of all its rows and its columns, a cell at a row in view and a column, where
    /// the cell says it is, under the heading of its column. A row out of view has no cells
    /// to be had.
    fn a_table(reader: &Reader, seen: &Seen) -> Result<(), Gone> {
        let (atspi, frame) = (&reader.atspi, seen.frame);
        reader.press(reader.find(frame, PAGE_TAB, "Table")?)?;
        let find = |role: c_int| Ok::<Thing, Gone>(reader.below(frame)?.into_iter().map(|(thing, _)| thing).find(|thing| reader.role(*thing).is_ok_and(|kind| kind == role)).unwrap_or(Thing::NONE));
        check("the page with the table is picked, and the table is told as one", reader.wait(5.0, || Ok(!find(TABLE)?.is_none())), "");
        let table = find(TABLE)?;
        let (rows, columns, picked) =
            (reader.ask(&atspi.atspi_table_get_n_rows, table)?, reader.ask(&atspi.atspi_table_get_n_columns, table)?, reader.ask(&atspi.atspi_table_get_n_selected_rows, table)?);
        check("  libatspi reads it as a table: all its rows, its columns, which rows are picked", (rows, columns, picked) == (5000, 4, 0), &format!("{rows} rows, {columns} columns, {picked} picked"));
        let heading = reader.ask_of(&atspi.atspi_table_get_column_header, table, 1)?;
        check("  the heading over a column", !heading.is_none() && reader.name(heading)? == "Size", "");
        let at = |row: c_int, column: c_int| {
            let mut problem = null_mut();
            // SAFETY: libatspi's function of this kind, on a table it handed out; `problem` is
            // room for what went wrong.
            let cell = unsafe { (atspi.atspi_table_get_accessible_at.call)(table, row, column, &mut problem) };
            reader.answered(atspi.atspi_table_get_accessible_at.name, cell, problem)
        };
        let cell = at(2, 1)?;
        let (mut row, mut column, mut problem) = (-1, -1, null_mut());
        // SAFETY: as above, with room for the row and the column.
        let placed = unsafe { (atspi.atspi_table_cell_get_position.call)(cell, &mut row, &mut column, &mut problem) };
        reader.answered(atspi.atspi_table_cell_get_position.name, placed, problem)?;
        let in_it = reader.ask(&atspi.atspi_table_cell_get_table, cell)?;
        let what = format!("{} {} at {row},{column}", reader.role(cell)?, quoted(&reader.name(cell)?));
        check(
            "  a cell by its row and column: where it says it is, in the table, in its row",
            !cell.is_none() && reader.role(cell)? == TABLE_CELL && (row, column) == (2, 1) && in_it.0 == table.0,
            &what,
        );
        check("    and it is in a row of the table", reader.role(reader.ask(&atspi.atspi_accessible_get_parent, cell)?)? == TABLE_ROW && reader.name(cell)?.ends_with(" KB"), &what);
        check("  a row out of view has no cell to be had", at(4000, 0)?.is_none(), "");
        Ok(())
    }

    /// The page of panels, the first of which holds a long tree of files: libatspi reads it
    /// as a tree in which many can be picked, and its items with how deep each is and which
    /// of how many (as Orca reads them); it opens one by its second action, picks one by the
    /// tree's own way of picking, and closes the first again.
    fn a_tree(reader: &Reader, seen: &Seen) -> Result<(), Gone> {
        let (atspi, frame) = (&reader.atspi, seen.frame);
        reader.press(reader.find(frame, PAGE_TAB, "Dock")?)?;
        let find = |role: c_int| Ok::<Thing, Gone>(reader.below(frame)?.into_iter().map(|(thing, _)| thing).find(|thing| reader.role(*thing).is_ok_and(|kind| kind == role)).unwrap_or(Thing::NONE));
        check("the page of panels is picked, and the tree in it is told as one", reader.wait(5.0, || Ok(!find(TREE)?.is_none())), "");
        let tree = find(TREE)?;
        // How deep an item is, from 1, which one it is of the items beside it, and of how many.
        let noted = |item: Thing| Ok::<[String; 3], Gone>([c"level", c"posinset", c"setsize"].map(|what| reader.noted(item, what).ok().flatten().unwrap_or_default()));
        let first = reader.find(tree, TREE_ITEM, "Folder 000")?;
        let (way, place) = (reader.words(reader.ask_of(&atspi.atspi_action_get_action_name, first, 1)?), noted(first)?);
        let as_it_is = reader.is(tree, &[MULTISELECTABLE]) && reader.is(first, &[EXPANDABLE, COLLAPSED]) && place == ["1", "1", "1000"] && way == "expand";
        check("  one in which many can be picked; its first folder is closed, the first of a thousand at the top", as_it_is, &format!("{place:?} {way}"));
        reader.forget_news();
        reader.ask_of(&atspi.atspi_action_do_action, first, 1)?;
        let opened = reader.wait(5.0, || Ok(reader.heard("object:state-changed:expanded", first, Some(1)) && reader.is(first, &[EXPANDED]) && reader.has(tree, TREE_ITEM, "Inner 1")?));
        let inner = reader.find(tree, TREE_ITEM, "Inner 1")?;
        let place = noted(inner)?;
        check("  its second action opens it, the news says so, and what is under it is a step deeper", opened && place == ["2", "2", "100"], &format!("{place:?}"));
        let at = reader.ask(&atspi.atspi_accessible_get_index_in_parent, inner)?;
        reader.forget_news();
        let asked = reader.ask_of(&atspi.atspi_selection_select_child, tree, at)? != 0;
        let picked = reader.wait(5.0, || Ok(reader.ask_of(&atspi.atspi_selection_is_child_selected, tree, at)? != 0 && reader.is(inner, &[SELECTED])));
        let count = reader.ask(&atspi.atspi_selection_get_n_selected_children, tree)?;
        check(
            "  the tree picks an item for the screen reader, and says it is the one picked",
            asked && picked && count == 1 && reader.heard("object:state-changed:selected", inner, Some(1)),
            &count.to_string(),
        );
        let way = reader.words(reader.ask_of(&atspi.atspi_action_get_action_name, first, 1)?);
        reader.ask_of(&atspi.atspi_action_do_action, first, 1)?;
        let closed = reader.wait(5.0, || Ok(reader.is(first, &[COLLAPSED]) && !reader.has(tree, TREE_ITEM, "Inner 1")?));
        check("  and the same action, now called so, closes it again", way == "collapse" && closed, &way);
        Ok(())
    }

    /// Back to the list, and text into the box that filters it.
    fn text_is_put_in(reader: &Reader, seen: &Seen) -> Result<(), Gone> {
        let (atspi, frame) = (&reader.atspi, seen.frame);
        reader.press(reader.find(frame, PAGE_TAB, "List")?)?;
        check("the first page is picked again", reader.wait(5.0, || Ok(reader.has(frame, ENTRY, "Filter items")? && reader.has(frame, LABEL, "item 2")?)), "");
        let filter = reader.find(frame, ENTRY, "Filter items")?;
        reader.forget_news();
        check("text is put into a text box for the screen reader", reader.is(filter, &[EDITABLE]) && reader.put_text(filter, c"item 1")?, "");
        let typed = reader.wait(5.0, || Ok(reader.text(filter)? == "item 1"));
        let letter_for_letter = typed && reader.heard("object:text-changed:insert", filter, None) && reader.ask(&atspi.atspi_text_get_character_count, filter)? == 6;
        check("  the box has it, letter for letter, and the news of it came", letter_for_letter, &quoted(&reader.text(filter)?));
        let cursor = || reader.ask(&atspi.atspi_text_get_caret_offset, filter);
        check("  the cursor is told as standing after it", cursor()? == 6, &cursor()?.to_string());
        let (word, start, end) = reader.word_at(filter, 1)?;
        check("  a word of it is read out by a letter in it", (word.as_str(), start, end) == ("item ", 0, 5), &format!("b{} {start} {end}", quoted(&word)));
        let filtered = || Ok(!reader.has(frame, LABEL, "item 2")? && reader.has(frame, LABEL, "item 10")? && reader.heard("object:children-changed:remove", Thing::NONE, None));
        check("  the list under it shows what the app made of that", reader.wait(5.0, filtered), "");
        reader.insert_text(filter, 4, c"s")?;
        check("  a letter is put in among the others", reader.wait(5.0, || Ok(reader.text(filter)? == "items 1")), "");
        reader.put_text(filter, c"")?;
        let emptied = reader.wait(5.0, || Ok(reader.text(filter)?.is_empty() && reader.has(frame, LABEL, "item 2")?));
        check("  and the box is emptied again", emptied && reader.heard("object:text-changed:delete", filter, None), "");
        Ok(())
    }

    // ---- the keyboard ----

    /// The keyboard and the pointer of the display, worked with the X server's own means of
    /// testing: what the walk asks of Xlib and of its test extension (libXtst), and the
    /// display that is open for it.
    #[allow(non_snake_case)]
    struct Keys {
        display: *mut c_void,
        XKeysymToKeycode: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_uchar,
        XSync: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
        XTestFakeKeyEvent: unsafe extern "C" fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int,
        XTestFakeMotionEvent: unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_ulong) -> c_int,
    }

    /// Opens Xlib and its test extension, and the display the environment names.
    fn open_keys() -> Keys {
        let (xlib, xtst) = (library(c"libX11.so.6"), library(c"libXtst.so.6"));
        // SAFETY: each function is declared as the headers of Xlib and of XTest declare it;
        // and a display with no name at all is the one the environment names.
        let keys = unsafe {
            let open: unsafe extern "C" fn(*const c_char) -> *mut c_void = function(xlib, c"XOpenDisplay").call;
            Keys {
                display: open(null()),
                XKeysymToKeycode: function(xlib, c"XKeysymToKeycode").call,
                XSync: function(xlib, c"XSync").call,
                XTestFakeKeyEvent: function(xtst, c"XTestFakeKeyEvent").call,
                XTestFakeMotionEvent: function(xtst, c"XTestFakeMotionEvent").call,
            }
        };
        assert!(!keys.display.is_null(), "there is no display to press keys on");
        keys
    }

    impl Keys {
        /// A key goes down and comes up, as if the person had pressed it.
        fn key(&self, sym: c_ulong) {
            // SAFETY: the display is open; the key that X has for the number goes down and
            // comes up at once, and X is waited for until it has taken that in.
            unsafe {
                let key = (self.XKeysymToKeycode)(self.display, sym) as c_uint;
                (self.XTestFakeKeyEvent)(self.display, key, 1, 0);
                (self.XTestFakeKeyEvent)(self.display, key, 0, 0);
                (self.XSync)(self.display, 0);
            }
        }

        /// The pointer goes to a place on the screen.
        fn point(&self, x: c_int, y: c_int) {
            // SAFETY: the display is open; the pointer of its first screen is moved at once.
            unsafe {
                (self.XTestFakeMotionEvent)(self.display, 0, x, y, 0);
                (self.XSync)(self.display, 0);
            }
        }
    }

    /// Tab is pressed, and the news says where the keyboard went; then the screen reader
    /// sends the keyboard to a thing itself.
    fn the_keyboard(reader: &Reader, seen: &Seen, keys: &Keys) -> Result<(), Gone> {
        // (With no window manager on this display, the keyboard is with the window the pointer is over.)
        keys.point(450, 400);
        reader.pump(0.3);
        let mut on: Vec<Thing> = Vec::new();
        for _ in 0..3 {
            reader.forget_news();
            keys.key(TAB);
            let came = reader.wait(3.0, || Ok(reader.heard("object:state-changed:focused", Thing::NONE, Some(1))));
            let onto = reader.first_heard_of("object:state-changed:focused", 1);
            check(&format!("Tab is pressed: the news says where the keyboard went ({})", reader.called(onto)?), came && !onto.is_none() && reader.is(onto, &[FOCUSED]) && !on.contains(&onto), "");
            if let (Some(last), false) = (on.last(), onto.is_none()) {
                check("  and that it left where it was", reader.heard("object:state-changed:focused", *last, Some(0)) && !reader.is(*last, &[FOCUSED]), "");
            }
            on.push(onto);
        }
        reader.forget_news();
        // (The tick box is looked for again: the walk has been to other pages and back since it
        // was first found, and coming back made it another object.)
        let (tick, last) = (reader.find(seen.frame, CHECK_BOX, TICK_BOX)?, on.last().copied().unwrap_or(Thing::NONE));
        let went = || reader.wait(5.0, || Ok(reader.heard("object:state-changed:focused", tick, Some(1)) && reader.is(tick, &[FOCUSED])));
        check("the screen reader sends the keyboard to a thing itself", reader.ask(&reader.atspi.atspi_component_grab_focus, tick)? != 0 && went() && !reader.is(last, &[FOCUSED]), "");
        Ok(())
    }

    // ---- a menu ----

    /// A menu is opened: its rows are told of, and Escape puts it away.
    fn a_menu(reader: &Reader, seen: &Seen, keys: &Keys) -> Result<(), Gone> {
        let frame = seen.frame;
        reader.press(reader.find(frame, BUTTON, "File")?)?;
        let came = reader.wait(5.0, || reader.has(frame, MENU_ITEM, "Rename this window…"));
        let rename = reader.find(frame, MENU_ITEM, "Rename this window…")?;
        let keys_of_it = || Ok::<String, Gone>(reader.words(reader.ask_of(&reader.atspi.atspi_action_get_key_binding, rename, 0)?));
        let written = if rename.is_none() { "None".to_string() } else { quoted(&keys_of_it()?) };
        check("a row of a menu is called by its name, and the keys that do the same are told apart from it", came && keys_of_it()? == ";;F2", &written);
        check("  what is under the menu is told of as before", reader.find(frame, BUTTON, "Count one")? == seen.count_one, "");
        keys.key(ESCAPE);
        check("  and Escape puts the menu away", reader.wait(5.0, || Ok(!reader.has(frame, MENU_ITEM, "Rename this window…")?)), "");
        Ok(())
    }

    // ---- a second window ----

    /// A second window is opened from a menu, used, and closed by its own button.
    fn a_second_window(reader: &Reader, seen: &Seen, keys: &Keys) -> Result<(), Gone> {
        let Seen { app, frame, count_one, .. } = *seen;
        let row = || reader.find(frame, CHECK_MENU_ITEM, "A second window");
        reader.forget_news();
        reader.press(reader.find(frame, BUTTON, "View")?)?;
        check("a menu is opened: its rows come, each saying whether it is ticked", reader.wait(5.0, || Ok(!row()?.is_none())) && !reader.is(row()?, &[CHECKED]), "");
        reader.forget_news();
        reader.press(row()?)?;
        let opened = reader.wait(5.0, || Ok(reader.children(app)?.len() == 2));
        let news_of_it = || reader.heard("object:children-changed:add", app, None) && reader.heard("window:create", Thing::NONE, None);
        check("a second window is opened from it: it comes as another frame of the program", opened && news_of_it(), "");
        let second = reader.children(app)?.get(1).copied().unwrap_or(Thing::NONE);
        let (titled, told) = ((reader.role(second)?, reader.name(second)?.as_str()) == (FRAME, "taut gallery: a second window"), reader.told(second)?);
        check("  with its own title, and its own things", titled && missing(&SHOWN_IN_THE_SECOND, &told).is_empty(), &listed_pairs(&told));
        check("  the menu is gone from the first window", row()?.is_none(), "");
        reader.press(reader.find(frame, BUTTON, "View")?)?;
        check("  opened again, its row for that window is ticked", reader.wait(5.0, || Ok(!row()?.is_none())) && reader.is(row()?, &[CHECKED]), "");
        keys.key(ESCAPE);
        check("  and Escape puts it away", reader.wait(5.0, || Ok(row()?.is_none())), "");
        let renamed = reader.find(second, ENTRY, "The name of the first window")?;
        reader.put_text(renamed, c"renamed")?;
        check("  what is typed in the second window is told of in the first", reader.wait(5.0, || reader.has(frame, LABEL, "renamed: 60 items, 1 presses counted")), "");
        reader.forget_news();
        reader.press(reader.find(second, BUTTON, "Close this window")?)?;
        let closed = reader.wait(5.0, || Ok(reader.children(app)?.len() == 1 && reader.heard("object:children-changed:remove", app, None)));
        check("it is closed by its own button: the frame goes, and the news of that comes", closed && reader.heard("window:destroy", Thing::NONE, None), "");
        let first = reader.children(app)?.first().copied().unwrap_or(Thing::NONE);
        check("  the first window is still told of", reader.name(first)? == "taut gallery" && reader.find(frame, BUTTON, "Count one")? == count_one, "");
        Ok(())
    }

    // ---- nobody listens any more, and then someone does again ----

    /// Nobody listens any more: the demo leaves the accessibility bus. Then someone listens
    /// again, and it is back.
    fn nobody_listens_and_then_someone_again(reader: &Reader, seen: &Seen, demo: u32) -> Result<(), Gone> {
        let desktop = seen.desktop;
        println!("     with someone listening: {}", memory(demo));
        listening(false);
        check("nobody listens any more: the demo leaves the accessibility bus", reader.wait(5.0, || Ok(sockets(demo) == 2 && reader.apps(desktop)?.is_empty())), &format!("{} sockets", sockets(demo)));
        rest(500);
        println!("     with nobody listening again: {}", memory(demo));
        listening(true);
        let windows = || reader.children(reader.apps(desktop)?.first().copied().unwrap_or(Thing::NONE));
        let back = reader.wait(5.0, || Ok(reader.apps(desktop)?.len() == 1 && windows()?.len() == 1));
        check("someone listens again: it is back, with its window", back && reader.name(windows()?.first().copied().unwrap_or(Thing::NONE))? == "taut gallery", "");
        Ok(())
    }

    // ---- the buses die under it ----

    /// The accessibility bus is killed under the demo, and then the session bus: it lives on,
    /// rests, and still does what the person asks of it.
    fn the_buses_die(started: &mut Started, keys: &Keys) {
        let demo = started.demo.id();
        let before = processor(demo);
        let under = running_the_bus(started);
        under.iter().for_each(|process| end_at_once(process));
        rest(1000);
        let let_go = !under.is_empty() && lives(&mut started.demo) && sockets(demo) == 2;
        check("the accessibility bus is killed under it: the demo lives on, and has let go of that bus", let_go, &format!("killed {}, {} sockets", listed(&under), sockets(demo)));
        let bus = session(SESSION_BUS, "org.freedesktop.DBus.GetConnectionUnixProcessID", &["org.freedesktop.DBus"]);
        // (The answer reads "(uint32 1234,)": the number is the process that runs the session bus.)
        let bus = bus.split_once("uint32 ").map(|(_, number)| number.chars().take_while(char::is_ascii_digit).collect::<String>()).filter(|number| !number.is_empty());
        let _ = started.launcher.kill();
        bus.iter().for_each(|process| end_at_once(process));
        rest(1000);
        let alive = lives(&mut started.demo);
        check("the session bus is killed under it: the demo lives on, with X alone", bus.is_some() && alive && sockets(demo) == 1, &format!("{} sockets", sockets(demo)));
        let idle = processor(demo);
        rest(1000);
        check("and it rests: no processor is used while nothing happens", processor(demo) - idle <= 10, &format!("{} ms in a second", processor(demo) - idle));
        keys.key(TAB);
        rest(300);
        check("and it still does what the person asks of it", lives(&mut started.demo), "");
        println!("     {} ms of processor while its buses died", processor(demo) - before);
    }

    /// The walk from where libatspi starts to where the buses die.
    fn read_and_work(reader: &Reader, started: &mut Started) -> Result<(), Gone> {
        // SAFETY: libatspi starts (0), or says that it had started already (1).
        check("libatspi starts", [0, 1].contains(&unsafe { (reader.atspi.atspi_init.call)() }), "");
        // SAFETY: libatspi has started, and hands out the first desktop: the only one there is.
        let desktop = unsafe { (reader.atspi.atspi_get_desktop.call)(0) };
        reader.pump(0.5);
        check("nobody listens: the demo is not among the programs of the desktop", reader.apps(desktop)?.is_empty(), &reader.programs(desktop)?);
        reader.listen()?;
        let app = someone_listens(reader, desktop, started)?;
        let seen = the_window_is_told(reader, desktop, app)?;
        buttons_are_pressed(reader, &seen)?;
        another_page_is_picked(reader, &seen)?;
        a_table(reader, &seen)?;
        a_tree(reader, &seen)?;
        text_is_put_in(reader, &seen)?;
        let keys = open_keys();
        the_keyboard(reader, &seen, &keys)?;
        a_menu(reader, &seen, &keys)?;
        a_second_window(reader, &seen, &keys)?;
        nobody_listens_and_then_someone_again(reader, &seen, started.demo.id())?;
        the_buses_die(started, &keys);
        Ok(())
    }

    pub fn walk() -> ExitCode {
        let set = |name: &str| env::var_os(name).is_some_and(|value| !value.is_empty());
        if !set("DISPLAY") || !set("DBUS_SESSION_BUS_ADDRESS") {
            eprintln!("usage: xvfb-run -a -s '-screen 0 900x600x24' dbus-run-session -- cargo run -p taut --example walk_reader --release [-- <the gallery>]");
            return ExitCode::FAILURE;
        }
        let gallery = env::args_os().nth(1).map_or_else(the_demo_beside, PathBuf::from);
        let gallery = std::path::absolute(&gallery).unwrap_or(gallery);
        if !gallery.is_file() {
            eprintln!("{} is not there. Build the demo first: cargo build -p taut --example gallery --release", gallery.display());
            return ExitCode::FAILURE;
        }
        let program = gallery.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let reader = Reader { atspi: open_atspi(), glib: open_glib(), program, news: Box::default() };
        alone_it_opens(&gallery);
        let Some(mut started) = start(&gallery) else {
            eprintln!("at-spi-bus-launcher is not on this machine");
            return ExitCode::FAILURE;
        };
        let (alive, demo) = (lives(&mut started.demo), started.demo.id());
        check("nobody listens: the demo is on the session bus only (two sockets: X, and that bus)", alive && sockets(demo) == 2, &format!("{} sockets", sockets(demo)));
        println!("     with nobody listening: {}", memory(demo));
        let walked = read_and_work(&reader, &mut started);
        // SAFETY: the demo is this walk's own child, which is asked to end (15).
        unsafe { kill(demo as c_int, 15) };
        let _ = started.demo.wait();
        end(&mut started);
        if let Err(Gone(what)) = walked {
            eprintln!("libatspi could not get an answer: {what}");
            return ExitCode::FAILURE;
        }
        let failed = FAILED.load(Ordering::Relaxed);
        println!("{}", if failed == 0 { "everything held".to_string() } else { format!("{failed} failed") });
        if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
    }
}
