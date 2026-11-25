//! SMIL animation in SVG files: `<animate>`, `<set>` and `<animateTransform>` (and the older
//! `<animateColor>`), timed as SMIL says and drawn at any moment.
//!
//! A moment is drawn as the file would be with each animated attribute at its value then: the
//! values are written into a copy of the text (a property at the end of its element's `style`,
//! where it wins over style rules; any other attribute in place), and that copy is read as any
//! drawing is. So a frame costs what such a file costs, and the cost budget bounds each.
//!
//! Timing: `begin` as clock values (the earliest of a list; an event or another animation never
//! begins it), `dur`, `end`, `repeatCount`, `repeatDur`, `fill="freeze"`. Values: `values` with
//! `keyTimes`, or `from`, `to`, `by`; `calcMode` discrete, linear, paced or spline
//! (`keySplines`); `additive` and `accumulate` sums. Numbers and lists of them (lengths,
//! `points`, path data with the same commands, `viewBox`, transforms) are interpolated number by
//! number, colours channel by channel; other values change halfway, as a discrete step does.
use crate::svg::{File, color, number};

/// The properties the reader takes from `style` as well as from attributes (see
/// `Reading::take_property`): an animated one is written into `style`.
const PROPERTIES: [&str; 41] = [
    "fill",
    "stroke",
    "color",
    "fill-opacity",
    "stroke-opacity",
    "opacity",
    "fill-rule",
    "stroke-width",
    "stroke-miterlimit",
    "stroke-dashoffset",
    "vector-effect",
    "stroke-dasharray",
    "stroke-linecap",
    "stroke-linejoin",
    "display",
    "visibility",
    "paint-order",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "mask",
    "clip-rule",
    "overflow",
    "marker-start",
    "marker-mid",
    "marker-end",
    "marker",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "letter-spacing",
    "filter",
    "flood-color",
    "flood-opacity",
    "color-interpolation-filters",
    "shape-rendering",
    "lighting-color",
    "direction",
    "font-variant",
];

/// Frames per second of an animation encoded as frames.
const FRAMES: f32 = 30.0;

/// The most pixels all frames of an encoded animation may come to: past it, fewer frames.
const PIXELS: usize = 1 << 26;

/// The longest cycle of an animation that runs for ever: one whose parts meet only later is cut
/// there.
const CYCLE: f32 = 60.0;

/// How an animation goes from value to value.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Discrete,
    Linear,
    /// Linear, the key times spread so that it moves at one speed.
    Paced,
    /// Each step eased along a cubic curve (`keySplines`).
    Spline,
}

/// When an animation runs: from `begin` (seconds; never where infinite), each cycle `simple`
/// seconds long (none: no end, and no change), for `active` seconds in all (maybe infinite).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Timing {
    begin: f32,
    simple: Option<f32>,
    active: f32,
}

/// One animation element as read.
struct Animation<'a> {
    /// The element and attribute it animates.
    target: usize,
    name: &'a str,
    /// Its values, as written (for `animateTransform`, each as the transform it is).
    values: Vec<String>,
    /// When each value is reached, 0 to 1 (empty: evenly), and how to ease between (`Spline`).
    times: Vec<f32>,
    splines: Vec<[f32; 4]>,
    mode: Mode,
    timing: Timing,
    freeze: bool,
    /// Whether it adds to the value under it, and adds its last value once per cycle repeated.
    additive: bool,
    accumulate: bool,
}

/// A file's animations, in the order they apply.
pub(crate) struct Timeline<'a> {
    file: File<'a>,
    animations: Vec<Animation<'a>>,
}

impl<'a> Timeline<'a> {
    /// Reads a file's animations. (None where it has none.)
    pub(crate) fn read(text: &'a str) -> Timeline<'a> {
        let file = File::read(text);
        let animations = (0..file.elements.len()).filter_map(|at| animation(&file, at)).collect();
        Timeline { file, animations }
    }

    /// The file's text with every animated attribute at its value `seconds` after it begins.
    pub(crate) fn frame(&self, seconds: f32) -> String {
        // Each animated attribute's value, by element: animations replace or add to the value
        // under them, in order.
        let mut values: Vec<(usize, &str, String)> = Vec::new();
        for animation in &self.animations {
            let Some(value) = animation.at(seconds) else { continue };
            let under = values.iter().rposition(|(at, name, _)| (*at, *name) == (animation.target, animation.name));
            let value = match (animation.additive, under) {
                (true, Some(under)) => added(&values[under].2, &value, animation.name),
                (true, None) => added(&base(&self.file, animation.target, animation.name), &value, animation.name),
                (false, _) => value,
            };
            match under {
                Some(under) => values[under].2 = value,
                None => values.push((animation.target, animation.name, value)),
            }
        }
        written(&self.file, &values)
    }

    /// The span worth drawing as frames: where it starts and how long it is (seconds), and how
    /// often it plays (0 for ever). An animation that runs for ever is one cycle of all that
    /// repeats, from when all of them have begun and the rest have ended. Nothing without
    /// animations.
    pub(crate) fn span(&self) -> Option<(f32, f32, u32)> {
        let timings: Vec<Timing> = self.animations.iter().map(|animation| animation.timing).filter(|timing| timing.begin.is_finite()).collect();
        let repeating: Vec<&Timing> = timings.iter().filter(|timing| timing.active.is_infinite() && timing.simple.is_some()).collect();
        // Where each that ends (or stands still once begun) is done.
        let done = |timing: &&Timing| timing.begin + if timing.active.is_finite() { timing.active } else { 0.0 };
        let last = timings.iter().filter(|timing| !(timing.active.is_infinite() && timing.simple.is_some())).map(|timing| done(&timing)).fold(0.0f32, f32::max);
        if repeating.is_empty() {
            return (!timings.is_empty()).then_some((0.0, last, 1));
        }
        let start = repeating.iter().map(|timing| timing.begin).fold(last, f32::max);
        // The cycles' least common multiple, in milliseconds.
        let cycle = repeating.iter().filter_map(|timing| timing.simple).map(|simple| (simple * 1000.0).round().max(1.0) as u64).fold(1, |all, one| (all / gcd(all, one)).saturating_mul(one));
        Some((start, (cycle as f32 / 1000.0).min(CYCLE), 0))
    }
}

impl Animation<'_> {
    /// Its value `seconds` after the file begins, where it applies then (with its accumulated
    /// sum, where it says so).
    fn at(&self, seconds: f32) -> Option<String> {
        let Timing { begin, simple, active } = self.timing;
        let local = seconds - begin;
        if local.is_nan() || local < 0.0 || (local >= active && !self.freeze) {
            return None;
        }
        let Some(simple) = simple else { return self.values.first().cloned() };
        // Frozen at the end of its active time: the end of a cycle where it ends with one.
        let local = local.min(active);
        let (mut cycle, mut within) = ((local / simple).floor(), local % simple / simple);
        if local >= active && within == 0.0 && cycle > 0.0 {
            (cycle, within) = (cycle - 1.0, 1.0);
        }
        let value = self.within(within);
        match (self.accumulate, self.values.last()) {
            (true, Some(last)) if cycle >= 1.0 => Some(added(&value, &scaled(last, cycle), self.name)),
            _ => Some(value),
        }
    }

    /// Its value at `share` (0 to 1) of a cycle.
    fn within(&self, share: f32) -> String {
        let count = self.values.len();
        if count < 2 {
            return self.values.first().cloned().unwrap_or_default();
        }
        if self.mode == Mode::Discrete {
            let even = || ((share * count as f32) as usize).min(count - 1);
            let at = if self.times.len() == count { self.times.iter().rposition(|time| *time <= share).unwrap_or(0) } else { even() };
            return self.values[at].clone();
        }
        let times = self.key_times();
        let step = times.windows(2).position(|pair| share < pair[1]).unwrap_or(count - 2);
        let local = ((share - times[step]) / (times[step + 1] - times[step])).clamp(0.0, 1.0);
        let local = match self.splines.get(step).filter(|_| self.mode == Mode::Spline) {
            Some(spline) => eased(spline, local),
            None => local,
        };
        between(&self.values[step], &self.values[step + 1], local)
    }

    /// When each value is reached, 0 to 1: as `keyTimes` says, else at one speed (paced, where
    /// its values have distances), else evenly.
    fn key_times(&self) -> Vec<f32> {
        let count = self.values.len();
        if self.times.len() == count && self.mode != Mode::Paced {
            return self.times.clone();
        }
        let distances: Option<Vec<f32>> = self.values.windows(2).map(|pair| distance(&pair[0], &pair[1])).collect();
        let whole: f32 = distances.iter().flatten().sum();
        let Some(distances) = distances.filter(|_| self.mode == Mode::Paced && whole > 0.0) else {
            return (0..count).map(|at| at as f32 / (count - 1) as f32).collect();
        };
        let (mut times, mut gone) = (vec![0.0], 0.0);
        for step in distances {
            gone += step;
            times.push(gone / whole);
        }
        times
    }
}

/// The animation element `at` is, if it is one: its target (the element it references, else
/// its parent), attribute, values and timing. Nothing for another element, or one with no
/// attribute or values to animate.
fn animation<'a>(file: &File<'a>, at: usize) -> Option<Animation<'a>> {
    let kind = file.elements[at].name;
    if !matches!(kind, "animate" | "set" | "animateTransform" | "animateColor") {
        return None;
    }
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    let target = file.linked(at).or_else(|| (0..at).rev().find(|parent| file.elements[*parent].end > at))?;
    let turn = (kind == "animateTransform").then(|| attr("type").unwrap_or("translate"));
    let name = attr("attributeName").or(turn.map(|_| "transform"))?;
    let mut values = values(file, (at, target, name), kind == "set")?;
    if let Some(turn) = turn {
        values = values.iter().map(|value| transform_value(turn, value)).collect();
    }
    let times = attr("keyTimes").map(|text| text.split(';').filter_map(|time| number(time).map(|(time, _)| time)).collect()).unwrap_or_default();
    let splines: Vec<[f32; 4]> = attr("keySplines").map(|text| text.split(';').filter_map(|spline| crate::svg::numbers(spline).collect::<Vec<f32>>().try_into().ok()).collect()).unwrap_or_default();
    let mode = match attr("calcMode") {
        _ if kind == "set" => Mode::Discrete,
        Some("discrete") => Mode::Discrete,
        Some("paced") => Mode::Paced,
        Some("spline") => Mode::Spline,
        _ => Mode::Linear,
    };
    // `by` alone adds to what is under it.
    let by_alone = attr("values").is_none() && attr("from").is_none() && attr("to").is_none() && attr("by").is_some();
    let additive = attr("additive") == Some("sum") || by_alone;
    Some(Animation {
        target,
        name,
        values,
        times,
        splines,
        mode,
        timing: timing(file, at, kind == "set"),
        freeze: attr("fill") == Some("freeze"),
        additive,
        accumulate: attr("accumulate") == Some("sum"),
    })
}

/// An animation's values: `values`, else `from` with `to` or with `by` added, `by` alone from
/// nothing, or `to` alone from the attribute's own value (`set`: `to` only). Nothing where it
/// gives none.
fn values(file: &File, (at, target, name): (usize, usize, &str), set: bool) -> Option<Vec<String>> {
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    if set {
        return attr("to").map(|to| vec![to.to_string()]);
    }
    if let Some(values) = attr("values") {
        let values: Vec<String> = values.split(';').map(str::trim).filter(|value| !value.is_empty()).map(String::from).collect();
        return (!values.is_empty()).then_some(values);
    }
    let base = || base(file, target, name);
    match (attr("from"), attr("to"), attr("by")) {
        (Some(from), Some(to), _) => Some(vec![from.to_string(), to.to_string()]),
        (Some(from), None, Some(by)) => Some(vec![from.to_string(), added(from, by, name)]),
        (None, None, Some(by)) => Some(vec![scaled(by, 0.0), by.to_string()]),
        (None, Some(to), _) => Some(vec![base(), to.to_string()]),
        _ => None,
    }
}

/// An attribute's own value on an element: in its `style` or as an attribute; else nothing
/// (numbers from zero, colours from black).
fn base(file: &File, at: usize, name: &str) -> String {
    file.own_property(at, name).unwrap_or(if name.contains("color") || name == "fill" || name == "stroke" { "black" } else { "0" }).to_string()
}

/// A transform of a type with its numbers as `animateTransform` gives them: those left out
/// filled in (a translation's y as 0, a scale's y as its x, a rotation's centre as the origin).
fn transform_value(turn: &str, value: &str) -> String {
    let numbers: Vec<f32> = crate::svg::numbers(value).collect();
    let all: Vec<f32> = match turn {
        "scale" => vec![numbers.first().copied().unwrap_or(1.0), numbers.get(1).or(numbers.first()).copied().unwrap_or(1.0)],
        "rotate" => (0..3).map(|n| numbers.get(n).copied().unwrap_or(0.0)).collect(),
        "skewX" | "skewY" => vec![numbers.first().copied().unwrap_or(0.0)],
        _ => (0..2).map(|n| numbers.get(n).copied().unwrap_or(0.0)).collect(),
    };
    format!("{turn}({})", all.iter().map(f32::to_string).collect::<Vec<String>>().join(" "))
}

/// When an animation runs (see `Timing`): `set` holds its value as long as nothing ends it.
fn timing(file: &File, at: usize, set: bool) -> Timing {
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    let begin = match attr("begin") {
        None => 0.0,
        Some(list) => list.split(';').filter_map(clock).fold(f32::INFINITY, f32::min),
    };
    let simple = attr("dur").and_then(clock).filter(|simple| *simple > 0.0 && !set);
    let repeats = attr("repeatCount").and_then(|count| if count == "indefinite" { Some(f32::INFINITY) } else { number(count).map(|(count, _)| count).filter(|count| *count > 0.0) });
    let repeated = attr("repeatDur").and_then(|span| if span == "indefinite" { Some(f32::INFINITY) } else { clock(span) });
    let active = match (simple, repeats, repeated) {
        (None, ..) => f32::INFINITY,
        (Some(simple), None, None) => simple,
        (Some(simple), count, span) => (simple * count.unwrap_or(f32::INFINITY)).min(span.unwrap_or(f32::INFINITY)),
    };
    let ends = attr("end").and_then(|list| list.split(';').filter_map(clock).reduce(f32::min)).map_or(f32::INFINITY, |end| end - begin);
    Timing { begin, simple, active: active.min(ends).max(0.0) }
}

/// A SMIL clock value in seconds: `h:mm:ss.f`, `mm:ss.f`, or a number with `h`, `min`, `s`
/// or `ms` (seconds by default). Nothing for anything else (an event, `indefinite`).
fn clock(text: &str) -> Option<f32> {
    let text = text.trim().trim_start_matches('+');
    if text.contains(':') {
        let parts: Vec<f32> = text.split(':').map(|part| part.trim().parse::<f32>().ok()).collect::<Option<Vec<f32>>>()?;
        return match parts[..] {
            [minutes, seconds] => Some(minutes * 60.0 + seconds),
            [hours, minutes, seconds] => Some(hours * 3600.0 + minutes * 60.0 + seconds),
            _ => None,
        };
    }
    let (count, unit) = number(text)?;
    let times = match unit.trim() {
        "" | "s" => 1.0,
        "ms" => 0.001,
        "min" => 60.0,
        "h" => 3600.0,
        _ => return None,
    };
    Some(count * times)
}

/// A value's numbers, and the rest of it with whitespace and commas left out: two values of one
/// kind have the same rest. (For `#rgb` colours, none.)
fn parts(text: &str) -> (Vec<f32>, String) {
    let (mut numbers, mut rest, mut at) = (Vec::new(), String::new(), 0);
    while at < text.len() {
        let here = &text[at..];
        let starts = here.starts_with(|c: char| c.is_ascii_digit() || c == '.') || (here.starts_with(['-', '+']) && here[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.'));
        if let Some((number, after)) = number(here).filter(|_| starts) {
            numbers.push(number);
            at = text.len() - after.len();
            continue;
        }
        let letter = here.chars().next().unwrap_or(' ');
        if !letter.is_whitespace() && letter != ',' {
            rest.push(letter);
        }
        at += letter.len_utf8();
    }
    (numbers, rest)
}

/// A value written as `text` is, with its numbers replaced in order by `numbers`.
fn with_numbers(text: &str, numbers: &[f32]) -> String {
    let (mut written, mut at, mut next) = (String::with_capacity(text.len()), 0, numbers.iter());
    while at < text.len() {
        let here = &text[at..];
        let starts = here.starts_with(|c: char| c.is_ascii_digit() || c == '.') || (here.starts_with(['-', '+']) && here[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.'));
        if let Some((_, after)) = number(here).filter(|_| starts) {
            // (A number running into the last, as "1-2", gets a space before it.)
            if written.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
                written.push(' ');
            }
            written += &next.next().copied().unwrap_or(0.0).to_string();
            at = text.len() - after.len();
            continue;
        }
        let letter = here.chars().next().unwrap_or(' ');
        written.push(letter);
        at += letter.len_utf8();
    }
    written
}

/// A colour's channels (red, green, blue, alpha) as levels, where `text` is a colour.
fn channels(text: &str) -> Option<[f32; 4]> {
    let color = color(&text.trim().to_ascii_lowercase())?.0;
    Some([16, 8, 0, 24].map(|shift| (color >> shift & 255) as f32))
}

/// A colour from channels (see `channels`), written as `#rrggbbaa`.
fn written_color(levels: [f32; 4]) -> String {
    let [r, g, b, a] = levels.map(|level| level.round().clamp(0.0, 255.0) as u32);
    format!("#{:08x}", r << 24 | g << 16 | b << 8 | a)
}

/// The value `share` (0 to 1) of the way from one value to another: colours channel by channel,
/// values of one kind number by number, others in one step halfway.
fn between(from: &str, to: &str, share: f32) -> String {
    if let (Some(from), Some(to)) = (channels(from), channels(to)) {
        return written_color(std::array::from_fn(|n| from[n] + (to[n] - from[n]) * share));
    }
    let ((low, kind), (high, other)) = (parts(from), parts(to));
    if kind == other && low.len() == high.len() {
        let numbers: Vec<f32> = low.iter().zip(&high).map(|(low, high)| low + (high - low) * share).collect();
        return with_numbers(from, &numbers);
    }
    if share < 0.5 { from } else { to }.to_string()
}

/// One value added to another: colours channel by channel (alpha kept), numbers of one kind
/// number by number, transforms one after the other; else the second alone.
fn added(under: &str, over: &str, name: &str) -> String {
    if name == "transform" {
        return format!("{under} {over}");
    }
    if let (Some(under), Some(over)) = (channels(under), channels(over)) {
        return written_color([under[0] + over[0], under[1] + over[1], under[2] + over[2], under[3]]);
    }
    let ((low, kind), (high, other)) = (parts(under), parts(over));
    match kind == other && low.len() == high.len() {
        true => with_numbers(under, &low.iter().zip(&high).map(|(low, high)| low + high).collect::<Vec<f32>>()),
        false => over.to_string(),
    }
}

/// A value with its numbers (or colour channels, alpha aside) times `by`.
fn scaled(text: &str, by: f32) -> String {
    if let Some([r, g, b, a]) = channels(text) {
        return written_color([r * by, g * by, b * by, a]);
    }
    with_numbers(text, &parts(text).0.iter().map(|number| number * by).collect::<Vec<f32>>())
}

/// How far apart two values are, for pacing: between colours or numbers of one kind. Nothing
/// for values that have no distance.
fn distance(from: &str, to: &str) -> Option<f32> {
    let squared = |pairs: &mut dyn Iterator<Item = (f32, f32)>| pairs.map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt();
    if let (Some(from), Some(to)) = (channels(from), channels(to)) {
        return Some(squared(&mut from.into_iter().zip(to)));
    }
    let ((low, kind), (high, other)) = (parts(from), parts(to));
    (kind == other && low.len() == high.len()).then(|| squared(&mut low.into_iter().zip(high)))
}

/// Where a cubic easing curve through (0, 0), (x1, y1), (x2, y2), (1, 1) stands at `share` of
/// the way across.
fn eased([x1, y1, x2, y2]: &[f32; 4], share: f32) -> f32 {
    let curve = |a: f32, b: f32, t: f32| 3.0 * a * t * (1.0 - t) * (1.0 - t) + 3.0 * b * t * t * (1.0 - t) + t * t * t;
    // Its x grows with t: halving finds the t at `share` to well within a level.
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if curve(*x1, *x2, middle) < share {
            low = middle;
        } else {
            high = middle;
        }
    }
    curve(*y1, *y2, (low + high) / 2.0)
}

/// The greatest common divisor of two whole numbers.
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// Attributes by name, with values.
type Values<'a> = Vec<(&'a str, String)>;

/// A file's text with attributes set to values (each by element and name): the properties at
/// the end of their element's `style`, where they win over style rules and attributes; any
/// other attribute in place, or added after its element's name.
fn written(file: &File, values: &[(usize, &str, String)]) -> String {
    // Each change: where in the text, how many bytes it replaces, and with what. (Quotes in a
    // value would end it: they go.)
    let mut changes: Vec<(usize, usize, String)> = Vec::new();
    let mut elements: Vec<usize> = values.iter().map(|(at, ..)| *at).collect();
    elements.sort_unstable();
    elements.dedup();
    for at in elements {
        let named_end = file.place_of(file.elements[at].name)[1] as usize;
        let mine = values.iter().filter(|(of, ..)| *of == at).map(|(_, name, value)| (*name, value.replace(['"', '\''], "")));
        let (properties, attributes): (Values, Values) = mine.partition(|(name, _)| PROPERTIES.contains(name));
        let declared: String = properties.iter().map(|(name, value)| format!(";{name}:{value}")).collect();
        match file.attribute(at, "style") {
            _ if declared.is_empty() => {}
            Some(style) => changes.push((file.place_of(style)[1] as usize, 0, declared)),
            None => changes.push((named_end, 0, format!(" style=\"{}\"", &declared[1..]))),
        }
        for (name, value) in attributes {
            let [from, to] = file.attribute(at, name).map_or([named_end as u32; 2], |own| file.place_of(own));
            let change = if file.attribute(at, name).is_some() { value } else { format!(" {name}=\"{value}\"") };
            changes.push((from as usize, (to - from) as usize, change));
        }
    }
    // From the end, so that places further on stay right. (Additions at one place may go in
    // any order.)
    changes.sort_by_key(|(place, ..)| std::cmp::Reverse(*place));
    let mut text = file.text.to_string();
    for (place, long, with) in changes {
        text.replace_range(place..place + long, &with);
    }
    text
}

/// The SMIL animation of an SVG file as an animated PNG, `width` by `height` pixels (see
/// `svg::encode_animation`).
pub(crate) fn encode(svg: &str, width: usize, height: usize) -> Option<Vec<u8>> {
    let timeline = Timeline::read(svg);
    let (start, long, plays) = timeline.span()?;
    // A one-time animation ends on a frame of its own, shown from then on.
    let ends = plays == 1;
    let most = (PIXELS / width.saturating_mul(height).max(1)).max(1);
    let frames = ((long * FRAMES).ceil() as usize + ends as usize).clamp(1, most);
    let steps = frames - ends as usize;
    let shows = ((long / steps.max(1) as f32 * 1000.0).round() as u16).max(1);
    let mut drawn = Vec::with_capacity(frames);
    for n in 0..frames {
        let seconds = start + long * n as f32 / steps.max(1) as f32;
        let (_, pixels) = crate::svg::decode(&timeline.frame(seconds), width, height)?;
        drawn.push((pixels, shows));
    }
    crate::png::encode_animation(width, &drawn, plays)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_values_are_read_in_every_form() {
        let read = ["2", "2s", "1.5s", "500ms", "1min", "0.5h", "01:30", "0:01:02.5", "+3s", "click", "indefinite", "2x"].map(clock);
        assert_eq!(read, [Some(2.0), Some(2.0), Some(1.5), Some(0.5), Some(60.0), Some(1800.0), Some(90.0), Some(62.5), Some(3.0), None, None, None]);
    }

    #[test]
    fn values_of_one_kind_go_number_by_number_and_others_in_one_step() {
        assert_eq!(between("10", "20", 0.25), "12.5");
        assert_eq!(between("M0,0 L10 10", "M10,20 L30 50", 0.5), "M5,10 L20 30");
        assert_eq!(between("10px", "20px", 0.5), "15px");
        assert_eq!(between("red", "blue", 0.5), "#800080ff");
        assert_eq!([between("visible", "hidden", 0.49), between("visible", "hidden", 0.5)], ["visible", "hidden"]);
        // Paths of other commands change in one step too.
        assert_eq!(between("M0 0L1 1", "M0 0C1 1 2 2 3 3", 0.4), "M0 0L1 1");
    }

    #[test]
    fn an_animation_is_timed_as_smil_says() {
        let file = r#"<svg><rect width="10"><animate attributeName="width" from="10" to="20" begin="1s" dur="2s" repeatCount="2" fill="freeze"/></rect></svg>"#;
        let timeline = Timeline::read(file);
        let width = |seconds: f32| {
            let frame = timeline.frame(seconds);
            let file = File::read(&frame);
            file.attribute(1, "width").map(str::to_string)
        };
        assert_eq!([0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 9.0].map(width), ["10", "10", "15", "10", "15", "20", "20"].map(|value| Some(value.to_string())));
        assert_eq!(timeline.span(), Some((0.0, 5.0, 1)));
    }

    #[test]
    fn key_times_paces_and_splines_place_the_values() {
        let x = |extra: &str, seconds: f32| {
            let text = format!(r#"<svg><rect x="0"><animate attributeName="x" values="0;10;30" dur="1s" {extra}/></rect></svg>"#);
            let frame = Timeline::read(&text).frame(seconds);
            parts(File::read(&frame).attribute(1, "x").unwrap_or("")).0[0]
        };
        // Evenly: halfway is the middle value; with key times, where they put it.
        assert_eq!(x("", 0.5), 10.0);
        assert_eq!(x(r#"keyTimes="0;0.8;1""#, 0.4), 5.0);
        // Paced: one speed over 30 units, so halfway is 15.
        assert_eq!(x(r#"calcMode="paced""#, 0.5), 15.0);
        // Discrete: thirds.
        assert_eq!(x(r#"calcMode="discrete""#, 0.5), 10.0);
        // A spline easing in: slower at first.
        let eased = x(r#"calcMode="spline" keySplines="0.5 0 1 1;0 0 1 1""#, 0.25);
        assert!(eased < 5.0 && eased > 0.0, "{eased}");
    }

    #[test]
    fn transforms_colours_sets_and_sums_are_written_where_they_count() {
        let file = r##"<svg><style>rect { fill: green }</style><rect id="r" transform="translate(5)" style="opacity:1"><animateTransform attributeName="transform" type="rotate" from="0 5 5" to="90 5 5" dur="1s" additive="sum"/><animate attributeName="fill" from="red" to="blue" dur="1s"/><set attributeName="visibility" to="hidden" begin="2s"/></rect><animate href="#r" attributeName="x" by="10" dur="1s" accumulate="sum" repeatCount="3"/></svg>"##;
        let timeline = Timeline::read(file);
        let frame = timeline.frame(0.5);
        let read = File::read(&frame);
        assert_eq!(read.attribute(2, "transform"), Some("translate(5) rotate(45 5 5)"));
        // A property goes into `style`, last: it wins over the style sheet.
        assert_eq!(read.own_property(2, "fill"), Some("#800080ff"));
        assert_eq!(read.own_property(2, "visibility"), None);
        assert_eq!(File::read(&timeline.frame(2.0)).own_property(2, "visibility"), Some("hidden"));
        // `by` alone adds to the value under it; each cycle repeated adds its last value.
        assert_eq!(read.attribute(2, "x"), Some("5"));
        assert_eq!(File::read(&timeline.frame(1.5)).attribute(2, "x"), Some("15"));
        // A set with no end, and the rest done: three seconds once.
        assert_eq!(timeline.span(), Some((0.0, 3.0, 1)));
    }

    #[test]
    fn what_runs_for_ever_is_one_cycle_of_all_that_repeats() {
        let file = r#"<svg><rect><animate attributeName="x" values="0;1" dur="0.4s" repeatCount="indefinite"/><animate attributeName="y" values="0;1" dur="0.6s" begin="1s" repeatCount="indefinite"/></rect></svg>"#;
        let (start, long, plays) = Timeline::read(file).span().unwrap();
        assert!(start == 1.0 && (long - 1.2).abs() < 1e-6 && plays == 0, "{start} {long} {plays}");
        assert_eq!(Timeline::read("<svg><rect/></svg>").span(), None);
    }

    #[test]
    fn an_animation_encodes_as_frames_that_play_as_drawn() {
        // A second-long move that ends: 31 frames (the last its end, shown from then on), each
        // as the drawing is at its time.
        let file =
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"><rect width="5" height="10" fill="red"><animate attributeName="x" from="0" to="15" dur="1s" fill="freeze"/></rect></svg>"#;
        let mut moving = crate::animation::LoadedAnimation::read(&encode(file, 20, 10).unwrap(), usize::MAX).unwrap();
        assert_eq!((moving.frames.len(), moving.plays, moving.frames[0].shows), (31, Some(1), 33));
        for (frame, seconds) in [(0, 0.0), (15, 0.5), (30, 1.0)] {
            assert_eq!(moving.shown(frame).0, &crate::svg::decode_at(file, 20, 10, seconds).unwrap().1[..], "frame {frame}");
        }
        // One that repeats for ever plays its cycle for ever; a still file encodes as nothing.
        let spinning = file.replace(r#"fill="freeze""#, r#"repeatCount="indefinite""#);
        let spinning = crate::animation::LoadedAnimation::read(&encode(&spinning, 20, 10).unwrap(), usize::MAX).unwrap();
        assert_eq!((spinning.frames.len(), spinning.plays), (30, None));
        assert!(encode(r#"<svg viewBox="0 0 2 2"><rect width="1" height="1"/></svg>"#, 4, 4).is_none());
    }
}
