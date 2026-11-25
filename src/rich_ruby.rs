//! Readings (ruby) over rich text ([`Ui::rich`](crate::Ui::rich)), as `ruby.rs` places them:
//! the code `Span::ruby` hands over, with the feature `ruby`. A piece with a reading is one
//! unbreakable thing on its row: as long as the longer of base and reading, its row as tall as
//! the reading's top needs; the base on the row's baseline, the reading over it, the shorter of
//! the two spread over the other's length.
use crate::rich_text::{Bit, Frag, Laid, Shape, Thing};
use crate::ruby::{can_spread, places, spread};
use crate::text::{Decoration, Font, TextSystem};
use crate::ui::Ui;

/// Measures a piece with a reading (see `measure`).
type Measures = fn(&mut Laid, usize, &str, &mut dyn TextSystem, f32, f32) -> f32;

/// Describes a piece with a reading (see `write`).
type Writes = fn(&mut Ui, &Shape, &Frag, [f32; 2], &[u32]);

/// The code that sets readings in rich text ([`Ui::rich`](crate::Ui::rich)), handed over by
/// [`Span::ruby`](crate::Span::ruby) (`RUBY`): a program that sets none carries none of it. (`told`
/// adds a reading to a paragraph's text, see `told`; `key` stands for it in a piece's key.)
pub(crate) struct Hooks {
    pub key: fn(&str) -> u64,
    pub told: fn(&mut String, &str) -> Thing,
    pub measure: Measures,
    pub write: Writes,
}

/// The hooks, once a reading is set.
pub(crate) static RUBY: std::sync::OnceLock<&'static Hooks> = std::sync::OnceLock::new();

/// The one set of hooks.
pub(crate) static HOOKS: Hooks = Hooks { key: |reading| crate::id::hash(reading), told, measure, write };

/// Adds a reading to a paragraph's text after its base, in brackets, as readers are told it:
/// the piece it makes of its base.
fn told(text: &mut String, reading: &str) -> Thing {
    text.push('(');
    let start = text.len() as u32;
    text.push_str(reading);
    text.push(')');
    Thing::Ruby([start, text.len() as u32 - 1])
}

/// The reading's font: half the base's, undecorated.
fn reading_font(base: Font) -> Font {
    Font { size: base.size * crate::vertical_layout::READING, decoration: Decoration::None, ..base }
}

/// How much of a font's em lies above its baseline: from its file where the library reads it,
/// else its ascent and descent.
fn em_above(system: &mut dyn TextSystem, font: Font, letter: char, scale: f32) -> f32 {
    if let Some(file) = system.file(font.face, letter) {
        return file.em_above();
    }
    let metrics = system.metrics(font, scale);
    metrics.ascent / (metrics.ascent + metrics.descent).max(f32::MIN_POSITIVE)
}

/// How far above the base's baseline its reading's baseline is, and the reading's top (its em
/// box's, as Chrome finds it): the reading's em box stands on the base's, each em's part above
/// (and the reading's below) its baseline rounded up, no more than its ascent (descent).
fn reading_height(system: &mut dyn TextSystem, base: Font, letters: [char; 2], scale: f32) -> [f32; 2] {
    let small = reading_font(base);
    let (big, little) = (system.metrics(base, scale), system.metrics(small, scale));
    let base_em = (base.size * em_above(system, base, letters[0], scale)).ceil().min(big.ascent);
    let reading_above = small.size * em_above(system, small, letters[1], scale);
    let reading_em = (small.size - reading_above).ceil().min(little.descent);
    let line = base_em + reading_em;
    [line, line + little.ascent - (little.ascent - reading_above).max(0.0).floor()]
}

/// Measures ruby piece `at` (its reading among `text` as its thing says) from `x` along its
/// paragraph's long line: as long as the longer of base and reading, as tall above the baseline
/// as the reading's top; a reading longer than its base reaching back over the text before it
/// (see `reach`). Returns where the text after it starts (reached over too).
fn measure(laid: &mut Laid, at: usize, text: &str, system: &mut dyn TextSystem, scale: f32, x: f32) -> f32 {
    let bit = laid.bits[at];
    let Thing::Ruby(reading) = bit.thing else { return x };
    let (base, read) = (&text[bit.words[0] as usize..bit.words[1] as usize], &text[reading[0] as usize..reading[1] as usize]);
    let (base_long, read_long) = (system.measure(base, bit.font, scale, None), system.measure(read, reading_font(bit.font), scale, None));
    let long = base_long.max(read_long);
    let letters = [base.chars().next().unwrap_or(' '), read.chars().next().unwrap_or(' ')];
    let [above, below] = crate::rich_text::heights(system, bit.font, scale, &mut Vec::new());
    let [_, top] = reading_height(system, bit.font, letters, scale);
    let firsts = |words: &str| cluster_firsts(words);
    let reaches = crate::ruby::reach(long - base_long, places(&firsts(base)), reading_font(bit.font).size);
    // (Over text no larger than its base, by at most half that text.)
    let beside = |other: &Bit| matches!(other.thing, Thing::Words) && other.font.size <= bit.font.size && reaches > 0.0;
    let back = at.checked_sub(1).map(|before| laid.bits[before]).filter(beside).map_or(0.0, |before| reaches.min(before.wide / 2.0));
    let after = laid.bits.get(at + 1).copied().filter(beside);
    let ahead = after.map_or(0.0, |after| reaches.min(system.measure(&text[after.words[0] as usize..after.words[1] as usize], after.font, scale, None) / 2.0));
    // (Raised by whole pixels, so its base lies on the row's baseline as other pieces do.)
    let raised = ((top - above).max(0.0) * scale).ceil() / scale;
    let bit = &mut laid.bits[at];
    (bit.x, bit.wide, bit.stops, bit.above, bit.below) = (x - back, long, laid.stops.len() as u32, above + raised, below);
    x - back + long - ahead
}

/// The first character of each grapheme cluster of some text.
fn cluster_firsts(text: &str) -> Vec<char> {
    let (mut firsts, mut at) = (Vec::new(), 0);
    while at < text.len() {
        firsts.extend(text[at..].chars().next());
        at += crate::text::cluster(&text[at..]);
    }
    firsts
}

/// Describes a piece with its reading over it, its fragment `frag` at `x`, `y`: the base on the
/// row's baseline, the reading on its own over it; the shorter of the two spread over the
/// other's length. A link's base is underlined while in `lit`.
fn write(ui: &mut Ui, shape: &Shape, frag: &Frag, [x, y]: [f32; 2], lit: &[u32]) {
    let bit = shape.laid.bits[frag.bit as usize];
    let Thing::Ruby(reading) = bit.thing else { return };
    let (base, read) = (&shape.text[bit.words[0] as usize..bit.words[1] as usize], &shape.text[reading[0] as usize..reading[1] as usize]);
    let (small, scale) = (reading_font(bit.font), ui.scale());
    let [above, _] = crate::rich_text::heights(ui.text.as_mut(), bit.font, scale, &mut Vec::new());
    let [small_above, _] = crate::rich_text::heights(ui.text.as_mut(), small, scale, &mut Vec::new());
    let letters = [base.chars().next().unwrap_or(' '), read.chars().next().unwrap_or(' ')];
    let [line, _] = reading_height(ui.text.as_mut(), bit.font, letters, scale);
    let font = if bit.link != 0 && lit.contains(&bit.link) { bit.font.underlined() } else { bit.font };
    let top = y + bit.above - above;
    ui.write_spread(base, font, bit.color, [x, top], frag.wide, None);
    ui.write_spread(read, small, bit.color, [x, ((top + above - line - small_above) * scale).round() / scale], frag.wide, Some(small.size));
}

impl Ui {
    /// Text spread over `long` units from `x`, its box's top at `y`: one text box if it is no
    /// shorter, else a box per cluster, spread as `spread` has it (for a reading, of that size).
    fn write_spread(&mut self, words: &str, font: Font, color: crate::color::Color, [x, y]: [f32; 2], long: f32, reading: Option<f32>) {
        let mut ends = Vec::new();
        let own = self.text.measure(words, font, self.scale(), Some(&mut ends));
        let firsts = cluster_firsts(words);
        let shared = spread(long - own, places(&firsts), reading);
        if long - own <= 0.01 || shared.gap == 0.0 {
            self.el().text(words, font, color).select(None, false).offset(x + shared.lead, y).show();
            return;
        }
        let (mut at, mut letters, mut left, mut before) = (0, 0, x + shared.lead, None::<char>);
        while at < words.len() {
            let next = at + crate::text::cluster(&words[at..]);
            let letter = words[at..].chars().next().unwrap_or(' ');
            left += if before.is_some_and(|before| can_spread(before, letter)) { shared.gap } else { 0.0 };
            let start = if letters == 0 { 0.0 } else { ends[letters - 1] };
            letters += words[at..next].chars().count();
            self.el().text(&words[at..next], font, color).select(None, false).offset(left, y).show();
            (left, at, before) = (left + ends[letters - 1] - start, next, Some(letter));
        }
    }
}
