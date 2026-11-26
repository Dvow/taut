//! Typing on Android through an input method (the soft keyboard): the connection it types
//! through (`taut.Typing`, a `BaseInputConnection`, which keeps the text it typed so the input
//! method sees what it expects), and what it composes. Android leaves showing that to the app:
//! it is drawn at the text cursor, over the text box, on the theme's raised colour and
//! underlined, until the input method commits it (then it is typed into the box) or drops it.
//! The first text box hands all this to the window (`hand_over`): a program with none carries
//! none of it.
use crate::canvas::Canvas;
use crate::geom::rect;
use crate::ui::Ui;
#[cfg(target_os = "android")]
pub(crate) use connection::*;

/// Units between the text and the panel's sides.
const PAD: f32 = 2.0;

/// The composing text as drawn: where it goes in the window, in pixels (left, top, right,
/// bottom), and its pixels, rows of its width, `0xAARRGGBB`.
#[derive(PartialEq)]
pub(crate) struct Panel {
    pub place: [i32; 4],
    pub pixels: Vec<u32>,
}

/// The panel for `text` at the text cursor, `insets` pixels in from the window's top left
/// (left, top); none without a text cursor or text.
pub(crate) fn panel(ui: &mut Ui, text: &str, insets: [i32; 2]) -> Option<Panel> {
    let spot = ui.ime_area().filter(|_| !text.is_empty())?;
    let (scale, font, back, ink) = (ui.scale(), ui.theme.font, ui.theme.raised, ui.theme.text);
    let wide = ui.text.measure(text, font, scale, None) + PAD * 2.0;
    let [width, height] = [(wide * scale).ceil() as usize, (spot.h * scale).ceil().max(1.0) as usize];
    let mut pixels = vec![back.0; width * height];
    let mut canvas = Canvas::new(&mut pixels, width, height, (0, 0), scale);
    let line = ui.text.metrics(font, scale);
    let top = ((spot.h - line.ascent - line.descent) / 2.0).max(0.0);
    ui.text.draw(&mut canvas, text, PAD, top, font, ink);
    canvas.rect(rect(PAD, top + line.ascent + 1.0, wide - PAD * 2.0, 1.0), ink);
    let [left, top] = [(spot.x * scale) as i32 + insets[0], (spot.y * scale) as i32 + insets[1]];
    Some(Panel { place: [left, top, left + width as i32, top + height as i32], pixels })
}

/// The connection and what it composes, on Android.
#[cfg(target_os = "android")]
mod connection {
    use super::Panel;
    use crate::android::{Heard, activity, classes, heard, kick, typing_password, view};
    use crate::android_jni::{self as jni, Env, Native, Obj, Value};
    use crate::android_sys::WindowBuffer;
    use crate::event::{Event, Key, Mods};
    use crate::text_edit::DeleteAround;
    use crate::ui::{Stroke, Ui};
    use std::cell::RefCell;
    use std::ffi::{CStr, c_int, c_void};

    /// The class the connection extends.
    const CONNECTION: &CStr = c"android/view/inputmethod/BaseInputConnection";

    /// What the window does with input methods, once a text box was described.
    #[derive(Clone, Copy)]
    pub(crate) struct InputMethod {
        /// A connection for an input method, typing into the view (null if none can be made).
        pub connect: fn(Env, Obj, Obj) -> Obj,
        /// The panel showing what is composed, `insets` pixels in: none while nothing is.
        pub panel: fn(&mut Ui, [i32; 2]) -> Option<Panel>,
        /// Puts the panel into the window's buffer.
        pub show: fn(&Panel, &mut [u32], &WindowBuffer),
        /// Drops what is composed (focus moved, the activity went).
        pub drop: fn(),
    }

    /// How the window treats input methods: nothing until a text box is described.
    pub(crate) static INPUT_METHOD: std::sync::OnceLock<InputMethod> = std::sync::OnceLock::new();

    thread_local! {
        /// What the input method is composing, not yet typed.
        static COMPOSING: RefCell<String> = const { RefCell::new(String::new()) };
    }

    /// Has the window take input methods' typing (from a text box being described): the
    /// connection's class gets its natives, once.
    pub(crate) fn hand_over() {
        if INPUT_METHOD.get().is_some() {
            return;
        }
        let (env, class) = (jni::env(), classes()[2]);
        let Some(env) = env.filter(|_| !class.is_null()) else { return };
        let natives = [
            Native::new(c"commitText", c"(Ljava/lang/CharSequence;I)Z", commit_text as *const c_void),
            Native::new(c"setComposingText", c"(Ljava/lang/CharSequence;I)Z", set_composing_text as *const c_void),
            Native::new(c"finishComposingText", c"()Z", finish_composing_text as *const c_void),
            Native::new(c"deleteSurroundingText", c"(II)Z", delete_surrounding_text as *const c_void),
            Native::new(c"deleteSurroundingTextInCodePoints", c"(II)Z", delete_surrounding_code_points as *const c_void),
            Native::new(c"performEditorAction", c"(I)Z", editor_action as *const c_void),
        ];
        if env.register(class, &natives) {
            INPUT_METHOD.get_or_init(|| InputMethod { connect, panel: composed_panel, show, drop: || COMPOSING.with_borrow_mut(String::clear) });
        }
    }

    /// An input method connects: plain text (a password's, with no suggestions), the keyboard
    /// never full screen, the cursor at the start of what it types; the connection takes it.
    fn connect(env: Env, view: Obj, info: Obj) -> Obj {
        // Text: a password (0x81), or auto-corrected (0x8001); no full-screen editor.
        env.set_int_field(info, c"inputType", if typing_password() { 0x81 } else { 0x8001 });
        env.set_int_field(info, c"imeOptions", 0x1200_0000);
        env.set_int_field(info, c"initialSelStart", 0);
        env.set_int_field(info, c"initialSelEnd", 0);
        COMPOSING.with_borrow_mut(String::clear);
        env.new_object(classes()[2], c"(Landroid/view/View;Z)V", &[Value::obj(view), Value::bool(true)])
    }

    /// The panel of what is composed now.
    fn composed_panel(ui: &mut Ui, insets: [i32; 2]) -> Option<Panel> {
        let text = COMPOSING.with_borrow(String::clone);
        super::panel(ui, &text, insets)
    }

    /// Puts the panel into the window's buffer, as far as it fits.
    fn show(panel: &Panel, rows: &mut [u32], buffer: &WindowBuffer) {
        let [left, top, right, bottom] = panel.place;
        let width = (right - left) as usize;
        for y in top.max(0)..bottom.min(buffer.height) {
            let from = &panel.pixels[(y - top) as usize * width..][..width];
            let row = &mut rows[(y * buffer.stride) as usize..][..buffer.width as usize];
            for x in left.max(0)..right.min(buffer.width) {
                row[x as usize] = crate::android::swap(from[(x - left) as usize]);
            }
        }
    }

    /// Tells the input method where the cursor and what it composes are in the text it typed,
    /// as the connection keeps it (the input method checks this to compose).
    fn tell_selection(env: Env, connection: Obj) {
        env.frame(|| {
            let text = env.call(connection, c"getEditable", c"()Landroid/text/Editable;", &[]).object();
            let selection = |name: &CStr| env.call_static(c"android/text/Selection", name, c"(Ljava/lang/CharSequence;)I", &[Value::obj(text)]).as_int();
            let composing = |name: &CStr| env.call_static(CONNECTION, name, c"(Landroid/text/Spannable;)I", &[Value::obj(text)]).as_int();
            let [start, end] = [c"getSelectionStart", c"getSelectionEnd"].map(selection);
            let [from, to] = [c"getComposingSpanStart", c"getComposingSpanEnd"].map(composing);
            let args = [Value::obj(view().0), Value::int(start), Value::int(end), Value::int(from), Value::int(to)];
            env.call(jni::service(env, activity(), "input_method"), c"updateSelection", c"(Landroid/view/View;IIII)V", &args);
        });
    }

    /// Types `text` into the focused box, ending what the input method was composing.
    fn type_text(text: String) {
        COMPOSING.with_borrow_mut(String::clear);
        if !text.is_empty() {
            heard(Heard::Event(Event::Text(text)));
        }
    }

    /// The input method committed text: it is typed, in place of what it was composing.
    extern "C" fn commit_text(env: Env, connection: Obj, text: Obj, cursor: c_int) -> u8 {
        env.call_super(connection, CONNECTION, c"commitText", c"(Ljava/lang/CharSequence;I)Z", &[Value::obj(text), Value::int(cursor)]);
        type_text(env.text(text).unwrap_or_default());
        kick();
        tell_selection(env, connection);
        1
    }

    /// The input method composes: shown, not yet typed (an empty text drops it).
    extern "C" fn set_composing_text(env: Env, connection: Obj, text: Obj, cursor: c_int) -> u8 {
        env.call_super(connection, CONNECTION, c"setComposingText", c"(Ljava/lang/CharSequence;I)Z", &[Value::obj(text), Value::int(cursor)]);
        let text = env.text(text).unwrap_or_default();
        COMPOSING.set(text);
        tell_selection(env, connection);
        kick();
        1
    }

    /// The input method is done composing: what it composed is typed as it is.
    extern "C" fn finish_composing_text(env: Env, connection: Obj) -> u8 {
        env.call_super(connection, CONNECTION, c"finishComposingText", c"()Z", &[]);
        type_text(COMPOSING.take());
        kick();
        tell_selection(env, connection);
        1
    }

    /// The input method deletes around the selection: so many UTF-16 units before and after it.
    extern "C" fn delete_surrounding_text(env: Env, connection: Obj, before: c_int, after: c_int) -> u8 {
        delete_around(env, connection, [before, after], false)
    }

    /// The same counted in code points.
    extern "C" fn delete_surrounding_code_points(env: Env, connection: Obj, before: c_int, after: c_int) -> u8 {
        delete_around(env, connection, [before, after], true)
    }

    /// Deletes `before` and `after` the focused box's selection, whole characters (its own text
    /// as the connection keeps it, too); nothing for a count below zero.
    fn delete_around(env: Env, connection: Obj, [before, after]: [c_int; 2], in_code_points: bool) -> u8 {
        let around = match (usize::try_from(before), usize::try_from(after)) {
            (Ok(before), Ok(after)) => DeleteAround { before, after, in_code_points },
            _ => return 0,
        };
        let method = if in_code_points { c"deleteSurroundingTextInCodePoints" } else { c"deleteSurroundingText" };
        env.call_super(connection, CONNECTION, method, c"(II)Z", &[Value::int(before), Value::int(after)]);
        heard(Heard::Other(Box::new(move |ui: &mut Ui| {
            ui.input.strokes.push(Stroke::DeleteAround(around));
            true
        })));
        tell_selection(env, connection);
        kick();
        1
    }

    /// The keyboard's action key (Done, Go): what was composed is typed, then Enter.
    extern "C" fn editor_action(_: Env, _: Obj, _: c_int) -> u8 {
        type_text(COMPOSING.take());
        heard(Heard::Event(Event::Key(Key::Enter, Mods::NONE)));
        kick();
        1
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Trial;

    #[test]
    fn what_is_composed_is_drawn_underlined_at_the_text_cursor() {
        let mut trial = Trial::new(String::from("Typed"), |text: &mut String, ui| {
            let edit = ui.text_box(text, "");
            if ui.focus() == 0 {
                ui.set_focus(edit.id);
            }
        });
        trial.run();
        trial.run();
        let (spot, scale) = (trial.ui.ime_area().expect("a text cursor"), trial.ui.scale());
        assert!(panel(&mut trial.ui, "", [0, 0]).is_none(), "nothing composed, nothing shown");
        let shown = panel(&mut trial.ui, "abc", [10, 20]).expect("the panel");
        let [left, top, right, bottom] = shown.place;
        assert_eq!([left, top], [(spot.x * scale) as i32 + 10, (spot.y * scale) as i32 + 20]);
        let (width, ink, back) = ((right - left) as usize, trial.ui.theme.text.0, trial.ui.theme.raised.0);
        assert_eq!(shown.pixels.len(), width * (bottom - top) as usize);
        // A row of the text's colour across under the letters: the underline.
        let rows: Vec<&[u32]> = shown.pixels.chunks(width).collect();
        let underline = rows.iter().position(|row| row[3..width - 3].iter().all(|pixel| *pixel == ink));
        assert!(underline.is_some(), "an underline");
        // Above it, the letters: some of the text's colour, on the panel's own.
        let letters = rows[..underline.unwrap()].iter().flat_map(|row| row.iter()).filter(|pixel| **pixel != back).count();
        assert!(letters > 10 && shown.pixels.contains(&back), "letters on the panel");
    }
}
