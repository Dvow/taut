//! Sliders that say more than where their knob is: one that stops at steps, with a mark under
//! each and its value beside it; and one that follows a curve, for what is felt as a ratio
//! (zoom, volume).
//!
//! ```ignore
//! ui.slider_marked(&mut app.stars, 0.0..=5.0, 1.0, "");    // 0 to 5, a mark at each
//! ui.slider_curved(&mut app.zoom, 25.0..=400.0, "%");      // half way is 100%
//! ```
//!
//! The curve: for a range starting above zero, each part of the way multiplies the value by the
//! same factor (25% to 400%: half way is 100%); for a range starting at zero or below, the
//! value grows with the square of the way (volume: half way is a quarter, and the quiet end is
//! set finely). Arrows move a step, or a twentieth of the way on the curve; Page Up and Page
//! Down ten steps or a quarter of the way; Home and End to the ends.
use crate::event::{Key, Mods};
use crate::layout::{Align, Length};
use crate::told::Role;
use crate::ui::{Response, Ui};
use std::fmt::Write;
use std::ops::RangeInclusive;

/// The knob's diameter.
const KNOB: f32 = 14.0;
/// The least spacing between marks: where steps lie closer, every nth step is marked.
const MARKS_APART: f32 = 6.0;

/// Where a value lies between `low` and `high`, from 0 to 1: evenly or by the curve.
fn along(value: f32, [low, high]: [f32; 2], curved: bool) -> f32 {
    let even = ((value - low) / (high - low).max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
    match (curved, low > 0.0) {
        (false, _) => even,
        (true, true) => ((value / low).ln() / (high / low).ln()).clamp(0.0, 1.0),
        (true, false) => even.sqrt(),
    }
}

/// The value at a point of the way from `low` to `high`.
fn value_at(along: f32, [low, high]: [f32; 2], curved: bool) -> f32 {
    let along = along.clamp(0.0, 1.0);
    match (curved, low > 0.0) {
        (false, _) => low + (high - low) * along,
        (true, true) => (low * (high / low).powf(along)).clamp(low, high),
        (true, false) => low + (high - low) * along * along,
    }
}

/// The decimals a value is written with: as many as the step has (0.25: two), or on the curve
/// enough for three significant digits.
fn decimals(step: f32, value: f32) -> usize {
    match step > 0.0 {
        true => step.to_string().split_once('.').map_or(0, |(_, places)| places.len().min(6)),
        false => (2.0 - value.abs().max(1e-6).log10().floor()).clamp(0.0, 3.0) as usize,
    }
}

/// Sliders with steps and marks, a curve, or their value beside them.
impl Ui {
    /// A slider that stops at every `step` of `range` from its start, with a mark under each
    /// (every nth where they would crowd) and the value beside it, followed by `unit` ("" for
    /// none). Arrows move a step, Page Up and Page Down ten. `changed` says when the person
    /// moved it.
    pub fn slider_marked(&mut self, value: &mut f32, range: RangeInclusive<f32>, step: f32, unit: &str) -> Response {
        let (low, high) = range.into_inner();
        self.slide(value, [low, high], step.abs().max((high - low).abs() / 10_000.0), unit)
    }
    /// A slider that follows a curve (see the module's top): for a zoom, a range starting above
    /// zero, where half way is the geometric mean of the ends; for a volume, one starting at
    /// zero. Its value is written beside it, followed by `unit`.
    pub fn slider_curved(&mut self, value: &mut f32, range: RangeInclusive<f32>, unit: &str) -> Response {
        let (low, high) = range.into_inner();
        self.slide(value, [low, high], 0.0, unit)
    }

    /// A slider as [`slider`](Ui::slider), with its value beside it followed by `unit` ("" for
    /// none): "21.5 °C". Written in the person's locale with three significant digits, as wide
    /// as its widest value so the line does not move.
    pub fn slider_with_value(&mut self, value: &mut f32, range: RangeInclusive<f32>, unit: &str) -> Response {
        let (low, high) = (*range.start(), *range.end());
        self.row().w(Length::Grow(1.0)).gap(10.0).align(Align::Start, Align::Center).open();
        let state = self.slider(value, range);
        self.slide_value(*value, [low, high], 0.0, unit);
        self.close();
        state
    }

    /// Both: with `step` above zero, steps and marks; without, the curve.
    fn slide(&mut self, value: &mut f32, [low, high]: [f32; 2], step: f32, unit: &str) -> Response {
        let (t, was, curved) = (self.theme, *value, step == 0.0);
        self.row().w(Length::Grow(1.0)).gap(10.0).align(Align::Start, Align::Center).open();
        self.column().w(Length::Grow(1.0)).open();
        let mut state = self.interact(self.next_id());
        let mut at = along(*value, [low, high], curved);
        if let (true, Some(p)) = (state.pressed, self.pointer()) {
            at = ((p.x - state.rect.x - KNOB / 2.0) / (state.rect.w - KNOB).max(1.0)).clamp(0.0, 1.0);
            *value = value_at(at, [low, high], curved);
        }
        if state.focused {
            *value = self.slide_keys(*value, at, [low, high], step);
        }
        // Where a screen reader set a value, it is set.
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            *value = to;
        }
        // A slider of steps stands on one.
        if !curved {
            let on = low + ((*value - low) / step).round() * step;
            let places = 10f32.powi(decimals(step, step) as i32);
            *value = (on * places).round() / places;
        }
        *value = value.clamp(low.min(high), high.max(low));
        state.changed = *value != was;
        self.again |= state.changed;
        let at = along(*value, [low, high], curved);
        // The line before and after the knob share the room by the knob's position.
        self.row().w(Length::Grow(1.0)).h(Length::Px(t.control)).radius(4.0).sense().focusable().open();
        self.tell_amount(&state, Role::Slider, [*value, low, high]);
        let next = if curved { value_at(at + 0.05, [low, high], true) - *value } else { step };
        self.tell_step(next.abs());
        self.el().w(Length::Grow(at)).h(Length::Px(4.0)).radius(2.0).fill(t.accent).show();
        self.el().size(KNOB, KNOB).radius(KNOB / 2.0).fill(if state.hovered || state.pressed { t.text } else { t.text_dim }).show();
        self.el().w(Length::Grow(1.0 - at)).h(Length::Px(4.0)).radius(2.0).fill(t.line_strong).show();
        self.close();
        if !curved {
            self.slide_marks([low, high], step, state.rect.w);
        }
        self.close();
        self.slide_value(*value, [low, high], step, unit);
        self.close();
        state
    }

    /// Where keys move a focused slider: a step, or a twentieth of the way on the curve; Page
    /// Up and Page Down ten steps or a quarter of the way; Home and End to the ends.
    fn slide_keys(&mut self, value: f32, at: f32, [low, high]: [f32; 2], step: f32) -> f32 {
        let mut key = |key: Key| self.key(key, Mods::NONE) as i32 as f32;
        let (by, large, [home, end]) = (key(Key::Right) - key(Key::Left), key(Key::PageUp) - key(Key::PageDown), [key(Key::Home), key(Key::End)]);
        match (home > 0.0, end > 0.0, step > 0.0) {
            (true, ..) => low,
            (_, true, _) => high,
            _ if by == 0.0 && large == 0.0 => value,
            (.., true) => value + (by + 10.0 * large) * step,
            (.., false) => value_at(at + by / 20.0 + large / 4.0, [low, high], true),
        }
    }

    /// The marks under a slider of steps, `wide` as last laid out: one per step, or every nth
    /// where they would lie closer than `MARKS_APART`; the ends are always marked. Each lies
    /// under the knob's middle at that step.
    fn slide_marks(&mut self, [low, high]: [f32; 2], step: f32, wide: f32) {
        let t = self.theme;
        let steps = ((high - low) / step).abs().round().max(1.0) as usize;
        // Before its first layout, twenty at most.
        let every = match wide > KNOB {
            true => ((steps as f32 * MARKS_APART) / (wide - KNOB)).ceil() as usize,
            false => steps.div_ceil(20),
        };
        let every = every.max(1);
        self.row().w(Length::Grow(1.0)).h(Length::Px(6.0)).pad((KNOB / 2.0, 0.0)).open();
        for mark in (0..=steps).step_by(every) {
            if mark > 0 {
                let apart = every.min(steps - (mark - every)) as f32;
                self.el().w(Length::Grow(apart)).show();
            }
            self.el().size(1.0, 5.0).fill(t.line_strong).show();
        }
        // The last step, when only every nth is marked and it is not one of them.
        if !steps.is_multiple_of(every) {
            self.el().w(Length::Grow((steps % every) as f32)).show();
            self.el().size(1.0, 5.0).fill(t.line_strong).show();
        }
        self.close();
    }

    /// A slider's value beside it with its unit, in the person's locale, as wide as its widest
    /// value so the line does not move.
    pub(crate) fn slide_value(&mut self, value: f32, [low, high]: [f32; 2], step: f32, unit: &str) {
        let (t, locale) = (self.theme, self.locale_in_use());
        let (mut written, mut rust) = (std::mem::take(&mut self.scratch), String::new());
        let mut widest = 0.0f32;
        for end in [low, high, value] {
            written.clear();
            rust.clear();
            let _ = write!(rust, "{:.*}", decimals(step, end), end);
            locale.write_number(&mut written, &rust, true);
            if !unit.is_empty() {
                written.push(' ');
                written.push_str(unit);
            }
            widest = widest.max(self.text_width(&written, t.font));
        }
        self.el().w(Length::Px(widest.ceil())).text(&written, t.font, t.text).text_align(Align::End).show();
        self.scratch = written;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_multiplies_above_nought_and_squares_from_it() {
        let zoom = [25.0, 400.0];
        assert!((value_at(0.5, zoom, true) - 100.0).abs() < 0.01, "half way from 25 to 400 is 100");
        assert!((value_at(0.75, zoom, true) - 200.0).abs() < 0.01);
        assert!((along(100.0, zoom, true) - 0.5).abs() < 1e-5);
        let volume = [0.0, 100.0];
        assert_eq!([value_at(0.5, volume, true), value_at(1.0, volume, true), value_at(0.0, volume, true)], [25.0, 100.0, 0.0]);
        assert!((along(25.0, volume, true) - 0.5).abs() < 1e-6);
        assert_eq!([along(5.0, [0.0, 10.0], false), value_at(0.3, [0.0, 10.0], false)], [0.5, 3.0]);
        // Every value comes back as itself.
        for value in [25.0, 31.5, 100.0, 333.0, 400.0] {
            assert!((value_at(along(value, zoom, true), zoom, true) - value).abs() < 0.01, "{value}");
        }
        assert_eq!([decimals(1.0, 3.0), decimals(0.25, 0.0), decimals(0.1, 0.0), decimals(0.0, 100.0), decimals(0.0, 1.0), decimals(0.0, 33.5)], [0, 2, 1, 0, 2, 1]);
    }
}
