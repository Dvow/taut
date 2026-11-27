//! Screen readers on Android, on an emulator. One file, two programs:
//!
//! - Built for Android (`cargo run --example apk -- tests/android.rs`), it is an app checking
//!   itself: a page with a paragraph that wraps, a button, a text box and a table of five
//!   hundred rows, read through the library's `AccessibilityNodeProvider` and the
//!   `AccessibilityNodeInfo`s it makes, as the system reads them for a screen reader: text by
//!   line and letter, letters' places, the table as a collection, scrolled a page on and to a
//!   row. Its findings go to the log (`adb logcat -s taut-check`).
//! - Elsewhere, with `TAUT_ANDROID_APK` naming that app and `adb` reaching an emulator, it
//!   installs and starts it, waits for its findings, reads the window as another program does
//!   (`uiautomator dump`), and moves focus with a hardware keyboard's Tab. Without the variable
//!   it does nothing.
//!
//! `TAUT_PICTURES` names a folder for the dumps and the log.
#[cfg(not(target_os = "android"))]
mod on_android;

fn main() {
    #[cfg(target_os = "android")]
    phone::main();
    #[cfg(not(target_os = "android"))]
    host::main();
}

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
extern "C" fn android_main() {
    phone::main()
}

/// The text box's text, read by word.
const NOTES: &str = "h\u{e9}llo w\u{f6}rld";

#[cfg(target_os = "android")]
mod phone {
    use super::NOTES;
    use std::ffi::{CStr, CString, c_char, c_void};
    use std::time::{Duration, Instant};
    use taut::{Table, Ui, Window};

    type Obj = *mut c_void;

    /// The paragraph read by line and letter.
    const PARAGRAPH: &str = "A paragraph long enough to break onto several lines of a phone's window, which a screen reader reads line by line and letter by letter.";

    /// A JNI value (`jvalue`).
    #[repr(C)]
    #[derive(Clone, Copy)]
    union Value {
        l: Obj,
        z: u8,
        i: i32,
        j: i64,
    }

    fn int(i: i32) -> Value {
        Value { i }
    }
    fn obj(l: Obj) -> Value {
        Value { l }
    }

    unsafe extern "C" {
        fn dlopen(name: *const c_char, flags: i32) -> *mut c_void;
        fn dlsym(library: *mut c_void, name: *const c_char) -> *mut c_void;
    }
    #[link(name = "log")]
    unsafe extern "C" {
        fn __android_log_write(priority: i32, tag: *const c_char, text: *const c_char) -> i32;
    }

    /// A finding, for the host to read in the log.
    fn note(text: &str) {
        let text = CString::new(text).unwrap_or_default();
        unsafe { __android_log_write(4, c"taut-check".as_ptr(), text.as_ptr()) };
    }

    /// The main thread's JNI interface, found through the VM (`JNI_GetCreatedJavaVMs`).
    #[derive(Clone, Copy)]
    struct Jni(*mut *const *const c_void);

    // JNI's table, as numbered in `jni.h`.
    const FIND_CLASS: usize = 6;
    const EXCEPTION_DESCRIBE: usize = 16;
    const EXCEPTION_CLEAR: usize = 17;
    const NEW_OBJECT_A: usize = 30;
    const GET_OBJECT_CLASS: usize = 31;
    const IS_INSTANCE_OF: usize = 32;
    const GET_METHOD_ID: usize = 33;
    const CALL_OBJECT_METHOD_A: usize = 36;
    const GET_FIELD_ID: usize = 94;
    const GET_INT_FIELD: usize = 100;
    const GET_FLOAT_FIELD: usize = 102;
    const GET_STATIC_METHOD_ID: usize = 113;
    const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
    const NEW_STRING_UTF: usize = 167;
    const GET_STRING_UTF_CHARS: usize = 169;
    const RELEASE_STRING_UTF_CHARS: usize = 170;
    const GET_ARRAY_LENGTH: usize = 171;
    const GET_OBJECT_ARRAY_ELEMENT: usize = 173;
    const EXCEPTION_CHECK: usize = 228;
    /// The return types, in the order of the `Call<Type>MethodA` slots (three apart).
    const TYPES: &[u8] = b"LZBCSIJFDV";

    impl Jni {
        fn get() -> Option<Jni> {
            unsafe {
                let helper = dlopen(c"libnativehelper.so".as_ptr(), 2);
                let found = dlsym(helper, c"JNI_GetCreatedJavaVMs".as_ptr());
                if found.is_null() {
                    return None;
                }
                let created: extern "C" fn(*mut *mut c_void, i32, *mut i32) -> i32 = std::mem::transmute(found);
                let (mut vm, mut count) = (std::ptr::null_mut::<c_void>(), 0);
                created(&mut vm, 1, &mut count);
                let table = *(vm as *const *const *const c_void);
                let get_env: extern "C" fn(*mut c_void, *mut *mut c_void, i32) -> i32 = std::mem::transmute(*table.add(6));
                let mut env = std::ptr::null_mut();
                (count == 1 && get_env(vm, &mut env, 0x0001_0006) == 0).then_some(Jni(env.cast()))
            }
        }
        unsafe fn slot<F: Copy>(self, n: usize) -> F {
            unsafe { std::mem::transmute_copy(&*(*self.0).add(n)) }
        }
        /// Whether Java threw (then it is logged and cleared).
        fn threw(self) -> bool {
            unsafe {
                let thrown = self.slot::<extern "C" fn(Jni) -> u8>(EXCEPTION_CHECK)(self) != 0;
                if thrown {
                    self.slot::<extern "C" fn(Jni)>(EXCEPTION_DESCRIBE)(self);
                    self.slot::<extern "C" fn(Jni)>(EXCEPTION_CLEAR)(self);
                }
                thrown
            }
        }
        fn class(self, name: &CStr) -> Obj {
            let class = unsafe { self.slot::<extern "C" fn(Jni, *const c_char) -> Obj>(FIND_CLASS)(self, name.as_ptr()) };
            if self.threw() { std::ptr::null_mut() } else { class }
        }
        /// Calls a method, through the slot its return type has (`first` is the object one's).
        fn invoke(self, first: usize, on: Obj, method: Obj, signature: &CStr, args: &[Value]) -> Value {
            let bytes = signature.to_bytes();
            let returns = bytes.iter().rposition(|byte| *byte == b')').and_then(|at| bytes.get(at + 1)).copied().unwrap_or(b'V');
            let returns = if returns == b'[' { b'L' } else { returns };
            let slot = first + 3 * TYPES.iter().position(|kind| *kind == returns).unwrap_or(9);
            let value = unsafe {
                match returns {
                    b'V' => (self.slot::<extern "C" fn(Jni, Obj, Obj, *const Value)>(slot)(self, on, method, args.as_ptr()), Value { j: 0 }).1,
                    b'Z' => Value { z: self.slot::<extern "C" fn(Jni, Obj, Obj, *const Value) -> u8>(slot)(self, on, method, args.as_ptr()) },
                    b'I' => int(self.slot::<extern "C" fn(Jni, Obj, Obj, *const Value) -> i32>(slot)(self, on, method, args.as_ptr())),
                    _ => obj(self.slot::<extern "C" fn(Jni, Obj, Obj, *const Value) -> Obj>(slot)(self, on, method, args.as_ptr())),
                }
            };
            if self.threw() { Value { j: 0 } } else { value }
        }
        fn call(self, on: Obj, name: &CStr, signature: &CStr, args: &[Value]) -> Value {
            if on.is_null() {
                return Value { j: 0 };
            }
            let method = unsafe {
                let class = self.slot::<extern "C" fn(Jni, Obj) -> Obj>(GET_OBJECT_CLASS)(self, on);
                self.slot::<extern "C" fn(Jni, Obj, *const c_char, *const c_char) -> Obj>(GET_METHOD_ID)(self, class, name.as_ptr(), signature.as_ptr())
            };
            if self.threw() { Value { j: 0 } } else { self.invoke(CALL_OBJECT_METHOD_A, on, method, signature, args) }
        }
        fn call_static(self, class: &CStr, name: &CStr, signature: &CStr, args: &[Value]) -> Value {
            let class = self.class(class);
            let method = unsafe { self.slot::<extern "C" fn(Jni, Obj, *const c_char, *const c_char) -> Obj>(GET_STATIC_METHOD_ID)(self, class, name.as_ptr(), signature.as_ptr()) };
            if self.threw() { Value { j: 0 } } else { self.invoke(CALL_STATIC_OBJECT_METHOD_A, class, method, signature, args) }
        }
        fn object(self, on: Obj, name: &CStr, signature: &CStr, args: &[Value]) -> Obj {
            unsafe { self.call(on, name, signature, args).l }
        }
        fn number(self, on: Obj, name: &CStr) -> i32 {
            unsafe { self.call(on, name, c"()I", &[]).i }
        }
        fn yes(self, on: Obj, name: &CStr) -> bool {
            unsafe { self.call(on, name, c"()Z", &[]).z != 0 }
        }
        fn made(self, class: &CStr, signature: &CStr, args: &[Value]) -> Obj {
            let class = self.class(class);
            let made = unsafe {
                let constructor = self.slot::<extern "C" fn(Jni, Obj, *const c_char, *const c_char) -> Obj>(GET_METHOD_ID)(self, class, c"<init>".as_ptr(), signature.as_ptr());
                self.slot::<extern "C" fn(Jni, Obj, Obj, *const Value) -> Obj>(NEW_OBJECT_A)(self, class, constructor, args.as_ptr())
            };
            if self.threw() { std::ptr::null_mut() } else { made }
        }
        fn string(self, text: &str) -> Obj {
            let text = CString::new(text).unwrap_or_default();
            unsafe { self.slot::<extern "C" fn(Jni, *const c_char) -> Obj>(NEW_STRING_UTF)(self, text.as_ptr()) }
        }
        /// The text of anything (`toString`); empty for null.
        fn text(self, of: Obj) -> String {
            let string = self.object(of, c"toString", c"()Ljava/lang/String;", &[]);
            if string.is_null() {
                return String::new();
            }
            unsafe {
                let chars = self.slot::<extern "C" fn(Jni, Obj, *mut u8) -> *const c_char>(GET_STRING_UTF_CHARS)(self, string, std::ptr::null_mut());
                let text = CStr::from_ptr(chars).to_string_lossy().into_owned();
                self.slot::<extern "C" fn(Jni, Obj, *const c_char)>(RELEASE_STRING_UTF_CHARS)(self, string, chars);
                text
            }
        }
        fn field<T>(self, on: Obj, name: &CStr, kind: &CStr, slot: usize) -> T {
            unsafe {
                let class = self.slot::<extern "C" fn(Jni, Obj) -> Obj>(GET_OBJECT_CLASS)(self, on);
                let field = self.slot::<extern "C" fn(Jni, Obj, *const c_char, *const c_char) -> Obj>(GET_FIELD_ID)(self, class, name.as_ptr(), kind.as_ptr());
                self.slot::<extern "C" fn(Jni, Obj, Obj) -> T>(slot)(self, on, field)
            }
        }
        fn is(self, on: Obj, class: &CStr) -> bool {
            let class = self.class(class);
            unsafe { self.slot::<extern "C" fn(Jni, Obj, Obj) -> u8>(IS_INSTANCE_OF)(self, on, class) != 0 }
        }
        /// The objects of a Java array, or of a `java.util.List`.
        fn items(self, of: Obj) -> Vec<Obj> {
            if of.is_null() {
                return Vec::new();
            }
            if self.is(of, c"java/util/List") {
                return (0..self.number(of, c"size")).map(|at| self.object(of, c"get", c"(I)Ljava/lang/Object;", &[int(at)])).collect();
            }
            unsafe {
                let long = self.slot::<extern "C" fn(Jni, Obj) -> i32>(GET_ARRAY_LENGTH)(self, of);
                (0..long).map(|at| self.slot::<extern "C" fn(Jni, Obj, i32) -> Obj>(GET_OBJECT_ARRAY_ELEMENT)(self, of, at)).collect()
            }
        }
    }

    /// `AccessibilityNodeInfo`'s actions and arguments, and the letter places' key.
    const NEXT_AT_GRANULARITY: i32 = 256;
    const SCROLL_FORWARD: i32 = 0x1000;
    const SCROLL_TO_POSITION: i32 = 0x0102_0037;
    const LETTER_PLACES: &str = "android.view.accessibility.extra.DATA_TEXT_CHARACTER_LOCATION_KEY";

    /// The library's view and its node provider, as the system reads them.
    struct Reader {
        jni: Jni,
        provider: Obj,
    }

    impl Reader {
        /// The library's view (a `taut.Host` in one of the app's windows), and its provider.
        fn find() -> Option<Reader> {
            let jni = Jni::get()?;
            let roots = jni.call_static(c"android/view/inspector/WindowInspector", c"getGlobalWindowViews", c"()Ljava/util/List;", &[]);
            let view = jni.items(unsafe { roots.l }).into_iter().find_map(|root| host_in(jni, root))?;
            let provider = jni.object(view, c"getAccessibilityNodeProvider", c"()Landroid/view/accessibility/AccessibilityNodeProvider;", &[]);
            (!provider.is_null()).then_some(Reader { jni, provider })
        }
        /// A virtual view's node (null for none).
        fn node(&self, id: i32) -> Obj {
            self.jni.object(self.provider, c"createAccessibilityNodeInfo", c"(I)Landroid/view/accessibility/AccessibilityNodeInfo;", &[int(id)])
        }
        /// The first node whose class and text are these: its virtual view id and node.
        fn find_node(&self, class: &str, text: &str) -> Option<(i32, Obj)> {
            let (jni, mut misses) = (self.jni, 0);
            for id in 1.. {
                let node = self.node(id);
                misses = if node.is_null() { misses + 1 } else { 0 };
                if misses > 300 {
                    return None;
                }
                let named = |name: &CStr| jni.text(jni.object(node, name, c"()Ljava/lang/CharSequence;", &[]));
                if !node.is_null() && (class.is_empty() || named(c"getClassName") == class) && named(c"getText") == text {
                    return Some((id, node));
                }
            }
            None
        }
        fn perform(&self, id: i32, action: i32, arguments: Obj) -> bool {
            unsafe { self.jni.call(self.provider, c"performAction", c"(IILandroid/os/Bundle;)Z", &[int(id), int(action), obj(arguments)]).z != 0 }
        }
        /// A bundle of numbers (and yes/no) by name.
        fn bundle(&self, numbers: &[(&str, i32)]) -> Obj {
            let bundle = self.jni.made(c"android/os/Bundle", c"()V", &[]);
            for (name, value) in numbers {
                self.jni.call(bundle, c"putInt", c"(Ljava/lang/String;I)V", &[obj(self.jni.string(name)), int(*value)]);
            }
            bundle
        }
        /// The place on the screen of each of a node's first `count` letters (left, top, right,
        /// bottom), as a reader asks for them; none for one that does not show.
        fn letter_places(&self, id: i32, count: i32) -> Vec<Option<[f32; 4]>> {
            let (jni, node, key) = (self.jni, self.node(id), self.jni.string(LETTER_PLACES));
            let asked = self
                .bundle(&[("android.view.accessibility.extra.DATA_TEXT_CHARACTER_LOCATION_ARG_START_INDEX", 0), ("android.view.accessibility.extra.DATA_TEXT_CHARACTER_LOCATION_ARG_LENGTH", count)]);
            let signature = c"(ILandroid/view/accessibility/AccessibilityNodeInfo;Ljava/lang/String;Landroid/os/Bundle;)V";
            jni.call(self.provider, c"addExtraDataToAccessibilityNodeInfo", signature, &[int(id), obj(node), obj(key), obj(asked)]);
            let extras = jni.object(node, c"getExtras", c"()Landroid/os/Bundle;", &[]);
            let places = jni.object(extras, c"getParcelableArray", c"(Ljava/lang/String;)[Landroid/os/Parcelable;", &[obj(key)]);
            let edges = |place: Obj| [c"left", c"top", c"right", c"bottom"].map(|edge| jni.field::<f32>(place, edge, c"F", GET_FLOAT_FIELD));
            jni.items(places).into_iter().map(|place| (!place.is_null()).then(|| edges(place))).collect()
        }
        /// The ids of a node's actions.
        fn actions(&self, node: Obj) -> Vec<i32> {
            let list = self.jni.object(node, c"getActionList", c"()Ljava/util/List;", &[]);
            self.jni.items(list).into_iter().map(|action| self.jni.number(action, c"getId")).collect()
        }
    }

    /// The library's view, in a view and those within it.
    fn host_in(jni: Jni, view: Obj) -> Option<Obj> {
        let class = jni.object(view, c"getClass", c"()Ljava/lang/Class;", &[]);
        if jni.text(jni.object(class, c"getName", c"()Ljava/lang/String;", &[])) == "taut.Host" {
            return Some(view);
        }
        if !jni.is(view, c"android/view/ViewGroup") {
            return None;
        }
        (0..jni.number(view, c"getChildCount")).find_map(|at| host_in(jni, jni.object(view, c"getChildAt", c"(I)Landroid/view/View;", &[int(at)])))
    }

    /// The page: the paragraph, a button, the text box and the table.
    struct Page {
        notes: String,
        table: Table,
        script: Script,
    }

    pub fn main() {
        let mut page = Page { notes: NOTES.into(), table: Table::default(), script: Script::default() };
        taut::run(Window { title: "taut check", ..Default::default() }, move |ui| {
            describe(&mut page, ui);
            page.script.step(ui);
        });
    }

    fn describe(page: &mut Page, ui: &mut Ui) {
        ui.column().pad(16.0).gap(12.0).grow().open();
        ui.paragraph(PARAGRAPH);
        ui.button("Press");
        ui.text_box(&mut page.notes, "Notes");
        let columns = [taut::Column::new("Name").px(160.0), taut::Column::new("Size").px(80.0)];
        ui.table(&mut page.table, &columns);
        for row in ui.rows(500, 24.0) {
            ui.selectable_row(row, false);
            ui.cell(&format!("row {row}"));
            ui.cell("1 KB");
            ui.close();
        }
        ui.close();
        ui.close();
        ui.close();
        // (A turn a tenth of a second, for the checks to go on with nothing happening.)
        ui.wake_after(Duration::from_millis(100));
    }

    /// The checks, a step at a time: each acts, then waits (a few seconds at most) for what it
    /// should bring.
    #[derive(Default)]
    struct Script {
        step: usize,
        since: Option<Instant>,
        acted: bool,
    }

    const STEPS: [&str; 8] = [
        "the provider is found",
        "text is read by line as wrapped",
        "each letter's place is given",
        "a text box is read by word, its cursor moving",
        "a table is a collection of all its rows",
        "a reader scrolls the table a page on",
        "a reader scrolls the table to a row",
        "",
    ];

    impl Script {
        fn step(&mut self, _: &mut Ui) {
            let since = *self.since.get_or_insert_with(Instant::now);
            let Some(name) = STEPS.get(self.step).copied().filter(|name| !name.is_empty()) else { return };
            if since.elapsed() < Duration::from_millis(500) {
                return;
            }
            let Some(reader) = Reader::find() else {
                if since.elapsed() > Duration::from_secs(8) {
                    self.next(name, Err("no provider".into()));
                }
                return;
            };
            if !self.acted {
                self.acted = true;
                act(&reader, name);
                return;
            }
            match check(&reader, name) {
                Err(_) if since.elapsed() < Duration::from_secs(8) => {}
                found => self.next(name, found),
            }
        }
        fn next(&mut self, name: &str, found: Result<(), String>) {
            note(&match found {
                Ok(()) => format!("ok {name}"),
                Err(why) => format!("FAILED {name}: {why}"),
            });
            (self.step, self.since, self.acted) = (self.step + 1, Some(Instant::now()), false);
            if STEPS[self.step].is_empty() {
                note("done");
            }
        }
    }

    fn act(reader: &Reader, name: &str) {
        let table = || reader.find_node("android.widget.GridView", "").map_or(0, |(id, _)| id);
        match name {
            "a text box is read by word, its cursor moving" => {
                let Some((id, _)) = reader.find_node("android.widget.EditText", NOTES) else { return };
                reader.perform(id, NEXT_AT_GRANULARITY, reader.bundle(&[("ACTION_ARGUMENT_MOVEMENT_GRANULARITY_INT", 2)]));
            }
            "a reader scrolls the table a page on" => _ = reader.perform(table(), SCROLL_FORWARD, std::ptr::null_mut()),
            "a reader scrolls the table to a row" => _ = reader.perform(table(), SCROLL_TO_POSITION, reader.bundle(&[("android.view.accessibility.action.ARGUMENT_ROW_INT", 401)])),
            _ => {}
        }
    }

    fn check(reader: &Reader, name: &str) -> Result<(), String> {
        let jni = reader.jni;
        match name {
            "the provider is found" => reader.find_node("", PARAGRAPH).map(drop).ok_or_else(|| "no paragraph told".into()),
            "text is read by line as wrapped" => lines(reader),
            "each letter's place is given" => letters(reader),
            "a text box is read by word, its cursor moving" => {
                let (_, node) = reader.find_node("android.widget.EditText", NOTES).ok_or("no text box")?;
                let selection = [c"getTextSelectionStart", c"getTextSelectionEnd"].map(|end| jni.number(node, end));
                if selection == [5, 5] { Ok(()) } else { Err(format!("its selection is {selection:?}")) }
            }
            "a table is a collection of all its rows" => table(reader),
            "a reader scrolls the table a page on" => {
                let rows = rows_told(reader);
                if rows.first().is_some_and(|first| *first > 0) { Ok(()) } else { Err(format!("rows told {rows:?}")) }
            }
            "a reader scrolls the table to a row" => {
                let rows = rows_told(reader);
                if rows.contains(&400) { Ok(()) } else { Err(format!("rows told {rows:?}")) }
            }
            _ => Ok(()),
        }
    }

    /// The rows of the table told (those in view), by their first cells' text.
    fn rows_told(reader: &Reader) -> Vec<u32> {
        let mut rows = Vec::new();
        let mut misses = 0;
        for id in 1.. {
            let node = reader.node(id);
            misses = if node.is_null() { misses + 1 } else { 0 };
            if misses > 300 {
                break;
            }
            let text = reader.jni.text(reader.jni.object(node, c"getText", c"()Ljava/lang/CharSequence;", &[]));
            rows.extend(text.strip_prefix("row ").and_then(|row| row.parse::<u32>().ok()));
        }
        rows.sort_unstable();
        rows
    }

    /// The paragraph by line: a reader moving a line at a time stops as many times as its
    /// letters lie on lines (by their places).
    fn lines(reader: &Reader) -> Result<(), String> {
        let (id, node) = reader.find_node("", PARAGRAPH).ok_or("no paragraph")?;
        let units = reader.jni.number(node, c"getMovementGranularities");
        let places = reader.letter_places(id, PARAGRAPH.len() as i32);
        let mut tops: Vec<i32> = places.iter().flatten().map(|place| place[1].round() as i32).collect();
        tops.dedup();
        let line = reader.bundle(&[("ACTION_ARGUMENT_MOVEMENT_GRANULARITY_INT", 4)]);
        let moves = (0..20).take_while(|_| reader.perform(id, NEXT_AT_GRANULARITY, line)).count();
        if units == 15 && tops.len() >= 3 && moves == tops.len() { Ok(()) } else { Err(format!("units {units}, {} lines by place, {moves} moves", tops.len())) }
    }

    /// The paragraph's letters: each shows, inside the paragraph, left to right along a line.
    fn letters(reader: &Reader) -> Result<(), String> {
        let (id, node) = reader.find_node("", PARAGRAPH).ok_or("no paragraph")?;
        let offered = reader.jni.items(reader.jni.object(node, c"getAvailableExtraData", c"()Ljava/util/List;", &[])).into_iter().map(|key| reader.jni.text(key)).collect::<Vec<_>>();
        let places: Vec<[f32; 4]> = reader.letter_places(id, PARAGRAPH.len() as i32).into_iter().flatten().collect();
        let bounds = reader.jni.made(c"android/graphics/Rect", c"()V", &[]);
        reader.jni.call(node, c"getBoundsInScreen", c"(Landroid/graphics/Rect;)V", &[obj(bounds)]);
        let [left, top, right, bottom] = [c"left", c"top", c"right", c"bottom"].map(|edge| reader.jni.field::<i32>(bounds, edge, c"I", GET_INT_FIELD) as f32);
        let inside = places.iter().all(|place| place[0] >= left - 1.0 && place[1] >= top - 1.0 && place[2] <= right + 1.0 && place[3] <= bottom + 1.0);
        let along = places.windows(2).all(|pair| pair[1][0] >= pair[0][0] || pair[1][1] > pair[0][1]);
        let ok = offered == [LETTER_PLACES] && places.len() == PARAGRAPH.len() && inside && along;
        if ok { Ok(()) } else { Err(format!("offered {offered:?}; {} places of {}, inside {inside}, in order {along}", places.len(), PARAGRAPH.len())) }
    }

    /// The table: all its rows (after its headings' row) and columns, scrolled by a reader; a
    /// cell's row and column, a heading.
    fn table(reader: &Reader) -> Result<(), String> {
        let jni = reader.jni;
        let (_, table) = reader.find_node("android.widget.GridView", "").ok_or("no table")?;
        let whole = jni.object(table, c"getCollectionInfo", c"()Landroid/view/accessibility/AccessibilityNodeInfo$CollectionInfo;", &[]);
        let (rows, columns) = (jni.number(whole, c"getRowCount"), jni.number(whole, c"getColumnCount"));
        let actions = reader.actions(table);
        let (_, cell) = reader.find_node("", "row 0").ok_or("no cell of row 0")?;
        let item = jni.object(cell, c"getCollectionItemInfo", c"()Landroid/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo;", &[]);
        let place = (jni.number(item, c"getRowIndex"), jni.number(item, c"getColumnIndex"));
        let (_, heading) = reader.find_node("", "Size").ok_or("no heading")?;
        let head = jni.object(heading, c"getCollectionItemInfo", c"()Landroid/view/accessibility/AccessibilityNodeInfo$CollectionItemInfo;", &[]);
        let headed = (jni.yes(head, c"isHeading"), jni.number(head, c"getColumnIndex"));
        let scrolls = jni.yes(table, c"isScrollable") && actions.contains(&SCROLL_FORWARD) && actions.contains(&SCROLL_TO_POSITION);
        let ok = (rows, columns) == (501, 2) && scrolls && place == (1, 0) && headed == (true, 1);
        if ok { Ok(()) } else { Err(format!("{rows} rows, {columns} columns, actions {actions:?}; row 0's cell at {place:?}; the heading {headed:?}")) }
    }
}

#[cfg(not(target_os = "android"))]
mod host {
    use super::NOTES;
    use super::on_android;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const PACKAGE: &str = "taut.example.android";

    /// How long the emulator may stay out of reach before the checks give up on it.
    const OUT_OF_REACH: Duration = Duration::from_secs(120);

    /// `adb`: what it printed (sent again whenever it never reached the emulator).
    fn adb(args: &[&str]) -> String {
        on_android::said(&on_android::program(), args).0
    }

    fn shell(command: &str) -> String {
        adb(&["shell", command])
    }

    /// One node of a `uiautomator dump`: class, text, focused, scrollable, bounds.
    #[derive(Clone, Debug, Default)]
    struct Node {
        class: String,
        text: String,
        focused: bool,
        scrollable: bool,
        bounds: [i32; 4],
    }

    /// An attribute of a dumped node.
    fn attribute(tag: &str, name: &str) -> String {
        let value = tag.split(&format!(" {name}=\"")).nth(1).and_then(|rest| rest.split('"').next()).unwrap_or_default();
        value.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
    }

    /// The window as another program reads it; the dump kept in `out`. A dump that failed is
    /// asked again: on a busy emulator `uiautomator` itself can fail to connect to the system.
    fn dump(out: &Path, name: &str) -> Vec<Node> {
        let since = Instant::now();
        while !shell("rm -f /sdcard/taut-ui.xml; uiautomator dump /sdcard/taut-ui.xml 2>&1").contains("dumped to") {
            assert!(since.elapsed() < OUT_OF_REACH, "uiautomator dumped nothing");
            std::thread::sleep(Duration::from_secs(1));
        }
        let xml = String::from_utf8_lossy(&on_android::read(&on_android::program(), "/sdcard/taut-ui.xml")).into_owned();
        let _ = std::fs::write(out.join(format!("{name}.xml")), &xml);
        let node = |tag: &str| {
            let corners: Vec<i32> = attribute(tag, "bounds").split(|c: char| !c.is_ascii_digit()).filter_map(|part| part.parse().ok()).collect();
            let flag = |name: &str| attribute(tag, name) == "true";
            Node { class: attribute(tag, "class"), text: attribute(tag, "text"), focused: flag("focused"), scrollable: flag("scrollable"), bounds: corners.try_into().unwrap_or_default() }
        };
        xml.split("<node ").skip(1).map(node).collect()
    }

    /// The part told focused, other than the library's own view.
    fn focused(nodes: &[Node]) -> Option<(String, String)> {
        nodes.iter().find(|node| node.focused && node.class != "android.view.View").map(|node| (node.class.clone(), node.text.clone()))
    }

    /// The app's findings, once it logged "done" (or the wait is over).
    fn findings(wait: Duration) -> String {
        let since = Instant::now();
        loop {
            let log = adb(&["logcat", "-d", "-s", "taut-check:I"]);
            if log.contains("done") || since.elapsed() > wait {
                return log;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    pub fn main() {
        let Some(apk) = std::env::var_os("TAUT_ANDROID_APK") else { return println!("The Android checks run with TAUT_ANDROID_APK set (they need an emulator).") };
        let out = std::env::var_os("TAUT_PICTURES").map_or_else(|| std::env::temp_dir().join("taut-android-readers"), PathBuf::from);
        std::fs::create_dir_all(&out).unwrap();
        let mut failed: Vec<String> = Vec::new();
        let (said, errors) = on_android::said(&on_android::program(), &["install", "-r", &apk.to_string_lossy()]);
        assert!(said.contains("Success"), "the app was not installed: {said}{errors}");
        adb(&["logcat", "-c"]);
        shell("settings put global hide_error_dialogs 1; settings put system accelerometer_rotation 0; settings put system user_rotation 0");
        shell(&format!("am start -W -n {PACKAGE}/taut.Activity"));
        // What the app found reading itself.
        let found = findings(Duration::from_secs(120));
        println!("the app's findings:\n{found}");
        if !found.contains("done") {
            failed.push("the app did not finish its checks".into());
        }
        failed.extend(found.lines().filter(|line| line.contains("FAILED")).map(str::to_owned));
        // The table as another program reads it: a scrollable grid.
        let nodes = dump(&out, "read");
        if !nodes.iter().any(|node| node.class == "android.widget.GridView" && node.scrollable) {
            failed.push("no scrollable table told to uiautomator".into());
        }
        // A hardware keyboard's Tab moves focus from the text box to the next part, Shift+Tab back.
        let Some(notes) = nodes.iter().find(|node| node.class == "android.widget.EditText" && node.text == NOTES).cloned() else {
            failed.push("no text box told to uiautomator".into());
            return finish(&out, failed);
        };
        shell(&format!("input tap {} {}", (notes.bounds[0] + notes.bounds[2]) / 2, (notes.bounds[1] + notes.bounds[3]) / 2));
        std::thread::sleep(Duration::from_secs(2));
        let at_box = focused(&dump(&out, "tapped"));
        shell("input keyevent KEYCODE_TAB");
        std::thread::sleep(Duration::from_secs(2));
        let after_tab = focused(&dump(&out, "tabbed"));
        shell("input keycombination KEYCODE_SHIFT_LEFT KEYCODE_TAB");
        std::thread::sleep(Duration::from_secs(2));
        let back = focused(&dump(&out, "tabbed-back"));
        println!("focus: tapped {at_box:?}, Tab {after_tab:?}, Shift+Tab {back:?}");
        let on_box = |focus: &Option<(String, String)>| focus.as_ref().is_some_and(|(class, _)| class == "android.widget.EditText");
        if !on_box(&at_box) || after_tab.is_none() || on_box(&after_tab) || !on_box(&back) {
            failed.push(format!("focus by keys: tapped {at_box:?}, Tab {after_tab:?}, Shift+Tab {back:?}"));
        }
        finish(&out, failed);
    }

    /// Keeps the log, and fails with what failed.
    fn finish(out: &Path, failed: Vec<String>) {
        let log = adb(&["logcat", "-d"]);
        let _ = std::fs::write(out.join("logcat.txt"), &log);
        // (The crash buffer names the process: `uiautomator` stopping on its own is no crash of the app.)
        if adb(&["logcat", "-d", "-b", "crash"]).contains(PACKAGE) {
            eprintln!("the app crashed: see logcat.txt");
            std::process::exit(1);
        }
        if !failed.is_empty() {
            eprintln!("FAILED:\n{}", failed.join("\n"));
            std::process::exit(1);
        }
        println!("all Android reader checks passed");
    }
}
