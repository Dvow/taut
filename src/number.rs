//! Number boxes: a box that shows a number, takes a new one from the keyboard, and steps by
//! keys and the wheel; with optional bounds and a unit.
//!
//! ```ignore
//! ui.number(&mut app.copies, 1);
//! ui.number_in(&mut app.weight, 0.0..=500.0, 0.5, "kg");
//! ```
//!
//! What is typed becomes the number as soon as it parses. Text that can never become a number,
//! or is out of bounds, is refused: the number stays, a note under the box says why while it is
//! typed into (screen readers hear it too), and when focus leaves, the box shows the number
//! again. Text that may still become one ("-", "1.") is waited for silently. The number is
//! written with the person's decimal mark ([`ui.locale`](Ui::locale)) and read back with it or
//! with a point.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Key, Mods};
use crate::id::Id;
use crate::layout::{Align, Flow, Length};
use crate::locale::Locale;
use crate::text_edit::Kind;
use crate::ui::{Anchor, Response, Ui};
use std::fmt::{Arguments, Display, Write};
use std::ops::RangeInclusive;
use std::str::FromStr;

/// The layer under a number box that says why the text is refused.
const REFUSED: Id = 0x5e5e_0d0d_5e5e_0d0d;
/// A number box's greatest width.
const WIDEST: f32 = 120.0;

/// The number types a `number` box or a `drag` can hold: every Rust number type.
pub trait Number: Copy + PartialEq + Display + FromStr {
    /// The number as an `f64`, to compute with.
    fn to_f64(self) -> f64;
    /// The nearest number of this type.
    fn from_f64(value: f64) -> Self;
}

macro_rules! numbers {
    ($nearest:ident: $($kind:ty)*) => {$(
        impl Number for $kind {
            fn to_f64(self) -> f64 {
                self as f64
            }
            fn from_f64(value: f64) -> Self {
                $nearest(value) as $kind
            }
        }
    )*};
}
numbers!(whole: i8 i16 i32 i64 isize u8 u16 u32 u64 usize);
numbers!(as_it_is: f32 f64);

fn whole(value: f64) -> f64 {
    value.round()
}
fn as_it_is(value: f64) -> f64 {
    value
}

/// Where `steps` steps take a number from `from`: the first to the next whole number of steps
/// from `base`, the rest a step each; rounded to the step's decimals as `written`, so three
/// steps of 0.1 give 0.3.
fn steps_on(from: f64, written: Arguments, step: f64, base: f64, steps: f64) -> f64 {
    let step = step.abs().max(f64::MIN_POSITIVE);
    let steps_in = (from - base) / step;
    let steps_in = if (steps_in - steps_in.round()).abs() < 1e-6 { steps_in.round() } else { steps_in };
    let on = base + (if steps > 0.0 { steps_in.floor() } else { steps_in.ceil() } + steps) * step;
    let places = written.to_string().split_once('.').map_or(0, |(_, places)| places.len().min(15));
    let scale = 10f64.powi(places as i32);
    (on * scale).round() / scale
}

/// Why typed text is refused: it is not a number and cannot become one; below the least; above
/// the most.
#[derive(Clone, Copy, PartialEq)]
enum Refused {
    NoNumber,
    TooSmall,
    TooLarge,
}

/// Number boxes, typed and stepped.
impl Ui {
    /// A box for a number of any type, typed like text. Up and Down step by `step` (ten steps
    /// with Shift, or Page Up and Page Down), as does the wheel over the focused box (ten steps
    /// sideways or with Shift). `changed` says when the number changed.
    pub fn number<T: Number>(&mut self, value: &mut T, step: T) -> Response {
        self.number_of(value, None, step, "")
    }
    /// The same for a number in `range`, with `unit` after it ("kg", "%"; "" for none). Steps
    /// stop at the ends; typed values beyond them are refused.
    pub fn number_in<T: Number>(&mut self, value: &mut T, range: RangeInclusive<T>, step: T, unit: &str) -> Response {
        let (least, most) = range.into_inner();
        self.number_of(value, Some([least, most]), step, unit)
    }

    /// The part of both that knows the number type: how it is written, read and stored.
    /// Everything else is done once in `f64`, so a program pays for it once however many types
    /// it uses.
    fn number_of<T: Number>(&mut self, value: &mut T, range: Option<[T; 2]>, step: T, unit: &str) -> Response {
        let was = *value;
        let read: fn(Locale, &str) -> Option<f64> = |locale, typed| locale.read_number::<T>(typed).map(T::to_f64);
        let bounds = range.map(|range| range.map(T::to_f64));
        let (mut state, to, refused) = self.number_box(format_args!("{value}"), format_args!("{step}"), [was.to_f64(), step.to_f64()], bounds, unit, read);
        if let Some(to) = to {
            *value = T::from_f64(to);
        }
        let [least, most] = bounds.unwrap_or([f64::MIN, f64::MAX]);
        self.retell_as_value(&state, Role::SpinButton, [value.to_f64(), least, most].map(|part| part.clamp(f32::MIN as f64, f32::MAX as f64) as f32));
        self.tell_step(step.to_f64() as f32);
        if let Some(refused) = refused {
            // The end it is beyond, written as its type writes it.
            let beyond = range.filter(|_| refused != Refused::NoNumber).map(|[least, most]| if refused == Refused::TooSmall { least } else { most });
            self.refuse(&state, refused, &beyond.map_or(String::new(), |beyond| beyond.to_string()), state.changed);
        }
        state.changed = *value != was;
        self.again |= state.changed;
        state
    }

    /// A number box with the number `written` as its type writes it and `[value, step]` as
    /// `f64`; `read` parses its type from the typed text. Gives back the response, the new
    /// number if it changed, and why the text is refused, if it is.
    fn number_box(
        &mut self,
        written: Arguments,
        step_written: Arguments,
        [value, step]: [f64; 2],
        range: Option<[f64; 2]>,
        unit: &str,
        read: fn(Locale, &str) -> Option<f64>,
    ) -> (Response, Option<f64>, Option<Refused>) {
        let (t, locale) = (self.theme, self.locale_in_use());
        let [least, most] = range.unwrap_or([f64::MIN, f64::MAX]);
        // The unit lies over the box's end, which keeps that much clear of the text.
        self.el().flow(Flow::Stack).w(Length::Grow(1.0)).max(WIDEST, f32::INFINITY).open();
        if !unit.is_empty() {
            let wide = self.text_width(unit, t.font);
            self.text_margin(0.0, wide + 6.0);
        }
        // Up and Down are taken before the text box sees them: there they would move within its
        // line.
        let keys = if self.focus == self.next_id() { self.number_keys() } else { 0.0 };
        let state = self.entry_box(written, locale.decimal_point, "");
        if !unit.is_empty() {
            self.el().grow().pad((0.0, 0.0, 8.0, 0.0)).text(unit, t.font, t.text_dim).text_align(Align::End).show();
        }
        self.close();
        let mut to = None;
        // Steps from keys and wheel: the first to the next whole number of steps from the least
        // (or zero), the rest a step each. Written out afresh on the next run, replacing what
        // was typed.
        let steps = keys + self.number_wheel(&state);
        if steps != 0.0 {
            let on = steps_on(value, step_written, step, range.map_or(0.0, |_| least), steps);
            (to, self.entry.0, self.again) = (Some(on.clamp(least, most)), 0, true);
        }
        // Where a screen reader set a value, it is set.
        if let Some((_, set)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            (to, self.entry.0) = (Some((set as f64).clamp(least, most)), 0);
        }
        // What is typed, and what of it is refused: a number just typed and in range is the
        // number now.
        if !(state.focused && self.entry.0 == state.id) {
            return (state, to, None);
        }
        let typed = self.entry.1.trim();
        let Some(number) = read(locale, typed) else {
            // What may still become a number is waited for: empty, a sign, "1." as a fraction
            // begins.
            let on_its_way = typed.is_empty() || read(locale, &format!("{typed}0")).is_some();
            return (state, to, (!on_its_way).then_some(Refused::NoNumber));
        };
        match number {
            _ if range.is_some() && number < least => (state, to, Some(Refused::TooSmall)),
            _ if range.is_some() && number > most => (state, to, Some(Refused::TooLarge)),
            _ => (state, if state.changed { Some(number) } else { to }, None),
        }
    }

    /// Steps the keys take a focused number box: ten for each large one.
    fn number_keys(&mut self) -> f64 {
        let mut key = |key: Key, mods: Mods| self.key(key, mods) as i32;
        let steps = key(Key::Up, Mods::NONE) - key(Key::Down, Mods::NONE);
        let large = key(Key::Up, Mods::SHIFT) - key(Key::Down, Mods::SHIFT) + key(Key::PageUp, Mods::NONE) - key(Key::PageDown, Mods::NONE);
        (steps + 10 * large) as f64
    }
    /// Steps the wheel takes the number box of `state`: ten for each large one.
    fn number_wheel(&mut self, state: &Response) -> f64 {
        let [wheel, large] = self.wheel_steps(state);
        (wheel + 10 * large) as f64
    }

    /// Says under box `of` why its text is refused (beyond an end: after the end written
    /// `beyond`), in a note over what is under it that takes no clicks; screen readers hear it
    /// when `just` typed.
    fn refuse(&mut self, of: &Response, refused: Refused, beyond: &str, just: bool) {
        let (t, words, locale) = (self.theme, self.number_words, self.locale_in_use());
        let mut said = String::from(words[refused as usize]);
        if !beyond.is_empty() {
            said.push(' ');
            locale.write_number(&mut said, beyond, true);
        }
        if just {
            self.announce(&said);
        }
        self.layer(of.id ^ REFUSED, Anchor::Under(of.rect, false), false, Color::NONE);
        self.el().pad((8.0, 4.0)).fill(t.raised).border(t.red, 1.0).radius(5.0).shadow(8.0).open();
        self.el().text(&said, t.small_font, t.text).show();
        self.close();
    }

    /// Wheel steps while the pointer is over `of` and it has focus: vertical, and sideways
    /// (Shift+wheel turns sideways); up or left is positive. A notch is a step, and less
    /// (touchpad) is a step too. Taken, it scrolls nothing.
    pub(crate) fn wheel_steps(&mut self, of: &Response) -> [i32; 2] {
        if !(of.hovered && of.focused) {
            return [0; 2];
        }
        let [across, down] = std::mem::take(&mut self.input.wheel);
        [down, across].map(|by| if by == 0.0 { 0 } else { -((by.abs() / 48.0).round().max(1.0) as i32) * by.signum() as i32 })
    }

    /// A text box for something the app does not hold as text (a number, a colour). Shows
    /// `value` (with `point` as decimal mark) until typed into, then the typed text while
    /// focused, and `hint` while empty, which is also its name for screen readers. `changed`
    /// says it was just typed into: its text is then in `self.entry`.
    pub(crate) fn entry_box(&mut self, value: Arguments, point: char, hint: &str) -> Response {
        let id = self.next_id();
        let (mine, fresh) = (self.focus == id, self.entry.0 != id);
        // The box being typed into has its own text; all others share one.
        let mut text = std::mem::take(if mine { &mut self.entry.1 } else { &mut self.scratch });
        if !mine || fresh {
            text.clear();
            let _ = text.write_fmt(value);
        }
        if (!mine || fresh) && point != '.' && text.contains('.') {
            text = text.replace('.', point.encode_utf8(&mut [0; 4]));
        }
        // Typing in it starts or stops.
        if mine == fresh {
            self.entry.0 = if mine { id } else { 0 };
        }
        let state = self.edit_box(&mut text, hint, Kind::Line, WIDEST);
        *(if mine { &mut self.entry.1 } else { &mut self.scratch }) = text;
        state
    }
}
