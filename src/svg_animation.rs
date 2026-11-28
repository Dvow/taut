//! SMIL animation in SVG files: `<animate>`, `<set>`, `<animateTransform>` and
//! `<animateMotion>` (and the older `<animateColor>`), timed as SMIL says and drawn at any
//! moment, in a window or as frames.
//!
//! A moment is drawn as the file would be with each animated attribute at its value then: the
//! values are written into a copy of the text (a property at the end of its element's `style`,
//! where it wins over style rules; any other attribute in place), and that copy is read as any
//! drawing is. So a frame costs what such a file costs, and the cost budget bounds each.
//!
//! Timing: `begin` and `end` lists of clock values, other animations' moments (`a.begin`,
//! `a.end-1s`, `a.repeat(2)`, `a.beginEvent`), events (`click`, `a.mouseover+0.5s`,
//! `b.focusin`), `accessKey(x)` and `wallclock(…)`; `dur`, `repeatCount`, `repeatDur`, `min`,
//! `max`, `restart` and `fill="freeze"`. Each animation's intervals are worked out in time order
//! (see `Timeline::schedule`); of animations of one attribute, the one begun last applies last.
//! Events and wall-clock times come from a window only ([`AnimatedDrawing`]): a file drawn at a
//! moment plays what is timed. A window keeps its timeline as it plays and adds each event to
//! it (see `Scheduling::hear`), with the same result as working it out from the start.
//!
//! Values: `values` with `keyTimes`, or `from`, `to`, `by`; `calcMode` discrete, linear, paced
//! or spline (`keySplines`); `additive` and `accumulate` sums. Numbers and lists of them
//! (lengths, `points`, path data with the same commands, `viewBox`, transforms) are interpolated
//! number by number, colours channel by channel; other values change halfway, as a discrete
//! step does.
//!
//! Motion: along `path` or an `<mpath>`'s path, else through `values` (or `from`, `to`, `by`)
//! points; at one speed unless `calcMode` says otherwise, `keyPoints` placing it along the path
//! at the `keyTimes`; turned as `rotate` says (`auto`, `auto-reverse`, an angle). The move goes
//! before the element's own `transform`, in its parent's units, as browsers do.
//!
//! In a window, as in browsers: the pointer is over the topmost shape whose fill or stroke would
//! paint there within its clip paths and viewports (masks aside; text as its whole element),
//! unless `pointer-events` is `none`, and where none is, over the root `<svg>`; an event at what a
//! `<use>` brings bubbles through the `<use>`; a press gives focus to the nearest element that
//! takes it (a `tabindex`, a link, or focus events waited for); Tab and Shift+Tab go from one to
//! the next in Tab order (positive `tabindex`es first), leaving the drawing at its ends.
//!
//! Deliberate difference from Edge: a fixed `rotate` angle turns the element, as the spec says
//! and Firefox does (Edge ignores it). (Edge reads no `wallclock()` at all.)
use crate::color::Color;
use crate::el::El;
use crate::event::{Key, Mods};
use crate::geom::{Point, pt};
use crate::path::{Path, Step};
use crate::svg::{Drawing, File, ParsedDrawing, Track, color, number, numbers};
use crate::ui::{Response, Ui};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::time::Duration;

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

/// The longest span encoded as frames, in seconds: an animation that ends later, or runs for ever
/// with parts that meet only later, is cut there. (It bounds how often a file is drawn.)
const LONGEST: f32 = 60.0;

/// How far a schedule is worked out for frames to encode: far enough to find a cycle of
/// intervals of up to `LONGEST` that comes again and again.
const AHEAD: f32 = 3.0 * LONGEST;

/// How much work a schedule may do per minute worked out: so many moments taken (begins, ends,
/// repeats told) and told to the animations waiting on them, and so many more per animation.
/// Past them it stops: a chain of animations beginning each other at no distance would never
/// end, and one moment told to every animation of the file, again and again, would cost the
/// square of its length.
const MOMENTS: usize = 1024;
const MOMENTS_EACH: usize = 256;

/// How much more work each event in a window allows, per animation: a begin or end of each, and
/// a telling of each. (A pointer moving over a drawing makes an event a frame.)
const MOMENTS_PER_EVENT: usize = 2;

/// How often a drawing in a window is drawn anew while something in it moves.
const FRAME: Duration = Duration::from_micros(16_667);

/// The events a drawing in a window takes from the pointer (`click` comes with `activate` and
/// `DOMActivate`) and from focus moving (see `FOCUS_EVENTS`). Others never come.
const EVENTS: [&str; 14] = ["click", "activate", "DOMActivate", "mousedown", "mouseup", "mouseover", "mouseout", "mouseenter", "mouseleave", "mousemove", "focus", "blur", "focusin", "focusout"];

/// The events of focus moving within a drawing (see `AnimatedDrawing::move_focus`): `focus`
/// and `blur` at the element alone, `focusin` and `focusout` at the elements it lies in too.
const FOCUS_EVENTS: [&str; 4] = ["focus", "blur", "focusin", "focusout"];

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

/// Whether an animation may begin again: at any time, only once it ended, or never.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Restart {
    Always,
    WhenNotActive,
    Never,
}

/// A moment of an animation another may wait for: its begin, its end, or its nth repeat (0: any).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Moment {
    Begin,
    End,
    Repeat(u32),
}

/// What a `begin` or `end` entry waits for, each with its offset in seconds: a time from the
/// start; another animation's moment (by its element); an event at an element or anything in
/// it; or a key typed. Or a time by the wall clock, as written (see `wall_time`).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Condition<'a> {
    Clock(f32),
    Synced(usize, Moment, f32),
    Event(usize, &'a str, f32),
    Key(char, f32),
    Wall(&'a str),
}

/// When an animation runs: what begins and ends it, each cycle `simple` seconds long (none: no
/// end, and no change), repeated for `active` seconds from each begin (maybe infinite), each
/// interval's length bound by `min` and `max` (0 and infinite where `min` exceeds `max`), and
/// whether it may begin again.
struct Timing<'a> {
    begins: Vec<Condition<'a>>,
    ends: Vec<Condition<'a>>,
    simple: Option<f32>,
    active: f32,
    bounds: [f32; 2],
    restart: Restart,
}

/// How a moved element turns: by a fixed angle (degrees), along its path, or against it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Turn {
    Fixed(f32),
    Along,
    Against,
}

/// The path an `animateMotion` moves along (in its target's parent's units), where it starts,
/// and how the element turns.
struct Motion {
    track: Track,
    start: Point,
    turn: Turn,
}

/// One animation element as read.
struct Animation<'a> {
    /// Its own element; the element and attribute it animates (`motion` for `animateMotion`).
    element: usize,
    target: usize,
    name: &'a str,
    /// Its values, as written (for `animateTransform`, each as the transform it is; for
    /// `animateMotion`, how far along its path, 0 to 1).
    values: Vec<String>,
    /// When each value is reached, 0 to 1 (empty: evenly), and how to ease between (`Spline`).
    times: Vec<f32>,
    splines: Vec<[f32; 4]>,
    mode: Mode,
    timing: Timing<'a>,
    freeze: bool,
    /// Whether it adds to the value under it, and adds its last value once per cycle repeated.
    additive: bool,
    accumulate: bool,
    motion: Option<Motion>,
}

/// A file's animations, in document order; each element's parent (the root's is itself); and
/// who waits on each animation's moments: the animations whose `begin` or `end` lists name one
/// (the animation waiting, whether as its end, which moment, the offset).
pub(crate) struct Timeline<'a> {
    file: File<'a>,
    animations: Vec<Animation<'a>>,
    parents: Vec<usize>,
    waiting: Vec<Vec<(usize, bool, Moment, f32)>>,
}

/// Something that happened to a drawing in a window, at a time on its clock (seconds).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Fired {
    seconds: f32,
    what: Happened,
}

/// What happened: an event (by name) at elements (where it happened first, then those it
/// bubbles to); a key typed; or the app beginning or ending an animation (by its element).
#[derive(Clone, PartialEq, Debug)]
enum Happened {
    Event(&'static str, Vec<usize>),
    Key(char),
    Begun(usize),
    Ended(usize),
}

/// A time an event gave an animation (SMIL's instance time): seconds on the drawing's clock, the
/// animation (by its place in the timeline), and whether it is an end or a begin.
#[derive(Clone, Copy, PartialEq, Debug)]
struct InstanceTime {
    seconds: f32,
    animation: u32,
    end: bool,
}

/// When each animation of a timeline runs, as far as worked out: its intervals in order, each
/// from its begin to its end (maybe infinite); the time worked out to; whether anything was
/// still to happen at a later time; and which animations change while they run (those with a
/// cycle and two values or more).
pub(crate) struct Schedule {
    intervals: Vec<Vec<[f32; 2]>>,
    until: f32,
    unfinished: bool,
    changing: Vec<bool>,
}

impl<'a> Timeline<'a> {
    /// Reads a file's animations.
    pub(crate) fn read(text: &'a str) -> Timeline<'a> {
        let file = File::read(text);
        // Each element's parent, by a list of the elements open around it.
        let (mut animations, mut open, mut parents): (Vec<Animation>, Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new(), Vec::with_capacity(file.elements.len()));
        for at in 0..file.elements.len() {
            while open.last().is_some_and(|parent| file.elements[*parent].end <= at) {
                open.pop();
            }
            parents.push(open.last().copied().unwrap_or(at));
            animations.extend(animation(&file, at, open.last().copied()));
            open.push(at);
        }
        let waiting = waiting(&animations);
        Timeline { file, animations, parents, waiting }
    }

    /// The file's text with every animated attribute at its value `seconds` after it begins, as
    /// `schedule` runs its animations.
    pub(crate) fn frame(&self, schedule: &Schedule, seconds: f32) -> String {
        // The animations applying then, by the begin of their interval and order: those begun
        // later replace or add to those under them.
        let mut applying: Vec<(f32, usize, String)> = Vec::new();
        for (n, animation) in self.animations.iter().enumerate() {
            let Some((begin, value)) = schedule.intervals.get(n).and_then(|intervals| animation.value_at(intervals, seconds)) else { continue };
            applying.push((begin, n, value));
        }
        applying.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        // Each animated attribute's value, by element, and where each is in that list.
        let mut values: Vec<(usize, &str, String)> = Vec::new();
        let mut slots: HashMap<(usize, &str), usize> = HashMap::new();
        for (_, n, value) in applying {
            let animation = &self.animations[n];
            let (target, name) = (animation.target, animation.name);
            let Some(&under) = slots.get(&(target, name)) else {
                let value = if animation.additive { added(&base(&self.file, target, name), &value, name) } else { value };
                slots.insert((target, name), values.len());
                values.push((target, name, value));
                continue;
            };
            values[under].2 = if animation.additive { added(&values[under].2, &value, name) } else { value };
        }
        values.sort_by_key(|(at, ..)| *at);
        written(&self.file, &values)
    }

    /// The file's text as it is `seconds` after it begins, its timed animations alone running.
    pub(crate) fn frame_at(&self, seconds: f32) -> String {
        self.frame(&self.schedule(&[], seconds.max(0.0)), seconds)
    }

    /// When each animation runs up to `until` seconds, given what happened in a window: each
    /// moment (a begin, an end, a repeat told) is taken in time order, telling the animations
    /// waiting on it, as much as `MOMENTS`, `MOMENTS_EACH` and `MOMENTS_PER_EVENT` allow.
    /// (`wallclock()` times never come: a file drawn at a moment has no wall clock.)
    pub(crate) fn schedule(&self, fired: &[Fired], until: f32) -> Schedule {
        let mut scheduling = Scheduling::new(self, None);
        let given: Vec<InstanceTime> = fired.iter().flat_map(|event| self.instance_times(event)).collect();
        scheduling.hear(self, &given);
        let unfinished = scheduling.run(self, |time| time > until, self.budget(until, fired.len()));
        scheduling.into_schedule(self, until, unfinished)
    }

    /// The instance times an event gives: for each animation waiting for it, a begin or an end
    /// at its time plus the offset said (the app's `beginElement()`, at its time).
    fn instance_times(&self, event: &Fired) -> Vec<InstanceTime> {
        let mut given = Vec::new();
        for (n, animation) in self.animations.iter().enumerate() {
            for (list, end) in [(&animation.timing.begins, false), (&animation.timing.ends, true)] {
                let asked = by_app(&event.what, end) == Some(animation.element);
                let offsets = list.iter().filter_map(|condition| heard(condition, &event.what)).chain(asked.then_some(0.0));
                given.extend(offsets.map(|offset| InstanceTime { seconds: event.seconds + offset, animation: n as u32, end }));
            }
        }
        given
    }

    /// How much work a schedule worked out to `until` seconds, with `events` fired, may do (see
    /// `MOMENTS`).
    fn budget(&self, until: f32, events: usize) -> usize {
        let minutes = (until / 60.0).ceil().clamp(1.0, 1e6) as usize;
        let timed = (MOMENTS + MOMENTS_EACH * self.animations.len()).saturating_mul(minutes);
        timed.saturating_add(events.saturating_mul(MOMENTS_PER_EVENT * (self.animations.len() + 1)))
    }

    /// The span worth drawing as frames: where it starts and how long it is (seconds, at most
    /// `LONGEST`), and how often it plays (0 for ever). Where all ends, the time to its end,
    /// once. Where some runs for ever, one cycle of all that repeats, from when the rest is
    /// done (or when intervals come again and again, one cycle of them). Nothing without
    /// animations that begin by themselves. (`schedule`: worked out `AHEAD`.)
    pub(crate) fn span(&self, schedule: &Schedule) -> Option<(f32, f32, u32)> {
        let begun: Vec<usize> = (0..self.animations.len()).filter(|n| !schedule.intervals[*n].is_empty()).collect();
        if begun.is_empty() {
            return None;
        }
        // Those that repeat for ever once begun: their last interval's begin, and their cycle.
        let endless: Vec<(f32, f32)> = begun.iter().filter_map(|n| self.endless(&schedule.intervals[*n], *n)).collect();
        let cycles = |first: u64| endless.iter().map(|(_, simple)| (simple * 1000.0).round().max(1.0) as u64).fold(first, |all, one| (all / gcd(all, one)).saturating_mul(one));
        if !schedule.unfinished {
            // When all else is done: each interval's end, or the begin of one with no end.
            let finite = |[begin, end]: &[f32; 2]| if end.is_finite() { *end } else { *begin };
            let last = begun.iter().flat_map(|n| schedule.intervals[*n].iter().map(finite)).fold(0.0f32, f32::max);
            if endless.is_empty() {
                return Some((0.0, last.min(LONGEST), 1));
            }
            let start = endless.iter().map(|(begin, _)| *begin).fold(last, f32::max);
            return Some((start, (cycles(1) as f32 / 1000.0).min(LONGEST), 0));
        }
        let Some(period) = schedule.period() else { return Some((0.0, LONGEST, 1)) };
        let cycle = cycles((period * 1000.0).round().max(1.0) as u64);
        Some((schedule.until - 2.0 * period, (cycle as f32 / 1000.0).min(LONGEST), 0))
    }

    /// Where animation `n`'s last interval runs for ever and repeats: its begin, and its cycle.
    fn endless(&self, intervals: &[[f32; 2]], n: usize) -> Option<(f32, f32)> {
        let [begin, end] = *intervals.last()?;
        let simple = self.animations[n].timing.simple?;
        end.is_infinite().then_some((begin, simple))
    }

    /// What a window needs to know of the file between frames (see `Listening`).
    fn listening(&self) -> Listening {
        let file = &self.file;
        let conditions: Vec<Condition> = self.animations.iter().flat_map(|animation| animation.timing.begins.iter().chain(&animation.timing.ends)).copied().collect();
        let events = EVENTS.into_iter().filter(|name| conditions.iter().any(|condition| matches!(condition, Condition::Event(_, said, _) if said == name))).collect();
        let keys = conditions.iter().any(|condition| matches!(condition, Condition::Key(..)));
        // How far before an event, and before another animation's moment, an offset reaches.
        let behind = conditions.iter().filter_map(|condition| match *condition {
            Condition::Event(.., offset) | Condition::Key(_, offset) => Some(-offset),
            _ => None,
        });
        let synced = conditions.iter().filter_map(|condition| match *condition {
            Condition::Synced(.., offset) => Some(-offset),
            _ => None,
        });
        let count = file.elements.len();
        let pointer = (0..count).map(|at| file.own_property(at, "pointer-events").map(|said| said != "none")).collect();
        let brings = (0..count).map(|at| if file.elements[at].name == "use" { file.linked(at).unwrap_or(at) } else { at }).collect();
        let (focusable, stops) = self.focus_stops(&conditions);
        let root = file.elements.iter().position(|element| element.name == "svg");
        let (behind, reach) = (behind.fold(0.0, f32::max), synced.fold(0.0, f32::max) * self.animations.len() as f32);
        Listening { parents: self.parents.clone(), brings, pointer, root, events, keys, focusable, stops, behind, reach }
    }

    /// Which elements take focus, as in browsers: those with a `tabindex`, links, and those whose
    /// focus events `conditions` wait for; and those Tab stops at, in its order: the positive
    /// `tabindex`es, lowest first, then the rest whose `tabindex` is not negative, each in the
    /// file's order.
    fn focus_stops(&self, conditions: &[Condition]) -> (Vec<bool>, Vec<usize>) {
        let file = &self.file;
        let tab_index = |at: usize| file.attribute(at, "tabindex").and_then(|index| index.trim().parse::<i32>().ok());
        let link = |at: usize| file.elements[at].name == "a" && (file.attribute(at, "href").is_some() || file.attribute(at, "xlink:href").is_some());
        let mut focusable: Vec<bool> = (0..file.elements.len()).map(|at| tab_index(at).is_some() || link(at)).collect();
        for condition in conditions {
            if let Condition::Event(element, name, _) = condition {
                focusable[*element] |= FOCUS_EVENTS.contains(name);
            }
        }
        let order = |at: usize| tab_index(at).filter(|index| *index > 0).unwrap_or(i32::MAX);
        let mut stops: Vec<usize> = Vec::new();
        // (Each goes after those of its order or lower: a stable sort, with no sort's code.)
        for at in (0..focusable.len()).filter(|at| focusable[*at] && tab_index(*at).is_none_or(|index| index >= 0)) {
            stops.insert(stops.partition_point(|stop| order(*stop) <= order(at)), at);
        }
        (focusable, stops)
    }

    /// Whether any animation waits for animation `n` to repeat.
    fn repeats_heard(&self, n: usize) -> bool {
        self.waiting[n].iter().any(|(.., moment, _)| matches!(moment, Moment::Repeat(_)))
    }

    /// The animation element named `id`, if there is one.
    fn animation_named(&self, id: &str) -> Option<usize> {
        let element = *self.file.named.get(id)?;
        self.animations.iter().any(|animation| animation.element == element).then_some(element)
    }
}

impl Schedule {
    /// Whether an interval runs at `seconds` whose animation changes over time.
    fn moving(&self, seconds: f32) -> bool {
        let running = |intervals: &Vec<[f32; 2]>| intervals.iter().any(|[begin, end]| *begin <= seconds && seconds < *end);
        self.intervals.iter().zip(&self.changing).any(|(intervals, changing)| *changing && running(intervals))
    }

    /// The first time after `seconds` an interval begins or ends.
    fn next_change(&self, seconds: f32) -> Option<f32> {
        let after = self.intervals.iter().flatten().flat_map(|interval| interval.iter().copied()).filter(|time| *time > seconds && time.is_finite());
        after.min_by(f32::total_cmp)
    }

    /// The shortest time after which the intervals come again: one that repeats them twice
    /// before the end. Tried: the gaps between each animation's last begin and those before it.
    fn period(&self) -> Option<f32> {
        let mut gaps: Vec<f32> = Vec::new();
        for intervals in &self.intervals {
            let Some([last, _]) = intervals.last() else { continue };
            gaps.extend(intervals.iter().rev().skip(1).take(16).map(|[begin, _]| last - begin));
        }
        gaps.retain(|gap| *gap >= 0.001 && 3.0 * gap <= self.until);
        gaps.sort_by(f32::total_cmp);
        gaps.dedup_by(|a, b| (*a - *b).abs() < 0.001);
        gaps.into_iter().take(64).find(|gap| self.repeats_after(*gap))
    }

    /// Whether every animation's intervals within `period` from three periods before the end
    /// come again a period later.
    fn repeats_after(&self, period: f32) -> bool {
        let from = self.until - 3.0 * period;
        let within = |intervals: &Vec<[f32; 2]>, start: f32| -> Vec<[f32; 2]> {
            intervals.iter().filter(|[begin, end]| *begin < start + period && *end > start).map(|[begin, end]| [begin - start, end - start]).collect()
        };
        let same = |a: &[f32; 2], b: &[f32; 2]| a.iter().zip(b).all(|(a, b)| a == b || (a - b).abs() < 0.001);
        self.intervals.iter().all(|intervals| {
            let (first, then) = (within(intervals, from), within(intervals, from + period));
            first.len() == then.len() && first.iter().zip(&then).all(|(a, b)| same(a, b))
        })
    }
}

/// An animation's state while a schedule is worked out: the times it may begin and end, the
/// interval running (its begin, and how many repeats are told), when its last interval ended
/// (begins before then are past), and how often it changed (moments due before are stale).
#[derive(Clone)]
struct Clock {
    begins: Vec<f32>,
    ends: Vec<f32>,
    running: Option<(f32, u32)>,
    done: f32,
    version: u32,
}

/// What happens next to an animation.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Next {
    Begin,
    End,
    /// It ends and begins again at once.
    Restart,
    Repeat,
}

/// A moment due: when, to which animation, as its clock stood (`Clock::version`), and what.
#[derive(Clone)]
struct Due {
    time: f32,
    animation: usize,
    version: u32,
    next: Next,
}

impl PartialEq for Due {
    fn eq(&self, other: &Due) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Due {}

impl PartialOrd for Due {
    fn partial_cmp(&self, other: &Due) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Due {
    /// The earliest is the greatest, as a heap takes the greatest first; then by document order.
    fn cmp(&self, other: &Due) -> Ordering {
        other.time.total_cmp(&self.time).then(other.animation.cmp(&self.animation)).then(self.version.cmp(&other.version))
    }
}

/// A schedule being worked out (see `Timeline::schedule`): each animation's clock and
/// intervals so far; the moments due; and the work done (see `MOMENTS`). A window keeps one as
/// its drawing plays, taking in each event as it comes (see `AnimatedDrawing::look_ahead`).
#[derive(Clone)]
struct Scheduling {
    clocks: Vec<Clock>,
    intervals: Vec<Vec<[f32; 2]>>,
    due: BinaryHeap<Due>,
    work: usize,
}

impl Scheduling {
    /// The start: the times each animation's clock values give it (and its `wallclock()`
    /// values, where the wall clock's time at the start is `started`, in seconds since 1970),
    /// and the first moment of each due. Events come in by `hear`.
    fn new(timeline: &Timeline, started: Option<f64>) -> Scheduling {
        let count = timeline.animations.len();
        let mut clocks = Vec::with_capacity(count);
        for animation in &timeline.animations {
            let mut begins = timed(&animation.timing.begins, started);
            let mut ends = timed(&animation.timing.ends, started);
            begins.sort_by(f32::total_cmp);
            ends.sort_by(f32::total_cmp);
            clocks.push(Clock { begins, ends, running: None, done: f32::NEG_INFINITY, version: 0 });
        }
        let mut scheduling = Scheduling { clocks, intervals: vec![Vec::new(); count], due: BinaryHeap::new(), work: 0 };
        (0..count).for_each(|n| scheduling.plan(timeline, n));
        scheduling
    }

    /// Takes the moments due in time order until `past` says one is past or `most` work is
    /// done. Says whether a moment at a time was left (more is to happen).
    fn run(&mut self, timeline: &Timeline, past: impl Fn(f32) -> bool, most: usize) -> bool {
        while let Some(due) = self.due.peek() {
            if due.version != self.clocks[due.animation].version {
                self.due.pop();
                continue;
            }
            if past(due.time) || self.work >= most {
                return due.time.is_finite();
            }
            if let Some(due) = self.due.pop() {
                self.take(timeline, due);
            }
        }
        false
    }

    /// The schedule as worked out to `until` (see `Schedule`).
    fn into_schedule(self, timeline: &Timeline, until: f32, unfinished: bool) -> Schedule {
        let changing = timeline.animations.iter().map(|animation| animation.timing.simple.is_some() && animation.values.len() > 1).collect();
        Schedule { intervals: self.intervals, until, unfinished, changing }
    }

    /// Takes in instance times events gave, which plans each animation given one again. Where an
    /// end ends the interval running sooner, that interval's end is as it would have been with
    /// the end known from the start. Hearing a time twice changes nothing. (No moment at or
    /// after a time heard may be taken yet: the schedule is then the one worked out from the
    /// start with the time known.)
    fn hear(&mut self, timeline: &Timeline, given: &[InstanceTime]) {
        let mut heard = vec![false; self.clocks.len()];
        for instance in given {
            let (n, time) = (instance.animation as usize, instance.seconds);
            heard[n] = true;
            let clock = &mut self.clocks[n];
            if !instance.end {
                insert(&mut clock.begins, time);
                continue;
            }
            insert(&mut clock.ends, time);
            if let (Some((begin, _)), Some(last)) = (clock.running.filter(|(begin, _)| time > *begin), self.intervals[n].last_mut()) {
                let [least, most] = timeline.animations[n].timing.bounds;
                last[1] = last[1].min(begin + (time - begin).max(least).min(most));
            }
        }
        (0..heard.len()).filter(|n| heard[*n]).for_each(|n| self.plan(timeline, n));
    }

    /// Forgets what can no longer change anything from `settled` seconds on, every moment before
    /// then taken: each animation's intervals but the last, begin times it can no longer take,
    /// end times no interval still to come can end at, and stale moments due. An interval yet to
    /// come begins after the one before it (or with it, restarting it), and at a time an event,
    /// a moment still due or one it brings gives: no earlier than the first due, or `settled`,
    /// by `reach` (the most a chain of moments told with negative offsets can go back).
    fn forget(&mut self, timeline: &Timeline, settled: f32, reach: f32) {
        let clocks = &self.clocks;
        self.due.retain(|due| due.version == clocks[due.animation].version);
        let earliest = self.due.peek().map_or(settled, |due| due.time.min(settled)) - reach;
        for (n, (clock, intervals)) in self.clocks.iter_mut().zip(&mut self.intervals).enumerate() {
            intervals.drain(..intervals.len().saturating_sub(1));
            let (begins_from, ends_from) = match clock.running {
                Some((begin, _)) => (begin, begin),
                None => (clock.done, clock.done.max(earliest)),
            };
            clock.begins.drain(..clock.begins.partition_point(|time| *time < begins_from));
            clock.ends.drain(..clock.ends.partition_point(|time| *time <= ends_from));
            if timeline.animations[n].timing.restart == Restart::Never && !intervals.is_empty() {
                clock.begins.clear();
            }
        }
    }

    /// Takes a moment due.
    fn take(&mut self, timeline: &Timeline, due: Due) {
        self.work += 1;
        let n = due.animation;
        match due.next {
            Next::Begin => self.begin(timeline, n, due.time),
            Next::End => self.end(timeline, n, due.time),
            Next::Restart => {
                self.end(timeline, n, due.time);
                self.begin(timeline, n, due.time);
            }
            Next::Repeat => self.repeat(timeline, n, due.time),
        }
        self.plan(timeline, n);
    }

    /// Animation `n`'s next moment, made due (any due before is stale).
    fn plan(&mut self, timeline: &Timeline, n: usize) {
        self.clocks[n].version += 1;
        if let Some((time, next)) = self.next(timeline, n) {
            self.due.push(Due { time, animation: n, version: self.clocks[n].version, next });
        }
    }

    /// What happens next to animation `n`, and when: running, it ends, begins again or repeats;
    /// else it begins at its next begin time (unless it may never begin again).
    fn next(&self, timeline: &Timeline, n: usize) -> Option<(f32, Next)> {
        let (clock, timing) = (&self.clocks[n], &timeline.animations[n].timing);
        let Some((begin, told)) = clock.running else {
            let never = timing.restart == Restart::Never && !self.intervals[n].is_empty();
            let at = clock.begins.partition_point(|time| *time < clock.done);
            return clock.begins.get(at).filter(|_| !never).map(|time| (*time, Next::Begin));
        };
        let mut next = (self.end_of(timeline, n, begin).unwrap_or(begin), Next::End);
        let again = clock.begins.get(clock.begins.partition_point(|time| *time <= begin)).copied();
        if let Some(again) = again.filter(|again| timing.restart == Restart::Always && *again < next.0) {
            next = (again, Next::Restart);
        }
        let heard = timeline.repeats_heard(n);
        if let Some(repeat) = timing.simple.map(|simple| begin + simple * (told + 1) as f32).filter(|repeat| heard && *repeat < next.0) {
            next = (repeat, Next::Repeat);
        }
        Some(next)
    }

    /// Where an interval of animation `n` begun at `begin` ends: at its first end time after
    /// the begin, or when its active time is up, whichever comes first, then held within `min`
    /// and `max`. Nothing where its end list holds clock values only and none after the begin:
    /// it does not begin then (as SMIL says).
    fn end_of(&self, timeline: &Timeline, n: usize, begin: f32) -> Option<f32> {
        let (timing, ends) = (&timeline.animations[n].timing, &self.clocks[n].ends);
        let first = ends.get(ends.partition_point(|end| *end <= begin)).copied();
        let timed = !timing.ends.is_empty() && timing.ends.iter().all(|end| matches!(end, Condition::Clock(_)));
        let end = match first {
            Some(end) => end.min(begin + timing.active),
            None if timed => return None,
            None => begin + timing.active,
        };
        let [least, most] = timing.bounds;
        Some(begin + (end - begin).max(least).min(most))
    }

    /// Begins an interval of animation `n` at `time` (its begin times up to then are used).
    fn begin(&mut self, timeline: &Timeline, n: usize, time: f32) {
        let clock = &mut self.clocks[n];
        let used = clock.begins.partition_point(|begin| *begin <= time);
        clock.begins.drain(..used);
        let Some(end) = self.end_of(timeline, n, time) else { return };
        self.clocks[n].running = Some((time, 0));
        self.intervals[n].push([time, end]);
        self.tell(timeline, n, Moment::Begin, time);
    }

    /// Ends animation `n`'s interval at `time`.
    fn end(&mut self, timeline: &Timeline, n: usize, time: f32) {
        let clock = &mut self.clocks[n];
        let Some((begin, _)) = clock.running.take() else { return };
        clock.done = time.max(begin);
        if let Some(last) = self.intervals[n].last_mut() {
            last[1] = time.max(begin);
        }
        self.tell(timeline, n, Moment::End, time.max(begin));
    }

    /// Tells animation `n`'s next repeat, at `time`.
    fn repeat(&mut self, timeline: &Timeline, n: usize, time: f32) {
        let Some((_, told)) = &mut self.clocks[n].running else { return };
        *told += 1;
        let count = *told;
        self.tell(timeline, n, Moment::Repeat(count), time);
    }

    /// Gives the animations waiting on animation `n`'s moment their begin or end times.
    fn tell(&mut self, timeline: &Timeline, n: usize, moment: Moment, time: f32) {
        for at in 0..timeline.waiting[n].len() {
            let (waiting, as_end, heard, offset) = timeline.waiting[n][at];
            if heard == moment || (heard == Moment::Repeat(0) && matches!(moment, Moment::Repeat(_))) {
                self.work += 1;
                let list = if as_end { &mut self.clocks[waiting].ends } else { &mut self.clocks[waiting].begins };
                insert(list, time + offset);
                self.plan(timeline, waiting);
            }
        }
    }

    /// How many times, intervals and moments due it holds: what it costs to keep and copy.
    #[cfg(test)]
    fn held(&self) -> usize {
        let clocks: usize = self.clocks.iter().map(|clock| clock.begins.len() + clock.ends.len()).sum();
        clocks + self.intervals.iter().map(Vec::len).sum::<usize>() + self.due.len()
    }
}

/// Puts a time into a list of times in order, once: listed twice, it begins or ends nothing more
/// (and events while paused, all at one time, would pile up).
fn insert(list: &mut Vec<f32>, time: f32) {
    let at = list.partition_point(|other| *other <= time);
    if list[..at].last() != Some(&time) {
        list.insert(at, time);
    }
}

/// Who waits on each animation's moments (see `Timeline`): for each, the animations whose begin
/// or end lists name one of its moments.
fn waiting(animations: &[Animation]) -> Vec<Vec<(usize, bool, Moment, f32)>> {
    let by_element: HashMap<usize, usize> = animations.iter().enumerate().map(|(n, animation)| (animation.element, n)).collect();
    let mut waiting = vec![Vec::new(); animations.len()];
    for (n, animation) in animations.iter().enumerate() {
        let lists = [(&animation.timing.begins, false), (&animation.timing.ends, true)];
        for (condition, as_end) in lists.into_iter().flat_map(|(list, as_end)| list.iter().map(move |condition| (condition, as_end))) {
            let Condition::Synced(element, moment, offset) = *condition else { continue };
            if let Some(&waited) = by_element.get(&element) {
                waiting[waited].push((n, as_end, moment, offset));
            }
        }
    }
    waiting
}

/// The times a `begin` or `end` list gives by itself: its clock values, and its `wallclock()`
/// values where the wall clock's time at the start is known (`started`, in seconds since 1970).
fn timed(list: &[Condition], started: Option<f64>) -> Vec<f32> {
    let mut times = Vec::new();
    for condition in list {
        match condition {
            Condition::Clock(time) => times.push(*time),
            Condition::Wall(text) => times.extend(started.and_then(|started| Some((wall_time(text, started)? - started) as f32))),
            Condition::Synced(..) | Condition::Event(..) | Condition::Key(..) => {}
        }
    }
    times
}

/// A `wallclock()` value in seconds since 1970: a date (`2026-11-26`), a time (`14:30`,
/// `14:30:15.5`: that time on the day of `started`) or both joined by `T`, the time in local
/// time unless a zone follows it (`Z`, `+01:00`, `-0500`). Nothing for anything else.
fn wall_time(text: &str, started: f64) -> Option<f64> {
    let text = text.trim();
    let (date, time) = match text.split_once('T') {
        Some((date, time)) => (Some(date), Some(time)),
        None if text.contains(':') => (None, Some(text)),
        None => (Some(text), None),
    };
    let (of_day, zone) = time.map_or(Some((0.0, None)), time_of_day)?;
    let day = match date {
        Some(date) => {
            let parts: Vec<i32> = date.split('-').map(|part| part.parse().ok()).collect::<Option<Vec<i32>>>()?;
            let [year, month @ 1..=12, day @ 1..=31] = parts[..] else { return None };
            crate::date::Date::new(year, month as u8, day as u8).days()
        }
        None => ((started + crate::date::ahead_of_greenwich(started as i64) as f64) / 86_400.0).floor() as i64,
    };
    let seconds = day as f64 * 86_400.0 + of_day;
    Some(seconds - zone.unwrap_or_else(|| crate::date::ahead_of_greenwich(seconds as i64) as f64))
}

/// A wall-clock time of day (`hh:mm`, `hh:mm:ss`, `hh:mm:ss.f`) in seconds, and its zone's
/// offset from UTC in seconds where it says one (`Z`, `+hh:mm`, `-hhmm`).
fn time_of_day(text: &str) -> Option<(f64, Option<f64>)> {
    let (clock, zone) = match text.find(['Z', '+', '-']) {
        Some(at) => (&text[..at], Some(&text[at..])),
        None => (text, None),
    };
    let parts: Vec<f64> = clock.split(':').map(|part| part.parse::<f64>().ok().filter(|part| part.is_finite())).collect::<Option<Vec<f64>>>()?;
    let ([hours, minutes] | [hours, minutes, _]) = parts[..] else { return None };
    let of_day = hours * 3600.0 + minutes * 60.0 + parts.get(2).copied().unwrap_or(0.0);
    let zone = match zone {
        None => None,
        Some("Z") => Some(0.0),
        Some(zone) => {
            let digits = zone[1..].replace(':', "");
            let (hours, minutes) = (digits.get(..2)?.parse::<f64>().ok()?, digits.get(2..)?.parse::<f64>().unwrap_or(0.0));
            Some(if zone.starts_with('-') { -1.0 } else { 1.0 } * (hours * 3600.0 + minutes * 60.0))
        }
    };
    Some((of_day, zone))
}

/// The animation element the app began (`end` false) or ended (`end` true), where that is what
/// happened.
fn by_app(what: &Happened, end: bool) -> Option<usize> {
    match (what, end) {
        (Happened::Begun(element), false) | (Happened::Ended(element), true) => Some(*element),
        _ => None,
    }
}

/// Where a condition waits for what happened: its offset.
fn heard(condition: &Condition, what: &Happened) -> Option<f32> {
    match (condition, what) {
        (Condition::Event(element, name, offset), Happened::Event(happened, chain)) if name == happened && chain.contains(element) => Some(*offset),
        (Condition::Key(key, offset), Happened::Key(typed)) if key == typed => Some(*offset),
        _ => None,
    }
}

impl Animation<'_> {
    /// Its value at `seconds` as its intervals run it, with the begin of the interval it is
    /// from: running, or frozen at the end of the last before. Nothing where it does not apply.
    /// (Past its repeats, in an interval `min` made longer, it is frozen or gone as after its
    /// end, as Edge has it.)
    fn value_at(&self, intervals: &[[f32; 2]], seconds: f32) -> Option<(f32, String)> {
        let [begin, end] = *intervals.get(intervals.partition_point(|[begin, _]| *begin <= seconds).checked_sub(1)?)?;
        let active = (end - begin).min(self.timing.active);
        if seconds - begin >= active && !self.freeze {
            return None;
        }
        Some((begin, self.at(seconds - begin, active)?))
    }

    /// Its value `local` seconds into an interval `active` seconds long (with its accumulated
    /// sum, where it says so).
    fn at(&self, local: f32, active: f32) -> Option<String> {
        if local.is_nan() || local < 0.0 {
            return None;
        }
        let Some(simple) = self.timing.simple else { return Some(self.shown(self.values.first()?, 0.0)) };
        // Frozen at the end of its active time: the end of a cycle where it ends with one.
        let local = local.min(active);
        let (mut cycle, mut within) = ((local / simple).floor(), local % simple / simple);
        if local >= active && within == 0.0 && cycle > 0.0 {
            (cycle, within) = (cycle - 1.0, 1.0);
        }
        let value = self.within(within);
        let cycles = if self.accumulate { cycle } else { 0.0 };
        if self.motion.is_some() {
            return Some(self.shown(&value, cycles));
        }
        match self.values.last() {
            Some(last) if cycles >= 1.0 => Some(added(&value, &scaled(last, cycles), self.name)),
            _ => Some(value),
        }
    }

    /// A value as written into the file: for motion, the transform its place along the path
    /// makes (`cycles` paths further on); others as they are.
    fn shown(&self, value: &str, cycles: f32) -> String {
        match &self.motion {
            Some(motion) => motion.transform(number(value).map_or(0.0, |(share, _)| share), cycles),
            None => value.to_string(),
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
        // (A step of no time, as at key times "0;1;1", is at its end.)
        let gap = times[step + 1] - times[step];
        let local = if gap > 0.0 { ((share - times[step]) / gap).clamp(0.0, 1.0) } else { 1.0 };
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

impl Motion {
    /// The transform putting an element `share` (0 to 1) of the way along the path, `cycles`
    /// whole paths further on, turned as it says.
    fn transform(&self, share: f32, cycles: f32) -> String {
        let (point, [cos, sin]) = self.track.point_at(share.clamp(0.0, 1.0) * self.track.length).unwrap_or((self.start, [1.0, 0.0]));
        let end = self.track.point_at(self.track.length).map_or(self.start, |(end, _)| end);
        let [x, y] = [point.x + (end.x - self.start.x) * cycles, point.y + (end.y - self.start.y) * cycles];
        let angle = match self.turn {
            Turn::Fixed(degrees) => degrees,
            Turn::Along => sin.atan2(cos).to_degrees(),
            Turn::Against => sin.atan2(cos).to_degrees() + 180.0,
        };
        format!("translate({x} {y}) rotate({angle})")
    }
}

/// The animation element `at` is, if it is one: its target (the element it references, else
/// its `parent`), attribute, values and timing. Nothing for another element, or one with no
/// attribute or values to animate.
fn animation<'a>(file: &File<'a>, at: usize, parent: Option<usize>) -> Option<Animation<'a>> {
    let kind = file.elements[at].name;
    if !matches!(kind, "animate" | "set" | "animateTransform" | "animateColor" | "animateMotion") {
        return None;
    }
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    let target = file.linked(at).or(parent)?;
    let turn = (kind == "animateTransform").then(|| attr("type").unwrap_or("translate"));
    let (name, mut values, motion) = match kind {
        "animateMotion" => {
            let (motion, values) = motion(file, at)?;
            ("motion", values, Some(motion))
        }
        _ => {
            let name = attr("attributeName").or(turn.map(|_| "transform"))?;
            (name, values(file, (at, target, name), kind == "set")?, None)
        }
    };
    if let Some(turn) = turn {
        values = values.iter().map(|value| transform_value(turn, value)).collect();
    }
    let times = attr("keyTimes").map(|text| text.split(';').filter_map(|time| number(time).map(|(time, _)| time)).collect()).unwrap_or_default();
    let splines: Vec<[f32; 4]> = attr("keySplines").map(|text| text.split(';').filter_map(|spline| numbers(spline).collect::<Vec<f32>>().try_into().ok()).collect()).unwrap_or_default();
    let mode = match attr("calcMode") {
        _ if kind == "set" => Mode::Discrete,
        Some("discrete") => Mode::Discrete,
        Some("paced") => Mode::Paced,
        Some("spline") => Mode::Spline,
        Some("linear") => Mode::Linear,
        // A motion goes at one speed unless told otherwise.
        _ if motion.is_some() => Mode::Paced,
        _ => Mode::Linear,
    };
    // `by` alone adds to what is under it.
    let by_alone = attr("values").is_none() && attr("from").is_none() && attr("to").is_none() && attr("by").is_some();
    let additive = attr("additive") == Some("sum") || by_alone;
    Some(Animation {
        element: at,
        target,
        name,
        values,
        times,
        splines,
        mode,
        timing: timing(file, at, target),
        freeze: attr("fill") == Some("freeze"),
        additive,
        accumulate: attr("accumulate") == Some("sum"),
        motion,
    })
}

/// An `animateMotion`'s path and how it turns, with its values: how far along the path each
/// is (its `keyPoints`, else its ends, or for a path through points, each point's). Its path is
/// an `<mpath>`'s, else its own, else through its points. Nothing without one.
fn motion(file: &File, at: usize) -> Option<(Motion, Vec<String>)> {
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    let turn = match attr("rotate") {
        Some("auto") => Turn::Along,
        Some("auto-reverse") => Turn::Against,
        other => Turn::Fixed(other.and_then(number).map_or(0.0, |(degrees, _)| degrees)),
    };
    let referenced = file.inside(at).find(|child| file.elements[*child].name == "mpath").and_then(|child| file.linked(child)).and_then(|path| file.attribute(path, "d"));
    let (path, corners) = match referenced.or(attr("path")) {
        Some(d) => (Path::svg(d), None),
        None => {
            let points = motion_points(file, at)?;
            (through(&points), Some(along(&points)))
        }
    };
    let start = match path.steps().first() {
        Some(Step::Move(start)) => *start,
        _ => pt(0.0, 0.0),
    };
    let values: Vec<String> = match (attr("keyPoints"), corners) {
        (Some(points), _) => points.split(';').map(str::trim).filter(|point| !point.is_empty()).map(String::from).collect(),
        (None, Some(corners)) => corners.iter().map(f32::to_string).collect(),
        (None, None) => vec!["0".to_string(), "1".to_string()],
    };
    Some((Motion { track: Track::of(&path), start, turn }, values))
}

/// The points a motion goes through: its `values`, else `from`, `to` and `by` (from the origin
/// where `from` is not given). Nothing where they give none, or one is no point.
fn motion_points(file: &File, at: usize) -> Option<Vec<Point>> {
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    let point = |text: &str| {
        let both: Vec<f32> = numbers(text).take(2).collect();
        (both.len() == 2).then(|| pt(both[0], both[1]))
    };
    if let Some(values) = attr("values") {
        return values.split(';').filter(|value| !value.trim().is_empty()).map(point).collect::<Option<Vec<Point>>>().filter(|points| !points.is_empty());
    }
    let origin = pt(0.0, 0.0);
    match (attr("from").and_then(point), attr("to").and_then(point), attr("by").and_then(point)) {
        (Some(from), Some(to), _) => Some(vec![from, to]),
        (from, None, Some(by)) => {
            let from = from.unwrap_or(origin);
            Some(vec![from, pt(from.x + by.x, from.y + by.y)])
        }
        (None, Some(to), _) => Some(vec![origin, to]),
        _ => None,
    }
}

/// A path through points in turn.
fn through(points: &[Point]) -> Path {
    let mut path = Path::new();
    for (n, point) in points.iter().enumerate() {
        match n {
            0 => path.move_to(point.x, point.y),
            _ => path.line_to(point.x, point.y),
        };
    }
    path
}

/// How far along a path through points each is, 0 to 1 (all at 0 where it has no length).
fn along(points: &[Point]) -> Vec<f32> {
    let mut gone = vec![0.0];
    for pair in points.windows(2) {
        gone.push(gone[gone.len() - 1] + (pair[1].x - pair[0].x).hypot(pair[1].y - pair[0].y));
    }
    let whole = gone[gone.len() - 1];
    gone.iter().map(|at| if whole > 0.0 { at / whole } else { 0.0 }).collect()
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
/// (numbers from zero, colours from black, no motion).
fn base(file: &File, at: usize, name: &str) -> String {
    if name == "motion" {
        return String::new();
    }
    file.own_property(at, name).unwrap_or(if name.contains("color") || name == "fill" || name == "stroke" { "black" } else { "0" }).to_string()
}

/// A transform of a type with its numbers as `animateTransform` gives them: those left out
/// filled in (a translation's y as 0, a scale's y as its x, a rotation's centre as the origin).
fn transform_value(turn: &str, value: &str) -> String {
    let numbers: Vec<f32> = numbers(value).collect();
    let all: Vec<f32> = match turn {
        "scale" => vec![numbers.first().copied().unwrap_or(1.0), numbers.get(1).or(numbers.first()).copied().unwrap_or(1.0)],
        "rotate" => (0..3).map(|n| numbers.get(n).copied().unwrap_or(0.0)).collect(),
        "skewX" | "skewY" => vec![numbers.first().copied().unwrap_or(0.0)],
        _ => (0..2).map(|n| numbers.get(n).copied().unwrap_or(0.0)).collect(),
    };
    format!("{turn}({})", all.iter().map(f32::to_string).collect::<Vec<String>>().join(" "))
}

/// When an animation runs (see `Timing`): without `dur` (as a `set` often is), it holds its
/// value as long as nothing ends it. No `begin` begins it at the start; `min` and `max` bound
/// its active time (ignored where `min` exceeds `max`).
fn timing<'a>(file: &File<'a>, at: usize, target: usize) -> Timing<'a> {
    let attr = |name: &str| file.attribute(at, name).map(str::trim);
    let begins = match attr("begin") {
        None => vec![Condition::Clock(0.0)],
        Some(list) => conditions(file, list, target),
    };
    let ends = attr("end").map(|list| conditions(file, list, target)).unwrap_or_default();
    let simple = attr("dur").and_then(clock).filter(|simple| *simple > 0.0);
    let repeats = attr("repeatCount").and_then(|count| if count == "indefinite" { Some(f32::INFINITY) } else { number(count).map(|(count, _)| count).filter(|count| *count > 0.0) });
    let repeated = attr("repeatDur").and_then(|span| if span == "indefinite" { Some(f32::INFINITY) } else { clock(span) });
    let active = match (simple, repeats, repeated) {
        (None, _, Some(span)) => span,
        (None, ..) => f32::INFINITY,
        (Some(simple), None, None) => simple,
        (Some(simple), count, span) => (simple * count.unwrap_or(f32::INFINITY)).min(span.unwrap_or(f32::INFINITY)),
    };
    let bounds = [attr("min").and_then(clock).unwrap_or(0.0), attr("max").and_then(clock).unwrap_or(f32::INFINITY)];
    let bounds = if bounds[0] <= bounds[1] { bounds } else { [0.0, f32::INFINITY] };
    let restart = match attr("restart") {
        Some("never") => Restart::Never,
        Some("whenNotActive") => Restart::WhenNotActive,
        _ => Restart::Always,
    };
    Timing { begins, ends, simple, active: active.max(0.0), bounds, restart }
}

/// A `begin` or `end` list's entries (see `condition`), those it cannot wait for left out.
fn conditions<'a>(file: &File<'a>, list: &'a str, target: usize) -> Vec<Condition<'a>> {
    list.split(';').filter_map(|entry| condition(file, entry, target)).collect()
}

/// One entry of a `begin` or `end` list: a clock value; `wallclock(…)`; `accessKey(x)`; or
/// `id.what` or `what` (at `target`), `what` being `begin`, `end`, `repeat(n)`, `beginEvent`,
/// `endEvent`, `repeatEvent` or an event; each of the last two with an optional offset (`+1s`,
/// `- 0.5s`). Nothing for `indefinite`, unknown events and ids naming nothing.
fn condition<'a>(file: &File<'a>, entry: &'a str, target: usize) -> Option<Condition<'a>> {
    let entry = entry.trim();
    if let Some(time) = clock(entry) {
        return Some(Condition::Clock(time));
    }
    if let Some(inside) = entry.strip_prefix("wallclock(").and_then(|rest| rest.strip_suffix(')')) {
        return Some(Condition::Wall(inside));
    }
    if let Some(rest) = entry.strip_prefix("accessKey(") {
        let key = rest.chars().next()?;
        return Some(Condition::Key(key, offset(rest[key.len_utf8()..].strip_prefix(')')?)?));
    }
    // An id before a point names the element; else the point is part of an offset.
    let named = entry.split_once('.').and_then(|(id, rest)| Some((*file.named.get(id.trim())?, rest)));
    let (element, rest) = named.unwrap_or((target, entry));
    let long = rest.find(|c: char| c.is_whitespace() || c == '+' || c == '-').unwrap_or(rest.len());
    let (what, offset) = (&rest[..long], offset(&rest[long..])?);
    let synced = |moment: Moment| Some(Condition::Synced(element, moment, offset));
    match what {
        "begin" | "beginEvent" => synced(Moment::Begin),
        "end" | "endEvent" => synced(Moment::End),
        "repeatEvent" => synced(Moment::Repeat(0)),
        _ if what.starts_with("repeat(") => synced(Moment::Repeat(what["repeat(".len()..].strip_suffix(')')?.trim().parse().ok().filter(|count| *count > 0)?)),
        _ => EVENTS.contains(&what).then_some(Condition::Event(element, what, offset)),
    }
}

/// An offset after an entry: nothing (none), or a sign and a clock value.
fn offset(text: &str) -> Option<f32> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.0);
    }
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, text.strip_prefix('+')?),
    };
    clock(rest).map(|time| sign * time)
}

/// A SMIL clock value in seconds: `h:mm:ss.f`, `mm:ss.f`, or a number with `h`, `min`, `s`
/// or `ms` (seconds by default). Nothing for anything else (an event, `indefinite`).
fn clock(text: &str) -> Option<f32> {
    let text = text.trim().trim_start_matches('+');
    if text.contains(':') {
        let parts: Vec<f32> = text.split(':').map(|part| part.trim().parse::<f32>().ok().filter(|part| part.is_finite())).collect::<Option<Vec<f32>>>()?;
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

/// The number `text` starts with, and the rest, where it starts with one (a sign at most before
/// its digits).
fn number_here(text: &str) -> Option<(f32, &str)> {
    let digits = text.strip_prefix(['-', '+']).unwrap_or(text);
    digits.starts_with(|c: char| c.is_ascii_digit() || c == '.').then(|| number(text)).flatten()
}

/// A value's numbers, and the rest of it with whitespace and commas left out: two values of one
/// kind have the same rest. (Colours are taken apart by `channels`.)
fn parts(text: &str) -> (Vec<f32>, String) {
    let (mut numbers, mut rest, mut at) = (Vec::new(), String::new(), 0);
    while at < text.len() {
        let here = &text[at..];
        if let Some((number, after)) = number_here(here) {
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
        if let Some((_, after)) = number_here(here) {
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
    if name == "transform" || name == "motion" {
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

/// A file's text with attributes set to values (each by element and name, in element order):
/// the properties at the end of their element's `style`, where they win over style rules and
/// attributes; any other attribute in place, or added after its element's name.
fn written(file: &File, values: &[(usize, &str, String)]) -> String {
    // Each change: where in the text, how many bytes it replaces, and with what. (Quotes in a
    // value would end it: they go.)
    let mut changes: Vec<(usize, usize, String)> = Vec::new();
    for element in values.chunk_by(|a, b| a.0 == b.0) {
        let at = element[0].0;
        let named_end = file.place_of(file.elements[at].name)[1] as usize;
        let mine = with_motion(file, at, element).into_iter().map(|(name, value)| (name, value.replace(['"', '\''], "")));
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
    // In one pass from the start. (Additions at one place may go in any order.)
    changes.sort_by_key(|(place, ..)| *place);
    let (mut text, mut done) = (String::with_capacity(file.text.len() + changes.iter().map(|(.., with)| with.len()).sum::<usize>()), 0);
    for (place, long, with) in changes {
        text += file.text.get(done..place).unwrap_or("");
        text += &with;
        done = done.max(place + long);
    }
    text + file.text.get(done..).unwrap_or("")
}

/// An element's animated values (`element`: all of one element's), its motion put before its
/// transform (the animated one, else its own) as one `transform`.
fn with_motion<'v>(file: &File, at: usize, element: &'v [(usize, &'v str, String)]) -> Values<'v> {
    let mut mine: Values = element.iter().map(|(_, name, value)| (*name, value.clone())).collect();
    let Some(moved) = mine.iter().position(|(name, _)| *name == "motion") else { return mine };
    let (_, motion) = mine.remove(moved);
    match mine.iter_mut().find(|(name, _)| *name == "transform") {
        Some((_, transform)) => *transform = format!("{motion} {transform}"),
        None => mine.push(("transform", format!("{motion} {}", file.attribute(at, "transform").unwrap_or("")))),
    }
    mine
}

/// The SMIL animation of an SVG file as an animated PNG, `width` by `height` pixels (see
/// `svg::encode_animation`).
pub(crate) fn encode(svg: &str, width: usize, height: usize) -> Option<Vec<u8>> {
    let timeline = Timeline::read(svg);
    let schedule = timeline.schedule(&[], AHEAD);
    let (start, long, plays) = timeline.span(&schedule)?;
    // A one-time animation ends on a frame of its own, shown from then on.
    let ends = plays == 1;
    let most = (PIXELS / width.saturating_mul(height).max(1)).max(1);
    let frames = ((long * FRAMES).ceil() as usize + ends as usize).clamp(1, most);
    let steps = frames - ends as usize;
    let shows = ((long / steps.max(1) as f32 * 1000.0).round() as u16).max(1);
    let mut drawn = Vec::with_capacity(frames);
    for n in 0..frames {
        let seconds = start + long * n as f32 / steps.max(1) as f32;
        let (_, pixels) = crate::svg::decode(&timeline.frame(&schedule, seconds), width, height)?;
        drawn.push((pixels, shows));
    }
    crate::png::encode_animation(width, &drawn, plays)
}

/// Numbers each animated drawing, for its box's key.
static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many events wait for a drawing's timeline at most: with as many, it takes them in at
/// once, not at its next frame.
const PENDING_MOST: usize = 4096;

/// How many copies of its timeline a drawing in a window keeps for a seek back at most (see
/// `Checkpoints`), and how much work its timeline does between two at first (see `MOMENTS`).
const CHECKPOINTS: usize = 32;
const CHECKPOINT_WORK: usize = MOMENTS;

/// Copies of a drawing's timeline as it stood at earlier times, each with every moment before
/// its time taken (seconds), oldest first: a seek back goes on from the latest before it, not
/// from the start. One is kept each time the timeline did `every` more work; with more than
/// `CHECKPOINTS`, every other one goes and `every` doubles. So a seek back takes at most a
/// thirty-second of the work done since the start.
#[derive(Clone)]
struct Checkpoints {
    kept: Vec<(f32, Scheduling)>,
    every: usize,
}

impl Checkpoints {
    fn new() -> Checkpoints {
        Checkpoints { kept: Vec::new(), every: CHECKPOINT_WORK }
    }

    /// Keeps a copy of the timeline `playing`, every moment before `settled` taken, where it
    /// did `every` more work since the last copy.
    fn keep(&mut self, settled: f32, playing: &Scheduling) {
        let last = self.kept.last().map_or(0, |(_, kept)| kept.work);
        if playing.work < last.saturating_add(self.every) {
            return;
        }
        self.kept.push((settled, playing.clone()));
        if self.kept.len() > CHECKPOINTS {
            // Every other one, the latest kept.
            let first = (self.kept.len() - 1) % 2;
            self.kept = std::mem::take(&mut self.kept).into_iter().skip(first).step_by(2).collect();
            self.every = self.every.saturating_mul(2);
        }
    }

    /// Forgets the copies of times after `seconds`: an instance time then came that they never
    /// took in.
    fn forget_after(&mut self, seconds: f32) {
        self.kept.truncate(self.kept.partition_point(|(at, _)| *at <= seconds));
    }

    /// The latest copy of a time at or before `settled`, with that time.
    fn latest(&self, settled: f32) -> Option<&(f32, Scheduling)> {
        self.kept[..self.kept.partition_point(|(at, _)| *at <= settled)].last()
    }
}

/// Puts an instance time into a list in time order (then by animation, begins first), once.
fn add_instance(list: &mut Vec<InstanceTime>, instance: InstanceTime) {
    let order = |other: &InstanceTime| other.seconds.total_cmp(&instance.seconds).then(other.animation.cmp(&instance.animation)).then(other.end.cmp(&instance.end));
    let at = list.partition_point(|other| order(other) == Ordering::Less);
    if list.get(at) != Some(&instance) {
        list.insert(at, instance);
    }
}

/// The elements an event at `element` comes to, each once: it and those it lies in up to the
/// element the innermost `<use>` that brought it brings (`uses`, innermost first; `brings`:
/// what each element brings, itself if no `<use>`), then that `<use>` and those it lies in, and
/// so on (`parents`: each element's parent, the root's being itself). As in browsers, what a
/// `<use>` brings bubbles through the `<use>`, not through where it lies in the file.
fn bubbling((parents, brings): (&[usize], &[usize]), element: usize, uses: &[u32]) -> Vec<usize> {
    let (mut chain, mut at, mut users) = (Vec::new(), element, uses.iter().map(|user| *user as usize).peekable());
    while at < parents.len() && !chain.contains(&at) {
        chain.push(at);
        let brought_here = users.peek().is_some_and(|user| brings.get(*user) == Some(&at));
        at = match users.next_if(|_| brought_here || parents[at] == at) {
            Some(user) => user,
            None if parents[at] == at => usize::MAX,
            None => parents[at],
        };
    }
    chain
}

/// What a drawing in a window needs to know of its file between frames: each element's parent
/// and what it brings, where a `<use>` (see `bubbling`); whether it says it takes the pointer
/// (`pointer-events`: `none`, another value, or nothing said); the root `<svg>`, which has the
/// pointer where no shape is; the events and keys its animations wait for; which elements take
/// focus (those with a `tabindex`, links, and those whose focus events it waits for, as in
/// browsers) and those Tab stops at, in its order; how far before an event its offsets reach
/// (seconds); and how far before a moment a chain of moments told with negative offsets can
/// reach (see `Scheduling::forget`).
struct Listening {
    parents: Vec<usize>,
    brings: Vec<usize>,
    pointer: Vec<Option<bool>>,
    root: Option<usize>,
    events: Vec<&'static str>,
    keys: bool,
    focusable: Vec<bool>,
    stops: Vec<usize>,
    behind: f32,
    reach: f32,
}

impl Listening {
    /// Whether anything waits for the pointer to cross an element, so that every move counts.
    fn follows(&self) -> bool {
        self.events.iter().any(|name| matches!(*name, "mouseover" | "mouseout" | "mouseenter" | "mouseleave" | "mousemove"))
    }

    /// Whether anything waits for focus to move within the drawing.
    fn follows_focus(&self) -> bool {
        self.events.iter().any(|name| FOCUS_EVENTS.contains(name))
    }

    /// The elements an event at `element` (brought by `uses`) comes to (see `bubbling`).
    fn chain(&self, element: usize, uses: &[u32]) -> Vec<usize> {
        bubbling((&self.parents, &self.brings), element, uses)
    }

    /// Whether a shape of `element` (brought by `uses`) takes the pointer: as the nearest of
    /// the elements an event there comes to that says.
    fn takes_pointer(&self, element: usize, uses: &[u32]) -> bool {
        self.chain(element, uses).iter().find_map(|at| self.pointer.get(*at).copied().flatten()) != Some(false)
    }

    /// Where focus goes from a press over elements (`under`, as an event there comes to them):
    /// to the nearest that takes focus, as an event there comes to it. Nowhere where none does.
    fn focus_under(&self, under: &[usize]) -> Vec<usize> {
        under.iter().position(|at| self.focusable[*at]).map_or_else(Vec::new, |at| under[at..].to_vec())
    }

    /// Where Tab coming into the drawing goes: to its first stop (Shift+Tab, `back`: its last).
    fn entry_stop(&self, back: bool) -> Option<usize> {
        if back { self.stops.last() } else { self.stops.first() }.copied()
    }

    /// Where Tab (Shift+Tab: `back`) goes from element `from`, as in Edge: from a stop, on in Tab
    /// order; from another element, to the nearest stop that way in the file; from none (a press
    /// where nothing takes focus), as from before the first element. None past the last.
    fn next_stop(&self, from: Option<usize>, back: bool) -> Option<usize> {
        if let Some(n) = from.and_then(|at| self.stops.iter().position(|stop| *stop == at)) {
            return if back { n.checked_sub(1).map(|n| self.stops[n]) } else { self.stops.get(n + 1).copied() };
        }
        let stops = self.stops.iter().copied();
        match (from, back) {
            (Some(at), true) => stops.filter(|stop| *stop < at).max(),
            (Some(at), false) => stops.filter(|stop| *stop > at).min(),
            (None, true) => None,
            (None, false) => stops.min(),
        }
    }

    /// The elements `focus` and `blur` at the first of `chain` come to: it, and each `<use>`
    /// that brought it.
    fn focus_chain(&self, chain: &[usize]) -> Vec<usize> {
        chain.iter().enumerate().filter(|(n, at)| *n == 0 || self.brings[**at] != **at).map(|(_, at)| *at).collect()
    }
}

/// An SVG file playing its SMIL animation in a window ([`Ui::animated_drawing`]): what is
/// timed, what waits for the pointer and keys over its box (`click`, `mouseover`,
/// `accessKey(x)` and the other events) and for focus moving within it (`focusin`), or for the
/// app ([`begin_element`](Self::begin_element)). Its clock runs while it shows; `wallclock()`
/// times count from when it first shows. Each frame is the file at that moment, read and
/// drawn as any drawing is (see [the module](crate::svg)); only its box is redrawn, and only
/// while something in it changes. Its timeline goes on from where it stood, each event added
/// to it: an event costs the same however long the drawing has played, and none is
/// forgotten. A seek back is exact: it goes on from the latest of up to 32 copies of its
/// timeline kept as it played. It keeps each time an event gave an animation (12 bytes;
/// events at one clock time give it once). [`Ui::drop_animated_drawing`] frees its last frame.
pub struct AnimatedDrawing {
    text: std::sync::Arc<str>,
    serial: u64,
    listening: Listening,
    /// The instance times events gave its animations, in time order (see `add_instance`):
    /// all, for a seek back; the events its timeline has not taken in yet; and how many events
    /// ever came.
    instances: Vec<InstanceTime>,
    pending: Vec<Fired>,
    heard: usize,
    /// Its clock (seconds), the window's time it last ran at, whether it stands still, and the
    /// wall clock's time when its clock stood at 0 (seconds since 1970), once it showed.
    seconds: f32,
    last: Option<Duration>,
    paused: bool,
    started: Option<f64>,
    /// Its timeline as it plays: every moment before `settled` seconds taken (see
    /// `look_ahead`); and copies of it at earlier times, for a seek back.
    playing: Option<Scheduling>,
    settled: f32,
    checkpoints: Checkpoints,
    /// The schedule last worked out from it, how much had happened then, and the clock then.
    schedule: Option<(Schedule, usize, f32)>,
    /// The frame on show: its text, when it is (seconds, and how much had happened), and its
    /// drawing in the window.
    shown: String,
    shown_for: Option<(f32, usize)>,
    drawing: Option<Drawing>,
    /// The elements under the pointer, as an event there comes to them.
    under: Vec<usize>,
    /// The element with focus, as an event there comes to it (none: no element has it); and
    /// whether the box had focus at the last run.
    focused: Vec<usize>,
    had_focus: bool,
}

impl std::fmt::Debug for AnimatedDrawing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnimatedDrawing").field("seconds", &self.seconds).field("paused", &self.paused).finish_non_exhaustive()
    }
}

impl AnimatedDrawing {
    /// Reads an SVG file to play. Nothing if it holds no drawing.
    pub fn new(svg: &str) -> Option<AnimatedDrawing> {
        ParsedDrawing::read(svg)?;
        Some(AnimatedDrawing {
            text: svg.into(),
            serial: SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            listening: Timeline::read(svg).listening(),
            instances: Vec::new(),
            pending: Vec::new(),
            heard: 0,
            seconds: 0.0,
            last: None,
            paused: false,
            started: None,
            playing: None,
            settled: f32::NEG_INFINITY,
            checkpoints: Checkpoints::new(),
            schedule: None,
            shown: String::new(),
            shown_for: None,
            drawing: None,
            under: Vec::new(),
            focused: Vec::new(),
            had_focus: false,
        })
    }

    /// Where its clock stands: the time since it began.
    pub fn position(&self) -> Duration {
        Duration::try_from_secs_f32(self.seconds).unwrap_or(Duration::MAX)
    }

    /// Sets its clock, as `setCurrentTime` does: it shows as it is then, with all that happened.
    pub fn seek(&mut self, to: Duration) {
        self.seconds = to.as_secs_f32();
    }

    /// Stops its clock, as `pauseAnimations` does.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Starts its clock again, as `unpauseAnimations` does.
    pub fn play(&mut self) {
        self.paused = false;
    }

    /// Whether its clock runs (not paused).
    pub fn playing(&self) -> bool {
        !self.paused
    }

    /// Begins the animation element with this id now, as its `beginElement()` does. False where
    /// no animation has it.
    pub fn begin_element(&mut self, id: &str) -> bool {
        let found = Timeline::read(&self.text).animation_named(id);
        found.map(|element| self.fire(Happened::Begun(element))).is_some()
    }

    /// Ends the animation element with this id now, as its `endElement()` does. False where no
    /// animation has it.
    pub fn end_element(&mut self, id: &str) -> bool {
        let found = Timeline::read(&self.text).animation_named(id);
        found.map(|element| self.fire(Happened::Ended(element))).is_some()
    }

    /// Sends an event (`click`, `mousedown`, `mouseover`, `focusin` and the others a window
    /// gives) to the element with this id now, as `dispatchEvent` does: it comes to that
    /// element and to those it lies in (`focus` and `blur` to the element alone). False where
    /// no element has the id, or the event is none of those.
    pub fn dispatch_event(&mut self, id: &str, event: &str) -> bool {
        let Some(what) = self.event_at(id, event) else { return false };
        self.fire(what);
        true
    }

    /// An event at the element with this id, with the elements it comes to (see
    /// `dispatch_event`).
    fn event_at(&self, id: &str, event: &str) -> Option<Happened> {
        let element = File::read(&self.text).named.get(id).copied()?;
        let name = EVENTS.iter().find(|name| **name == event)?;
        let chain = self.listening.chain(element, &[]);
        let chain = if matches!(*name, "focus" | "blur") { self.listening.focus_chain(&chain) } else { chain };
        Some(Happened::Event(name, chain))
    }

    /// The drawing as it is at its clock, `width` by `height` pixels, as
    /// [`decode`](crate::svg::decode) draws a file.
    pub fn decode(&mut self, width: usize, height: usize) -> Option<(usize, Vec<u32>)> {
        crate::svg::decode(&self.frame(), width, height)
    }

    /// The file's text as it is at its clock.
    fn frame(&mut self) -> String {
        let text = self.text.clone();
        let timeline = Timeline::read(&text);
        let seconds = self.seconds;
        timeline.frame(self.schedule(&timeline), seconds)
    }

    /// Takes in what happened, at the clock's time: taken in by the timeline at its next frame
    /// (or now, once `PENDING_MOST` wait).
    fn fire(&mut self, what: Happened) {
        self.pending.push(Fired { seconds: self.seconds, what });
        self.heard += 1;
        if self.pending.len() >= PENDING_MOST {
            let text = self.text.clone();
            self.schedule(&Timeline::read(&text));
        }
    }

    /// Moves the clock on to the window's time `now`, unless paused. (The first time, the wall
    /// clock's time at its start is taken, and its timeline made again with its `wallclock()`
    /// times.)
    fn advance(&mut self, now: Duration) {
        let last = self.last.replace(now);
        if last.is_none() {
            self.started = Some(crate::platform::unix_time() - f64::from(self.seconds));
            (self.playing, self.schedule, self.checkpoints) = (None, None, Checkpoints::new());
        }
        if let Some(last) = last.filter(|_| !self.paused) {
            self.seconds += now.saturating_sub(last).as_secs_f32();
        }
    }

    /// The schedule at the clock's time: worked out again once something happened, the clock
    /// went back, or it nears where the last one ends (it reaches a minute further).
    fn schedule(&mut self, timeline: &Timeline) -> &Schedule {
        let (seconds, heard) = (self.seconds, self.heard);
        let kept = self.schedule.take().filter(|(schedule, known, from)| *known == heard && seconds >= *from && seconds < schedule.until - 1.0);
        let kept = kept.unwrap_or_else(|| (self.look_ahead(timeline), heard, seconds));
        &self.schedule.insert(kept).0
    }

    /// Plays the timeline on to the clock's time and works out the minute after: every moment
    /// before the clock (less how far before an event its offsets reach) is taken for good, and
    /// what can no longer matter forgotten; a copy goes on a minute further. The schedule is the
    /// one worked out from the start with every event (see `Scheduling::hear`). The copy does
    /// no more work than its span worked out from the start with the instance times of the
    /// last minute and after may: not all the budget saved up while it played.
    fn look_ahead(&mut self, timeline: &Timeline) -> Schedule {
        let (seconds, until, reach) = (self.seconds, self.seconds + LONGEST, self.listening.reach);
        let most = timeline.budget(until, self.heard);
        let settled = seconds - self.listening.behind;
        let mut playing = self.catch_up(timeline, settled);
        playing.run(timeline, |time| time >= settled, most);
        playing.forget(timeline, settled, reach);
        self.checkpoints.keep(settled, &playing);
        let recent = self.instances.len() - self.instances.partition_point(|instance| instance.seconds < settled - LONGEST);
        let ahead_most = most.min(playing.work.saturating_add(timeline.budget(until - settled, recent)));
        let mut ahead = playing.clone();
        (self.playing, self.settled) = (Some(playing), settled);
        let unfinished = ahead.run(timeline, |time| time > until, ahead_most);
        ahead.into_schedule(timeline, until, unfinished)
    }

    /// The timeline with every event taken in, to go on to `settled`: the one playing, with
    /// the instance times of the events waiting added; or, where the clock went back before what
    /// it took for good or an event gave an earlier time, the latest copy kept from before then.
    fn catch_up(&mut self, timeline: &Timeline, settled: f32) -> Scheduling {
        let given: Vec<InstanceTime> = self.pending.drain(..).flat_map(|event| timeline.instance_times(&event)).collect();
        given.iter().for_each(|instance| add_instance(&mut self.instances, *instance));
        let earliest = given.iter().map(|instance| instance.seconds).fold(f32::INFINITY, f32::min);
        self.checkpoints.forget_after(earliest);
        match self.playing.take() {
            Some(mut playing) if self.settled <= earliest.min(settled) => {
                playing.hear(timeline, &given);
                playing
            }
            _ => self.restored(timeline, settled),
        }
    }

    /// The timeline as the latest copy kept from `settled` or before has it (or as it starts),
    /// with every instance time from then on taken in.
    fn restored(&mut self, timeline: &Timeline, settled: f32) -> Scheduling {
        let (from, mut playing) = match self.checkpoints.latest(settled) {
            Some((at, kept)) => (*at, kept.clone()),
            None => (f32::NEG_INFINITY, Scheduling::new(timeline, self.started)),
        };
        let after = self.instances.partition_point(|instance| instance.seconds < from);
        playing.hear(timeline, &self.instances[after..]);
        self.settled = from;
        playing
    }

    /// Takes in what the pointer, keys and focus did over its box (`response`), as events at
    /// the elements under the pointer and with focus: those the file waits for.
    fn take_input(&mut self, ui: &mut Ui, response: &Response) {
        let under = if response.hovered { self.under_pointer(ui) } else { Vec::new() };
        let mut events = crossed(&self.under, &under);
        let on = |happened: bool| happened && !under.is_empty();
        let (input, pressed) = (&ui.input, response.hovered && ui.input.pressed[0]);
        let happened = [(on(input.moved), "mousemove"), (on(pressed), "mousedown"), (on(response.hovered && input.released[0]), "mouseup")];
        events.extend(happened.into_iter().filter(|(happened, _)| *happened).map(|(_, name)| (name, under.clone())));
        if self.listening.follows_focus() {
            events.extend(self.move_focus(ui, response, pressed, &under));
        }
        if on(response.clicked) {
            events.extend(["click", "activate", "DOMActivate"].map(|name| (name, under.clone())));
        }
        let listened: Vec<(&'static str, Vec<usize>)> = events.into_iter().filter(|(name, _)| self.listening.events.contains(name)).collect();
        listened.into_iter().for_each(|(name, chain)| self.fire(Happened::Event(name, chain)));
        self.under = under;
        if self.listening.keys && response.focused {
            ui.typed().chars().for_each(|key| self.fire(Happened::Key(key)));
        }
    }

    /// Moves focus within the drawing as browsers do: with the box's focus gone, from the
    /// element that had it; on a press (`pressed`, over the elements `under`), to the nearest
    /// there that takes focus; with focus come to the box otherwise, to its first stop (by
    /// Shift+Tab, its last); by Tab or Shift+Tab the box kept (see `tab_ways`), to the next stop
    /// that way. Gives the events that makes: `blur` and `focusout`, then `focus` and `focusin`.
    fn move_focus(&mut self, ui: &mut Ui, response: &Response, pressed: bool, under: &[usize]) -> Vec<(&'static str, Vec<usize>)> {
        let had = std::mem::replace(&mut self.had_focus, response.focused);
        // (Tab from the box onto itself, the window's only stop, comes in afresh.)
        let (tabbed, entered) = (ui.input.tabbed, !had || ui.input.tabbed.is_some());
        let chain_of = |stop: Option<usize>| stop.map_or_else(Vec::new, |at| self.listening.chain(at, &[]));
        let next = match (response.focused, pressed, entered) {
            (false, ..) => Vec::new(),
            (true, true, _) => self.listening.focus_under(under),
            (true, false, true) => chain_of(self.listening.entry_stop(tabbed == Some(true))),
            (true, false, false) => match self.tabbed_to(ui) {
                Some(to) => chain_of(Some(to)),
                None => return Vec::new(),
            },
        };
        if next == self.focused {
            return Vec::new();
        }
        let left = std::mem::replace(&mut self.focused, next);
        let mut events = Vec::new();
        if !left.is_empty() {
            events.extend([("blur", self.listening.focus_chain(&left)), ("focusout", left)]);
        }
        if !self.focused.is_empty() {
            events.extend([("focus", self.listening.focus_chain(&self.focused)), ("focusin", self.focused.clone())]);
        }
        events
    }

    /// The stop Tab or Shift+Tab, kept by the box, goes to; none if neither was pressed.
    fn tabbed_to(&self, ui: &mut Ui) -> Option<usize> {
        let back = if ui.key(Key::Tab, Mods::NONE) {
            false
        } else if ui.key(Key::Tab, Mods::SHIFT) {
            true
        } else {
            return None;
        };
        self.listening.next_stop(self.focused.first().copied(), back)
    }

    /// Which of Tab and Shift+Tab the box keeps, to move focus within it: each with a stop that
    /// way.
    fn tab_ways(&self) -> [bool; 2] {
        let from = self.focused.first().copied();
        [false, true].map(|back| self.listening.next_stop(from, back).is_some())
    }

    /// The elements under the pointer, the one it is over first, then those an event there comes
    /// to. Where no shape that takes the pointer is, the root `<svg>` has it, as in browsers
    /// (none where that takes none either).
    fn under_pointer(&self, ui: &Ui) -> Vec<usize> {
        let kept = self.drawing.and_then(|drawing| ui.drawings.get(drawing.0));
        let (Some(kept), Some(at)) = (kept, ui.input.pointer) else { return Vec::new() };
        let takes = |element: usize, uses: &[u32]| self.listening.takes_pointer(element, uses);
        match kept.element_at(at, takes) {
            Some((element, uses)) => self.listening.chain(element, uses),
            None => self.listening.root.filter(|root| self.listening.takes_pointer(*root, &[])).map_or_else(Vec::new, |root| self.listening.chain(root, &[])),
        }
    }

    /// Puts the frame at the clock's time on show, where the time or what happened changed:
    /// read afresh where the frame's text changed, as a new drawing in the window (so its box is
    /// drawn again), the last one dropped.
    fn show_frame(&mut self, ui: &mut Ui) {
        let now = (self.seconds, self.heard);
        let showing = self.drawing.and_then(|drawing| ui.drawings.get(drawing.0));
        if showing.is_some() && self.shown_for == Some(now) {
            return;
        }
        let noted = showing.and_then(ParsedDrawing::noted);
        let frame = self.frame();
        self.shown_for = Some(now);
        if showing.is_some() && frame == self.shown {
            return;
        }
        let Some(mut parsed) = ParsedDrawing::read_with_hits(&frame) else { return };
        if let Some(within) = noted {
            parsed.note(within);
        }
        let drawing = Drawing(ui.drawings.put(parsed));
        if let Some(last) = self.drawing.replace(drawing) {
            ui.drawings.take(last.0);
        }
        self.shown = frame;
    }

    /// How long until the drawing next changes (the schedule being that of the frame on show):
    /// a frame while something moves, else until the next begin or end. Nothing while paused,
    /// or where nothing more happens.
    fn wait(&self) -> Option<Duration> {
        let (schedule, ..) = self.schedule.as_ref().filter(|_| !self.paused)?;
        if schedule.moving(self.seconds) {
            return Some(FRAME);
        }
        // (Where the schedule ends with more to come, it is worked out again then.)
        let next = schedule.next_change(self.seconds).or(schedule.unfinished.then_some(schedule.until - 1.0))?;
        Duration::try_from_secs_f32(next - self.seconds).ok().map(|wait| wait.max(FRAME))
    }
}

/// The events a move of the pointer from over the elements `from` to over `to` makes (each
/// list as an event there comes to them, see `AnimatedDrawing::under`): out of the one left and
/// into the one reached (each coming to all), then a leave or an enter for each element left or
/// reached.
fn crossed(from: &[usize], to: &[usize]) -> Vec<(&'static str, Vec<usize>)> {
    let mut events = Vec::new();
    if from.first() == to.first() {
        return events;
    }
    if !from.is_empty() {
        events.push(("mouseout", from.to_vec()));
    }
    events.extend(from.iter().filter(|element| !to.contains(element)).map(|element| ("mouseleave", vec![*element])));
    if !to.is_empty() {
        events.push(("mouseover", to.to_vec()));
    }
    events.extend(to.iter().filter(|element| !from.contains(element)).map(|element| ("mouseenter", vec![*element])));
    events
}

/// SVG drawings that play their animation in a window.
impl Ui {
    /// A box showing an animated drawing at its clock, at the drawing's own size unless sized
    /// after: `ui.animated_drawing(&mut drawing, color).grow().show()`. `color` is
    /// `currentColor`. The pointer over it, keys while it has focus, and focus moving within it
    /// (where the file waits for keys or focus, it takes focus, and keeps Tab and Shift+Tab while
    /// they move between its elements) begin and end the animations waiting for them. Only this
    /// box is redrawn per frame, and none while nothing in it changes.
    pub fn animated_drawing(&mut self, drawing: &mut AnimatedDrawing, color: Color) -> El<'_> {
        let key = ("taut animated drawing", drawing.serial);
        let id = self.id(key);
        drawing.advance(self.now);
        let response = self.interact(id);
        if !drawing.listening.events.is_empty() || drawing.listening.keys {
            drawing.take_input(self, &response);
        }
        if response.focused && drawing.listening.follows_focus() {
            self.takes_tab = drawing.tab_ways().map(|kept| if kept { id } else { 0 });
        }
        if response.hovered && drawing.listening.follows() {
            self.follow_pointer();
        }
        drawing.show_frame(self);
        if let Some(wait) = drawing.wait().filter(|_| !self.window_minimized()) {
            self.wake_after(wait);
        }
        self.shows.drawing = Some(ParsedDrawing::show_noted);
        let (shown, focusable) = (drawing.drawing, drawing.listening.keys || drawing.listening.follows_focus());
        let el = self.el().key(key).sense();
        let el = if focusable { el.focusable() } else { el };
        match shown {
            Some(shown) => el.drawing(shown, color),
            None => el.size(0.0, 0.0),
        }
    }

    /// Drops an animated drawing: its last frame is freed.
    pub fn drop_animated_drawing(&mut self, drawing: AnimatedDrawing) {
        if let Some(shown) = drawing.drawing {
            self.drop_drawing(shown);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file's span as frames are encoded.
    fn span(text: &str) -> Option<(f32, f32, u32)> {
        let timeline = Timeline::read(text);
        timeline.span(&timeline.schedule(&[], AHEAD))
    }

    /// An attribute of an element (by its place in the file) at a time, timed animations alone
    /// running.
    fn value_at(text: &str, element: usize, name: &str, seconds: f32) -> Option<String> {
        let frame = Timeline::read(text).frame_at(seconds);
        File::read(&frame).own_property(element, name).map(str::to_string)
    }

    /// The intervals a file's animations run, given what happened.
    fn intervals(text: &str, fired: &[Fired]) -> Vec<Vec<[f32; 2]>> {
        Timeline::read(text).schedule(fired, 100.0).intervals
    }

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
        let width = |seconds: f32| value_at(file, 1, "width", seconds);
        assert_eq!([0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 9.0].map(width), ["10", "10", "15", "10", "15", "20", "20"].map(|value| Some(value.to_string())));
        assert_eq!(span(file), Some((0.0, 5.0, 1)));
    }

    #[test]
    fn key_times_paces_and_splines_place_the_values() {
        let x = |extra: &str, seconds: f32| {
            let text = format!(r#"<svg><rect x="0"><animate attributeName="x" values="0;10;30" dur="1s" {extra}/></rect></svg>"#);
            parts(&value_at(&text, 1, "x", seconds).unwrap()).0[0]
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
    fn a_key_time_repeated_at_the_end_holds_the_last_value() {
        let file = r#"<svg><rect x="0"><animate attributeName="x" values="0;10;30" keyTimes="0;1;1" dur="1s" fill="freeze"/></rect></svg>"#;
        assert_eq!(value_at(file, 1, "x", 2.0).as_deref(), Some("30"));
    }

    #[test]
    fn a_long_animation_is_encoded_for_a_minute_at_most() {
        let file = r#"<svg><rect><animate attributeName="x" values="0;1" dur="100000s"/></rect></svg>"#;
        assert_eq!(span(file), Some((0.0, LONGEST, 1)));
    }

    #[test]
    fn transforms_colours_sets_and_sums_are_written_where_they_count() {
        let file = r##"<svg><style>rect { fill: green }</style><rect id="r" transform="translate(5)" style="opacity:1"><animateTransform attributeName="transform" type="rotate" from="0 5 5" to="90 5 5" dur="1s" additive="sum"/><animate attributeName="fill" from="red" to="blue" dur="1s"/><set attributeName="visibility" to="hidden" begin="2s"/></rect><animate href="#r" attributeName="x" by="10" dur="1s" accumulate="sum" repeatCount="3"/></svg>"##;
        let timeline = Timeline::read(file);
        let frame = timeline.frame_at(0.5);
        let read = File::read(&frame);
        assert_eq!(read.attribute(2, "transform"), Some("translate(5) rotate(45 5 5)"));
        // A property goes into `style`, last: it wins over the style sheet.
        assert_eq!(read.own_property(2, "fill"), Some("#800080ff"));
        assert_eq!(read.own_property(2, "visibility"), None);
        assert_eq!(File::read(&timeline.frame_at(2.0)).own_property(2, "visibility"), Some("hidden"));
        // `by` alone adds to the value under it; each cycle repeated adds its last value.
        assert_eq!(read.attribute(2, "x"), Some("5"));
        assert_eq!(File::read(&timeline.frame_at(1.5)).attribute(2, "x"), Some("15"));
        // A set with no end, and the rest done: three seconds once.
        assert_eq!(span(file), Some((0.0, 3.0, 1)));
    }

    #[test]
    fn what_runs_for_ever_is_one_cycle_of_all_that_repeats() {
        let file = r#"<svg><rect><animate attributeName="x" values="0;1" dur="0.4s" repeatCount="indefinite"/><animate attributeName="y" values="0;1" dur="0.6s" begin="1s" repeatCount="indefinite"/></rect></svg>"#;
        let (start, long, plays) = span(file).unwrap();
        assert!(start == 1.0 && (long - 1.2).abs() < 1e-6 && plays == 0, "{start} {long} {plays}");
        assert_eq!(span("<svg><rect/></svg>"), None);
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

    #[test]
    fn begin_and_end_lists_take_every_kind_of_entry() {
        let file = r##"<svg><rect id="r"><animate id="a" attributeName="x" dur="1s"/></rect><g id="g"><circle/></g></svg>"##;
        let file = File::read(file);
        let read = |entry: &'static str| condition(&file, entry, 1);
        assert_eq!(read(" 2.5s "), Some(Condition::Clock(2.5)));
        assert_eq!(read("a.end + 1s"), Some(Condition::Synced(2, Moment::End, 1.0)));
        assert_eq!(read("a.begin-0.5s"), Some(Condition::Synced(2, Moment::Begin, -0.5)));
        assert_eq!(read("a.repeat(3)"), Some(Condition::Synced(2, Moment::Repeat(3), 0.0)));
        assert_eq!(read("a.repeatEvent"), Some(Condition::Synced(2, Moment::Repeat(0), 0.0)));
        assert_eq!(read("click"), Some(Condition::Event(1, "click", 0.0)));
        assert_eq!(read("click+0.5s"), Some(Condition::Event(1, "click", 0.5)));
        assert_eq!(read("g.mouseover+200ms"), Some(Condition::Event(3, "mouseover", 0.2)));
        assert_eq!(read("accessKey(q)-1s"), Some(Condition::Key('q', -1.0)));
        assert_eq!(read("wallclock(2026-01-01T00:00:00Z)"), Some(Condition::Wall("2026-01-01T00:00:00Z")));
        assert_eq!(read("g.focusin"), Some(Condition::Event(3, "focusin", 0.0)));
        // Nothing to wait for: never.
        assert_eq!(["indefinite", "nothing.end", "a.wobble", "a.repeat(0)", "click+x"].map(read), [None; 5]);
    }

    #[test]
    fn an_animation_begins_and_ends_with_others() {
        // b begins when a ends, a second later; c half a second before b ends... c ends with a's
        // second begin.
        let file = r##"<svg><rect><animate id="a" attributeName="x" values="0;1" dur="2s" begin="0s;b.end"/><animate id="b" attributeName="y" values="0;1" dur="1s" begin="a.end+1s"/><animate id="c" attributeName="width" values="0;1" dur="10s" begin="b.end-0.5s" end="a.begin"/></rect></svg>"##;
        let runs = intervals(file, &[]);
        assert_eq!(&runs[0][..3], &[[0.0, 2.0], [4.0, 6.0], [8.0, 10.0]]);
        assert_eq!(&runs[1][..2], &[[3.0, 4.0], [7.0, 8.0]]);
        assert_eq!(&runs[2][..2], &[[3.5, 4.0], [7.5, 8.0]]);
        // The loop comes again every four seconds: one cycle of it, for ever.
        let (start, long, plays) = span(file).unwrap();
        assert!((long - 4.0).abs() < 1e-3 && plays == 0 && start > 0.0, "{start} {long} {plays}");
    }

    #[test]
    fn a_repeat_and_a_restart_are_told_as_smil_says() {
        let file = r##"<svg><rect><animate id="a" attributeName="x" values="0;1" dur="1s" repeatCount="4"/><animate attributeName="y" values="0;1" dur="0.5s" begin="a.repeat(2)"/><animate attributeName="width" values="0;1" dur="3s" begin="0s;1s" restart="whenNotActive"/><animate attributeName="height" values="0;1" dur="3s" begin="0s;1s"/><animate attributeName="rx" values="0;1" dur="1s" begin="0s;2s" restart="never"/></rect></svg>"##;
        let runs = intervals(file, &[]);
        assert_eq!(runs[1], [[2.0, 2.5]]);
        // Begun again while running: not, when not active; else at once, ending the first.
        assert_eq!(runs[2], [[0.0, 3.0]]);
        assert_eq!(runs[3], [[0.0, 1.0], [1.0, 4.0]]);
        assert_eq!(runs[4], [[0.0, 1.0]]);
    }

    #[test]
    fn min_holds_an_interval_past_its_end_and_its_fill_applies_past_its_repeats() {
        // As Edge draws it: ended at 1s, held to 2s; repeats done at 1s, gone until 3s.
        let file = r#"<svg><rect><animate attributeName="x" values="0;10" dur="5s" end="1s" min="2s" fill="freeze"/><animate attributeName="y" values="0;10" dur="1s" min="3s"/></rect></svg>"#;
        assert_eq!(intervals(file, &[]), [[[0.0, 2.0]], [[0.0, 3.0]]]);
        let read = |name: &str| [0.5, 1.5, 2.5].map(|seconds| value_at(file, 1, name, seconds));
        assert_eq!(read("x").each_ref().map(Option::as_deref), [Some("1"), Some("3"), Some("4")]);
        assert_eq!(read("y").each_ref().map(Option::as_deref), [Some("5"), None, None]);
    }

    #[test]
    fn an_end_of_clock_values_before_a_begin_keeps_it_from_beginning() {
        // Begun at 0 and 5, ended at 3: the second begin has no end after it.
        let file = r#"<svg><rect><animate attributeName="x" values="0;1" dur="10s" begin="0s;5s" end="3s"/><animate attributeName="y" values="0;1" dur="2s" end="click"/></rect></svg>"#;
        let runs = intervals(file, &[]);
        assert_eq!(runs[0], [[0.0, 3.0]]);
        // An end waiting for an event leaves its active time to end it.
        assert_eq!(runs[1], [[0.0, 2.0]]);
    }

    #[test]
    fn events_begin_animations_where_they_happen_and_where_they_come_to() {
        let file = r##"<svg><g id="g"><rect id="r"/><circle id="c"/></g><animate href="#r" attributeName="x" values="0;1" dur="1s" begin="click"/><animate href="#c" attributeName="y" values="0;1" dur="1s" begin="g.click+0.5s"/><animate href="#r" attributeName="width" values="0;1" dur="1s" begin="accessKey(k)"/></svg>"##;
        let timeline = Timeline::read(file);
        // Timed alone, none begins.
        assert!(timeline.schedule(&[], 100.0).intervals.iter().all(Vec::is_empty));
        // A click on the circle comes to the group, not the rectangle.
        let click_c = Fired { seconds: 2.0, what: Happened::Event("click", timeline.listening().chain(3, &[])) };
        let runs = timeline.schedule(&[click_c], 100.0).intervals;
        assert_eq!(runs, [vec![], vec![[2.5, 3.5]], vec![]]);
        // On the rectangle, both; a key, the third.
        let click_r = Fired { seconds: 1.0, what: Happened::Event("click", timeline.listening().chain(2, &[])) };
        let key = Fired { seconds: 4.0, what: Happened::Key('k') };
        let runs = timeline.schedule(&[click_r, key], 100.0).intervals;
        assert_eq!(runs, [vec![[1.0, 2.0]], vec![[1.5, 2.5]], vec![[4.0, 5.0]]]);
        // Encoded as frames, events never come.
        assert_eq!(span(file), None);
    }

    #[test]
    fn of_two_animations_of_one_attribute_the_one_begun_last_wins() {
        let file = r#"<svg><rect x="0"><set id="late" attributeName="x" to="2" begin="1s"/><set attributeName="x" to="1" begin="0s"/></rect></svg>"#;
        assert_eq!(value_at(file, 1, "x", 0.5).as_deref(), Some("1"));
        assert_eq!(value_at(file, 1, "x", 1.5).as_deref(), Some("2"));
    }

    /// Where an `animateMotion`'s target is put and how it is turned at a time: the numbers of
    /// its written transform (x, y, angle).
    fn moved(motion: &str, seconds: f32) -> Vec<f32> {
        let file = format!(r##"<svg><path id="p" d="M0 0H10V10"/><rect transform="scale(2)">{motion}</rect></svg>"##);
        let transform = Timeline::read(&file).frame_at(seconds);
        let transform = File::read(&transform).attribute(2, "transform").unwrap().to_string();
        assert!(transform.ends_with("scale(2)"), "{transform}");
        numbers(&transform.replace(['(', ')', 'a', 'b', 'c', 'e', 'l', 'n', 'o', 'r', 's', 't'], " ")).take(3).collect()
    }

    #[test]
    fn a_motion_moves_along_its_path_at_one_speed_before_the_element_s_own_transform() {
        assert_eq!(moved(r#"<animateMotion path="M0 0H10V10" dur="2s"/>"#, 0.5), [5.0, 0.0, 0.0]);
        assert_eq!(moved(r#"<animateMotion path="M0 0H10V10" dur="2s" rotate="auto"/>"#, 1.5), [10.0, 5.0, 90.0]);
        assert_eq!(moved(r#"<animateMotion path="M0 0H10V10" dur="2s" rotate="auto-reverse"/>"#, 0.5), [5.0, 0.0, 180.0]);
        // Through an `<mpath>`, placed by key points; a fixed angle.
        assert_eq!(moved(r##"<animateMotion dur="1s" keyPoints="0.5;1" keyTimes="0;1" calcMode="linear" rotate="30"><mpath href="#p"/></animateMotion>"##, 0.0), [10.0, 0.0, 30.0]);
        // Through points: each at its key time, evenly in time where linear.
        assert_eq!(moved(r#"<animateMotion values="0,0;10,0;10,30" dur="2s" calcMode="linear"/>"#, 0.5), [5.0, 0.0, 0.0]);
        assert_eq!(moved(r#"<animateMotion values="0,0;10,0;10,30" dur="2s"/>"#, 0.5), [10.0, 0.0, 0.0]);
        assert_eq!(moved(r#"<animateMotion from="5,5" by="10,0" dur="1s" fill="freeze"/>"#, 3.0), [15.0, 5.0, 0.0]);
    }

    #[test]
    fn a_repeated_motion_with_accumulate_goes_on_from_where_it_ended() {
        assert_eq!(moved(r#"<animateMotion path="M0 0H10" dur="1s" repeatCount="3" accumulate="sum" fill="freeze"/>"#, 1.5), [15.0, 0.0, 0.0]);
        assert_eq!(moved(r#"<animateMotion path="M0 0H10" dur="1s" repeatCount="3" accumulate="sum" fill="freeze"/>"#, 5.0), [30.0, 0.0, 0.0]);
    }

    #[test]
    fn a_schedule_takes_so_many_moments_at_most_however_its_animations_chain() {
        // Each begins when the one before ends or first repeats, the first again when the last
        // ends: a ring of millisecond animations that would never stop.
        let count = 500;
        let mut text = String::from("<svg><rect>");
        for n in 0..count {
            let (before, first) = ((n + count - 1) % count, if n == 0 { "0s;" } else { "" });
            text += &format!(r#"<animate id="a{n}" attributeName="x" values="0;1" dur="1ms" repeatCount="3" begin="{first}a{before}.end;a{before}.repeat(1)"/>"#);
        }
        text += "</rect></svg>";
        let timeline = Timeline::read(&text);
        let schedule = timeline.schedule(&[], AHEAD);
        // Each interval is a moment taken: no more than the budget for three minutes.
        let intervals: usize = schedule.intervals.iter().map(Vec::len).sum();
        assert!(intervals > count && intervals <= (MOMENTS + MOMENTS_EACH * count) * 3, "{intervals}");
        assert!(schedule.unfinished);
        // A frame writes each animated attribute once.
        assert!(timeline.frame(&schedule, 0.0005).len() < text.len() + 64);
    }

    #[test]
    fn each_animation_told_of_a_moment_counts_against_the_budget() {
        // One animation repeating every millisecond, 300 waiting for each repeat: 300 tellings a
        // millisecond use the budget up within the first second, not after 20 s of tellings.
        let mut text = String::from(r#"<svg><rect><animate id="a" attributeName="x" values="0;1" dur="1ms" repeatCount="indefinite"/>"#);
        text += &r#"<animate attributeName="y" values="0;1" dur="10s" begin="a.repeatEvent" restart="whenNotActive"/>"#.repeat(300);
        text += "</rect></svg>";
        let schedule = Timeline::read(&text).schedule(&[], 60.0);
        let last = schedule.intervals.iter().flatten().map(|[begin, _]| *begin).fold(0.0, f32::max);
        assert!(schedule.unfinished && last < 1.0, "{last}");
    }

    #[test]
    fn wall_clock_times_are_read_in_every_form() {
        let day = crate::date::Date::new(2026, 11, 26).days() as f64 * 86_400.0;
        let at = |text: &str| wall_time(text, day + 43_200.0);
        assert_eq!(at("2026-11-26T14:30:15.5Z"), Some(day + 52_215.5));
        assert_eq!(at("2026-11-26T14:30+01:00"), Some(day + 48_600.0));
        assert_eq!(at("2026-11-26T14:30-0530"), Some(day + 72_000.0));
        assert_eq!(at("2026-11-27"), Some(day + 86_400.0 - crate::date::ahead_of_greenwich((day + 86_400.0) as i64) as f64));
        // A time alone is on the day the clock started; without a zone, in local time.
        assert_eq!(at("12:00Z"), Some(day + 43_200.0));
        assert_eq!(at("14:30"), Some(day + 52_200.0 - crate::date::ahead_of_greenwich((day + 52_200.0) as i64) as f64));
        assert_eq!(["2026-13-01", "noon", "12", "", "2026-11-26T14", "14:30+x", "NaN:00", "inf:00"].map(at), [None; 8]);
    }

    #[test]
    fn a_wall_clock_time_begins_an_animation_in_a_window_as_the_clock_reaches_it() {
        let file = r#"<svg viewBox="0 0 4 4"><rect width="4" height="4"><set attributeName="width" to="1" begin="wallclock(2026-11-26T12:00:02Z)"/></rect></svg>"#;
        let mut drawing = AnimatedDrawing::new(file).unwrap();
        drawing.started = Some(crate::date::Date::new(2026, 11, 26).days() as f64 * 86_400.0 + 43_200.0);
        let width = |drawing: &mut AnimatedDrawing| File::read(&drawing.frame()).own_property(1, "width").map(str::to_string);
        drawing.seek(Duration::from_millis(1500));
        assert_eq!(width(&mut drawing).as_deref(), Some("4"));
        drawing.seek(Duration::from_millis(2500));
        assert_eq!(width(&mut drawing).as_deref(), Some("1"));
        // Drawn at a moment, it has no wall clock: never.
        assert_eq!(value_at(file, 1, "width", 1e9), Some("4".to_string()));
    }

    /// A file of animations begun and ended by events, by each other with offsets on both
    /// sides, restarted in each way, held by `min`.
    const EVENTFUL: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect id="r" width="5" height="5"/><rect id="s" x="5" width="5" height="5"/>
        <animate id="a" href="#r" attributeName="x" values="0;5" dur="1s" begin="r.click" end="s.mousemove+0.2s" fill="freeze"/>
        <animate id="b" href="#s" attributeName="y" values="0;5" dur="0.5s" begin="a.begin-0.3s;s.click-0.25s" repeatCount="3"/>
        <set href="#s" attributeName="width" to="2" begin="b.repeat(1)" end="r.mouseover"/>
        <animate href="#r" attributeName="height" values="5;1" dur="0.4s" begin="r.mousedown;2s" restart="whenNotActive" min="0.7s"/>
        <set href="#r" attributeName="fill" to="red" begin="r.click+1s" restart="never"/>
        <animate href="#s" attributeName="height" values="5;2" dur="0.3s" begin="0s;b.end+0.1s;r.click" end="s.click"/></svg>"##;

    /// Sends an event to a drawing, as `dispatch_event` does; what it fired.
    fn dispatched(drawing: &mut AnimatedDrawing, id: &str, event: &str) -> Option<Fired> {
        let what = drawing.event_at(id, event)?;
        drawing.fire(what.clone());
        Some(Fired { seconds: drawing.seconds, what })
    }

    #[test]
    fn playing_on_draws_what_working_out_from_the_start_draws() {
        let events =
            [(0.3, "r", "click"), (0.9, "s", "mousemove"), (1.0, "s", "click"), (1.0, "r", "mousedown"), (1.6, "r", "mouseover"), (2.2, "r", "click"), (2.25, "s", "click"), (3.1, "s", "mousemove")];
        let (timeline, mut drawing) = (Timeline::read(EVENTFUL), AnimatedDrawing::new(EVENTFUL).unwrap());
        let (mut next, mut fired) = (events.iter().peekable(), Vec::new());
        let mut compare = |drawing: &mut AnimatedDrawing, step: u32| {
            let seconds = step as f32 * 0.05;
            drawing.seek(Duration::from_secs_f32(seconds));
            while let Some((_, id, event)) = next.next_if(|(at, ..)| *at <= seconds + 1e-4) {
                fired.extend(dispatched(drawing, id, event));
            }
            let worked_out = timeline.frame(&timeline.schedule(&fired, drawing.seconds + LONGEST), drawing.seconds);
            assert_eq!(drawing.frame(), worked_out, "at {seconds}");
        };
        (0..100).for_each(|step| compare(&mut drawing, step));
        // Back, and on again: worked out from the start with what happened.
        (10..60).for_each(|step| compare(&mut drawing, step));
    }

    #[test]
    fn an_event_costs_the_same_however_long_the_drawing_has_played() {
        // Each move over the shape begins an animation, and its end the next of a chain; one
        // runs again and again by itself. A move a second for a thousand seconds.
        let file = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect id="r" width="5" height="5"/>
            <animate id="a" href="#r" attributeName="x" values="0;5" dur="0.2s" begin="r.mousemove" end="r.mousemove+0.5s"/>
            <animate id="b" href="#r" attributeName="y" values="0;5" dur="0.2s" begin="a.end"/>
            <animate href="#r" attributeName="width" values="5;1" dur="0.3s" begin="0s;b.end+0.2s;r.click"/>
            <animate id="c" href="#r" attributeName="height" values="5;1" dur="0.25s" begin="0s;c.end"/></svg>"##;
        let (timeline, mut drawing) = (Timeline::read(file), AnimatedDrawing::new(file).unwrap());
        let mut cost = |seconds: u64| {
            drawing.seek(Duration::from_secs(seconds));
            drawing.dispatch_event("r", "mousemove");
            let before = drawing.playing.as_ref().map_or(0, |playing| playing.work);
            drawing.frame();
            let playing = drawing.playing.as_ref().unwrap();
            let mut ahead = playing.clone();
            ahead.run(&timeline, |time| time > drawing.seconds + LONGEST, usize::MAX);
            // Moments taken for good, moments taken a minute ahead, and what it holds.
            [playing.work - before, ahead.work - playing.work, playing.held()]
        };
        // (Before the first move, nothing it begins ran: the first second costs less.)
        cost(1);
        let first = cost(2);
        for seconds in 3..=1001 {
            assert_eq!(cost(seconds), first, "the event at {seconds} s");
        }
    }

    #[test]
    fn a_click_keeps_what_it_began_however_many_moves_follow() {
        // Before, once 4,096 moves had come the click was forgotten, and the fill went back.
        let file = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect id="r" width="5" height="5"><set attributeName="fill" to="red" begin="click"/><animate attributeName="x" values="0;1" dur="0.1s" begin="mousemove"/></rect></svg>"##;
        let mut drawing = AnimatedDrawing::new(file).unwrap();
        drawing.seek(Duration::from_secs(1));
        drawing.dispatch_event("r", "click");
        for n in 1..=5000 {
            drawing.seek(Duration::from_secs_f32(1.0 + n as f32 * 0.01));
            drawing.dispatch_event("r", "mousemove");
            if n % 1000 == 0 {
                assert_eq!(File::read(&drawing.frame()).own_property(1, "fill"), Some("red"), "after {n} moves");
            }
        }
        drawing.frame();
        assert!(drawing.playing.as_ref().unwrap().held() < 16, "{}", drawing.playing.as_ref().unwrap().held());
    }

    #[test]
    fn a_wall_clock_time_counts_from_the_first_show_after_an_earlier_frame() {
        // A frame drawn before it shows has no wall clock: showing works the timeline out again.
        let file = r#"<svg viewBox="0 0 4 4"><rect width="4" height="4"><set attributeName="width" to="1" begin="wallclock(2026-11-26T12:00:02Z)"/></rect></svg>"#;
        let mut drawing = AnimatedDrawing::new(file).unwrap();
        drawing.frame();
        drawing.advance(Duration::ZERO);
        drawing.started = Some(crate::date::Date::new(2026, 11, 26).days() as f64 * 86_400.0 + 43_200.0);
        drawing.seek(Duration::from_millis(2500));
        assert_eq!(File::read(&drawing.frame()).own_property(1, "width"), Some("1"));
    }

    /// A file whose moves over its shape each begin an animation.
    const MOVED: &str =
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect id="r" width="5" height="5"><animate attributeName="x" values="0;1" dur="0.1s" begin="mousemove"/></rect></svg>"##;

    #[test]
    fn moves_over_a_paused_drawing_keep_its_timeline_small() {
        let mut drawing = AnimatedDrawing::new(MOVED).unwrap();
        drawing.seek(Duration::from_secs(1));
        drawing.pause();
        for _ in 0..1000 {
            drawing.dispatch_event("r", "mousemove");
            drawing.frame();
        }
        assert!(drawing.playing.as_ref().unwrap().held() < 16, "{}", drawing.playing.as_ref().unwrap().held());
    }

    #[test]
    fn events_with_no_frame_drawn_keep_its_timeline_small() {
        let mut drawing = AnimatedDrawing::new(MOVED).unwrap();
        for n in 0..3 * PENDING_MOST {
            drawing.seek(Duration::from_secs_f32(n as f32 * 0.01));
            drawing.dispatch_event("r", "mousemove");
        }
        let held = drawing.playing.as_ref().map_or(0, Scheduling::held);
        assert!(held < PENDING_MOST, "{held}");
    }

    #[test]
    fn the_minute_ahead_costs_no_more_after_hours_of_play() {
        // Clicked, it repeats at no distance (the clock's step is coarser than its cycle) until
        // the budget is spent: not the budget of all the hours played, on every event.
        let file = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect id="r" width="5" height="5"/>
            <animate id="a" href="#r" attributeName="x" values="0;5" dur="0.000001s" repeatCount="indefinite" begin="r.click+30s"/>
            <animate href="#r" attributeName="y" values="0;5" dur="1s" begin="a.repeat(1)"/></svg>"##;
        // (Before, ten hours in, an event took 170 times as long as a minute in.)
        let event = |played: u64| {
            let mut drawing = AnimatedDrawing::new(file).unwrap();
            drawing.seek(Duration::from_secs(played));
            drawing.dispatch_event("r", "click");
            drawing.frame();
            drawing.seek(Duration::from_secs(played + 1));
            drawing.dispatch_event("r", "mousedown");
            let started = std::time::Instant::now();
            drawing.frame();
            assert!(drawing.schedule.as_ref().is_some_and(|(schedule, ..)| schedule.unfinished), "the budget runs out");
            started.elapsed()
        };
        let best = |played: u64| (0..3).map(|_| event(played)).min().unwrap_or_default();
        let (minute, hours) = (best(60), best(36_000));
        assert!(hours < 10 * minute.max(Duration::from_millis(2)), "a minute in {minute:?}, ten hours in {hours:?}");
    }

    #[test]
    fn tab_reaches_the_lowest_positive_tabindex_first_as_in_browsers() {
        let first = |file: &str| {
            let timeline = Timeline::read(file);
            timeline.listening().entry_stop(false).and_then(|at| timeline.file.attribute(at, "id").map(str::to_string))
        };
        let positive = r##"<svg><rect id="a" tabindex="0"/><rect id="b" tabindex="3"/><rect id="c" tabindex="2"/><rect id="d" tabindex="2"/></svg>"##;
        assert_eq!(first(positive).as_deref(), Some("c"));
        let none = r##"<svg><rect id="a" tabindex="-1"/><a id="b" href="#a"/><rect id="c" tabindex="0"/></svg>"##;
        assert_eq!(first(none).as_deref(), Some("b"));
    }

    #[test]
    fn a_seek_back_keeps_what_happened_as_edge_does() {
        // Clicked at 1 s: seen from 0.5 s it has not begun; at 2 s it runs from 1 s.
        let file =
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect id="r" width="50" height="50"><animate attributeName="x" values="0;50" dur="5s" begin="click"/></rect></svg>"##;
        let mut drawing = AnimatedDrawing::new(file).unwrap();
        let x = |drawing: &mut AnimatedDrawing, seconds: f32| {
            drawing.seek(Duration::from_secs_f32(seconds));
            File::read(&drawing.frame()).attribute(1, "x").map(str::to_string)
        };
        x(&mut drawing, 1.0);
        drawing.dispatch_event("r", "click");
        assert_eq!(x(&mut drawing, 0.5), None);
        assert_eq!(x(&mut drawing, 2.0).as_deref(), Some("10"));
    }

    #[test]
    fn a_seek_back_after_twenty_thousand_events_draws_what_working_out_from_the_start_draws() {
        // Before, a seek back kept the last 4,096 events: those before were lost.
        let (timeline, mut drawing) = (Timeline::read(EVENTFUL), AnimatedDrawing::new(EVENTFUL).unwrap());
        let kinds = [("r", "click"), ("s", "mousemove"), ("s", "click"), ("r", "mousedown"), ("r", "mouseover")];
        let send = |drawing: &mut AnimatedDrawing, n: u32| {
            let (id, event) = kinds[(n.wrapping_mul(2_654_435_761) >> 28) as usize % kinds.len()];
            dispatched(drawing, id, event)
        };
        let mut fired = Vec::new();
        for n in 0..20_000 {
            drawing.seek(Duration::from_secs_f32(n as f32 * 0.1));
            fired.extend(send(&mut drawing, n));
            if n % 10 == 0 {
                drawing.frame();
            }
        }
        let compare = |drawing: &mut AnimatedDrawing, fired: &[Fired], seconds: f32| {
            drawing.seek(Duration::from_secs_f32(seconds));
            let worked_out = timeline.frame(&timeline.schedule(fired, seconds + LONGEST), seconds);
            assert_eq!(drawing.frame(), worked_out, "at {seconds}");
        };
        for seconds in [0.35, 3.0, 123.45, 1000.05, 1999.0, 50.0] {
            compare(&mut drawing, &fired, seconds);
        }
        assert!(drawing.checkpoints.kept.len() <= CHECKPOINTS, "{}", drawing.checkpoints.kept.len());
        // Events after a seek back: copies of later times go; on from there.
        fired.extend((20_000..20_040).filter_map(|n| send(&mut drawing, n)));
        for seconds in [50.0, 50.3, 51.7, 300.0, 49.0] {
            compare(&mut drawing, &fired, seconds);
        }
    }

    #[test]
    fn an_event_on_what_a_use_brings_bubbles_through_the_use_as_in_edge() {
        // As Edge tells a real click (`own/probes/usebubble.svg`): the brought element, then
        // what it lies in up to what the `<use>` brings, then the `<use>`; never `outer` or
        // `symwrap`, where they lie in the file.
        let file = r##"<svg id="root"><defs><g id="outer"><rect id="ref"/></g><g id="symwrap"><symbol id="sym"><g id="ing"><use id="inner" href="#ref"/></g></symbol></g></defs><g id="holder"><use id="u" href="#ref"/></g><g id="holder2"><use id="u2" href="#sym"/></g></svg>"##;
        let (read, listening) = (File::read(file), Timeline::read(file).listening());
        let named = |id: &str| read.named[id];
        let ids = |chain: Vec<usize>| chain.iter().map(|at| read.attribute(*at, "id").unwrap_or("?")).collect::<Vec<&str>>().join(" ");
        assert_eq!(ids(listening.chain(named("ref"), &[named("u") as u32])), "ref u holder root");
        assert_eq!(ids(listening.chain(named("ref"), &[named("inner") as u32, named("u2") as u32])), "ref inner ing sym u2 holder2 root");
        // Not brought: where it lies.
        assert_eq!(ids(listening.chain(named("ref"), &[])), "ref outer ? root");
    }

    #[test]
    fn a_move_of_the_pointer_makes_out_over_leave_and_enter() {
        // From over the circle (in the group) to over the rectangle (in the group too).
        let events = crossed(&[3, 1, 0], &[2, 1, 0]);
        let names: Vec<&str> = events.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, ["mouseout", "mouseleave", "mouseover", "mouseenter"]);
        assert_eq!((events[1].1.clone(), events[3].1.clone()), (vec![3], vec![2]));
        assert!(crossed(&[2, 1, 0], &[2, 1, 0]).is_empty());
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::accessibility::Role;
    use crate::testing::Trial;

    /// A trial window showing a drawing; the colour at a point of the drawing (in its units, it
    /// being 100 units square and shown at its size).
    fn shown(svg: &str) -> Trial<AnimatedDrawing> {
        Trial::new(AnimatedDrawing::new(svg).unwrap(), |drawing, ui| _ = ui.animated_drawing(drawing, Color::BLACK).show())
    }

    fn color_at(trial: &Trial<AnimatedDrawing>, x: f32, y: f32) -> u32 {
        let within = trial.app.drawing.and_then(|drawing| trial.ui.drawings.get(drawing.0)).and_then(ParsedDrawing::noted).unwrap();
        let (x, y) = ((within.x + x) as usize, (within.y + y) as usize);
        trial.screen.pixels[y * trial.screen.width + x] & 0xff_ffff
    }

    /// Where the drawing's point lies in the window.
    fn place(trial: &Trial<AnimatedDrawing>, x: f32, y: f32) -> Point {
        let within = trial.app.drawing.and_then(|drawing| trial.ui.drawings.get(drawing.0)).and_then(ParsedDrawing::noted).unwrap();
        pt(within.x + x, within.y + y)
    }

    const HALVES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><rect id="r" width="50" height="100" fill="#ff0000"><set attributeName="fill" to="#0000ff" begin="click"/></rect><rect id="g" x="50" width="50" height="100" fill="#00ff00"><animate attributeName="x" from="50" to="0" dur="1s" begin="mouseover" fill="freeze"/></rect></svg>"##;

    #[test]
    fn a_drawing_in_a_window_waits_for_the_pointer_over_its_shapes() {
        let mut trial = shown(HALVES);
        assert_eq!([color_at(&trial, 25.0, 50.0), color_at(&trial, 75.0, 50.0)], [0xff0000, 0x00ff00]);
        // Nothing runs by itself: nothing to wake for.
        assert_eq!(trial.ui.wake_in(), None);
        // A click on the left half makes it blue; the right half waits for the pointer.
        let left = place(&trial, 25.0, 50.0);
        trial.click(left);
        assert_eq!([color_at(&trial, 25.0, 50.0), color_at(&trial, 75.0, 50.0)], [0x0000ff, 0x00ff00]);
        assert_eq!(trial.ui.wake_in(), None);
        let right = place(&trial, 75.0, 50.0);
        trial.move_to(right);
        assert_eq!(trial.ui.wake_in(), Some(FRAME));
        trial.wait(Duration::from_millis(1100));
        // It moved over the left half in a second, and stays there.
        assert_eq!(color_at(&trial, 25.0, 50.0), 0x00ff00);
        assert_ne!(color_at(&trial, 75.0, 50.0), 0x00ff00);
        assert_eq!(trial.ui.wake_in(), None);
    }

    #[test]
    fn keys_begin_what_waits_for_them_while_the_drawing_has_focus() {
        let keyed = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><rect width="100" height="100" fill="#ff0000"><set attributeName="fill" to="#0000ff" begin="accessKey(b)"/></rect></svg>"##;
        let mut trial = shown(keyed);
        trial.type_text("b");
        assert_eq!(color_at(&trial, 50.0, 50.0), 0xff0000, "not focused: the key is not its");
        let middle = place(&trial, 50.0, 50.0);
        trial.click(middle);
        trial.type_text("a");
        assert_eq!(color_at(&trial, 50.0, 50.0), 0xff0000);
        trial.type_text("b");
        assert_eq!(color_at(&trial, 50.0, 50.0), 0x0000ff);
    }

    #[test]
    fn the_app_begins_and_seeks_what_it_names() {
        let named = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><rect width="100" height="100" fill="#ff0000"><animate id="a" attributeName="fill" from="#ff0000" to="#0000ff" dur="2s" begin="indefinite" fill="freeze"/></rect></svg>"##;
        let mut drawing = AnimatedDrawing::new(named).unwrap();
        assert!(!drawing.begin_element("nothing") && !drawing.dispatch_event("a", "wobble"));
        drawing.seek(Duration::from_secs(1));
        assert!(drawing.begin_element("a"));
        drawing.seek(Duration::from_secs(2));
        let (_, pixels) = drawing.decode(4, 4).unwrap();
        assert_eq!(pixels[5], 0xff800080);
        drawing.seek(Duration::from_secs(9));
        assert_eq!(drawing.decode(4, 4).unwrap().1[5], 0xff0000ff);
    }

    /// The left half takes focus (a `tabindex`) and turns blue while it has it; the right half
    /// takes none.
    const FOCUSED: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><rect width="50" height="100" fill="#ff0000" tabindex="0"><set attributeName="fill" to="#0000ff" begin="focusin" end="focusout"/></rect><rect x="50" width="50" height="100" fill="#00ff00"/></svg>"##;

    #[test]
    fn a_press_moves_focus_within_the_drawing_as_in_edge() {
        // As Edge does (`own/probes/focus.svg`): a press on what takes focus gives it focus; one
        // on what takes none takes focus from it.
        let mut trial = shown(FOCUSED);
        let (left, right) = (place(&trial, 25.0, 50.0), place(&trial, 75.0, 50.0));
        trial.click(left);
        assert_eq!(color_at(&trial, 25.0, 50.0), 0x0000ff);
        trial.click(right);
        assert_eq!(color_at(&trial, 25.0, 50.0), 0xff0000);
        trial.click(left);
        assert_eq!(color_at(&trial, 25.0, 50.0), 0x0000ff);
    }

    #[test]
    fn focus_come_by_keyboard_goes_to_the_first_element_tab_reaches() {
        let mut trial = shown(FOCUSED);
        trial.key("Tab");
        assert_ne!(trial.ui.focus(), 0, "the drawing takes focus where it waits for focus moving");
        assert_eq!(color_at(&trial, 25.0, 50.0), 0x0000ff);
    }

    /// Five elements Tab stops at, and `d`, which takes focus but no Tab (`tabindex="-1"`).
    const STOPS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" id="root" viewBox="0 0 100 100" width="100" height="100"><rect id="a" width="20" height="20" tabindex="0"/><rect id="b" x="30" width="20" height="20" tabindex="2"/><rect id="c" x="60" width="20" height="20" tabindex="1"/><rect id="d" y="40" width="20" height="20" tabindex="-1"/><rect id="f" x="40" y="40" width="20" height="20" tabindex="0"/><a id="e" href="#a"><rect y="70" width="20" height="20"/></a></svg>"##;

    #[test]
    fn tab_and_shift_tab_go_between_elements_as_in_edge() {
        // As Edge goes (nav2 `tools/probes/stops.svg`): positive tabindexes, lowest first, then
        // the rest in the file's order; from `d`, or a press on nothing, by the file's order.
        let timeline = Timeline::read(STOPS);
        let listening = timeline.listening();
        let (file, named) = (&timeline.file, |id: &str| timeline.file.named.get(id).copied());
        let id = |at: Option<usize>| at.and_then(|at| file.attribute(at, "id"));
        let order: Vec<&str> = listening.stops.iter().filter_map(|at| file.attribute(*at, "id")).collect();
        assert_eq!(order, ["c", "b", "a", "f", "e"]);
        assert_eq!([id(listening.entry_stop(false)), id(listening.entry_stop(true))], [Some("c"), Some("e")]);
        assert_eq!([false, true].map(|back| id(listening.next_stop(named("b"), back))), [Some("a"), Some("c")]);
        assert_eq!([false, true].map(|back| id(listening.next_stop(named("d"), back))), [Some("f"), Some("c")]);
        assert_eq!([false, true].map(|back| id(listening.next_stop(None, back))), [Some("a"), None]);
        assert_eq!([false, true].map(|back| id(listening.next_stop(named("e"), back))), [None, Some("f")]);
    }

    /// `STOPS` waiting for focus to move within it, in a window with `buttons` before and after.
    fn stops_shown(buttons: bool) -> Trial<AnimatedDrawing> {
        let svg = STOPS.replace("</svg>", r##"<set href="#a" attributeName="fill" to="#0000ff" begin="a.focusin" end="a.focusout"/></svg>"##);
        Trial::new(AnimatedDrawing::new(&svg).unwrap(), move |drawing, ui| {
            if buttons {
                ui.button("before");
            }
            ui.animated_drawing(drawing, Color::BLACK).show();
            if buttons {
                ui.button("after");
            }
        })
    }

    /// What has focus: a button, by its name, or an element of the drawing, by its id.
    fn focused_at(trial: &Trial<AnimatedDrawing>) -> String {
        if let Some(button) = ["before", "after"].into_iter().find(|name| trial.has(Role::Button, name) && trial.find(Role::Button, name).focused) {
            return button.to_string();
        }
        let file = File::read(&trial.app.text);
        trial.app.focused.first().and_then(|at| file.attribute(*at, "id")).unwrap_or_default().to_string()
    }

    /// Where focus goes after each of `keys`.
    fn tabbed(trial: &mut Trial<AnimatedDrawing>, keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (trial.key(key), focused_at(trial)).1).collect()
    }

    #[test]
    fn tab_and_shift_tab_move_between_a_drawings_elements_and_leave_it_at_its_ends() {
        let mut trial = stops_shown(true);
        let went = tabbed(&mut trial, &["Tab"; 7]);
        assert_eq!(went, ["before", "c", "b", "a", "f", "e", "after"]);
        let went = tabbed(&mut trial, &["Shift+Tab"; 7]);
        assert_eq!(went, ["e", "f", "a", "b", "c", "before", "after"]);
        // After a press on nothing in the drawing, Tab goes by the file's order; Shift+Tab leaves.
        let nothing = place(&trial, 90.0, 90.0);
        trial.click(nothing);
        assert_eq!(tabbed(&mut trial, &["Tab"]), ["a"]);
        trial.click(nothing);
        assert_eq!(tabbed(&mut trial, &["Shift+Tab"]), ["before"]);
    }

    #[test]
    fn tab_wraps_round_a_drawing_alone_in_its_window() {
        let mut trial = stops_shown(false);
        let went = tabbed(&mut trial, &["Tab", "Tab", "Tab", "Tab", "Tab", "Tab", "Shift+Tab"]);
        assert_eq!(went, ["c", "b", "a", "f", "e", "c", "e"]);
    }

    #[test]
    fn the_root_svg_has_the_pointer_where_no_shape_is_as_in_edge() {
        // As Edge (nav2 `tools/probes/stops.svg`): a click beside every shape comes to the root.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" id="root" viewBox="0 0 100 100" width="100" height="100"><rect width="50" height="50" fill="#ff0000"><set attributeName="fill" to="#0000ff" begin="root.click"/></rect></svg>"##;
        let mut trial = shown(svg);
        let beside = place(&trial, 75.0, 75.0);
        trial.click(beside);
        assert_eq!(color_at(&trial, 25.0, 25.0), 0x0000ff);
        // Unless it takes no pointer.
        let mut trial = shown(&svg.replace("id=\"root\"", "id=\"root\" pointer-events=\"none\""));
        trial.click(beside);
        assert_eq!(color_at(&trial, 25.0, 25.0), 0xff0000);
    }

    #[test]
    fn the_pointer_counts_within_clip_paths_and_viewports_not_masks_as_in_edge() {
        // As Edge finds them (`own/probes/hits1.svg`, `turned.svg`): a box clip, a round clip, a
        // viewport and a turned one keep the pointer to what they show; a mask does not. A click on
        // each shape colours the square at the bottom right.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><defs><clipPath id="box"><rect width="25" height="50"/></clipPath><clipPath id="round"><circle cx="12.5" cy="67.5" r="8"/></clipPath><mask id="m"><rect x="50" width="25" height="50" fill="white"/></mask></defs>
            <rect id="boxed" width="50" height="50" fill="#808080" clip-path="url(#box)"/><rect id="masked" x="50" width="50" height="50" fill="#808080" mask="url(#m)"/>
            <rect id="rounded" y="55" width="25" height="25" fill="#808080" clip-path="url(#round)"/><svg x="30" y="55" width="20" height="20"><rect id="big" width="40" height="40" fill="#808080"/></svg><g transform="rotate(45 72 70)"><svg x="65" y="63" width="14" height="14"><rect id="turned" width="40" height="40" fill="#808080"/></svg></g>
            <rect x="90" y="90" width="10" height="10" fill="#000000"><set attributeName="fill" to="#ff0000" begin="boxed.click"/><set attributeName="fill" to="#ffff00" begin="rounded.click"/><set attributeName="fill" to="#00ff00" begin="masked.click"/><set attributeName="fill" to="#0000ff" begin="big.click"/><set attributeName="fill" to="#ff00ff" begin="turned.click"/></rect></svg>"##;
        let mut trial = shown(svg);
        for (x, y) in [(40.0, 25.0), (2.0, 57.0), (55.0, 60.0), (82.5, 77.5)] {
            let at = place(&trial, x, y);
            trial.click(at);
            assert_eq!(color_at(&trial, 95.0, 95.0), 0x000000, "clipped away at {x}, {y}");
        }
        for ((x, y), color) in [((10.0, 25.0), 0xff0000), ((12.0, 67.0), 0xffff00), ((90.0, 25.0), 0x00ff00), ((35.0, 60.0), 0x0000ff), ((72.0, 70.0), 0xff00ff)] {
            let at = place(&trial, x, y);
            trial.click(at);
            assert_eq!(color_at(&trial, 95.0, 95.0), color, "at {x}, {y}");
        }
    }

    #[test]
    fn a_click_on_what_a_use_brings_comes_to_the_use_and_passes_through_what_takes_no_pointer() {
        // The left half: a symbol brought by a `<use>`; over it, a shape taking no pointer. The
        // right half: plain.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><defs><symbol id="s"><rect width="50" height="100" fill="#ff0000"/></symbol></defs><use id="u" href="#s"/><rect width="50" height="100" fill="#000000" fill-opacity="0.01" pointer-events="none"/><rect id="r" x="50" width="50" height="100" fill="#00ff00"/><set href="#r" attributeName="fill" to="#0000ff" begin="u.click"/></svg>"##;
        let mut trial = shown(svg);
        let right = place(&trial, 75.0, 50.0);
        trial.click(right);
        assert_eq!(color_at(&trial, 75.0, 50.0), 0x00ff00, "a click on the right half is not on the use");
        let left = place(&trial, 25.0, 50.0);
        trial.click(left);
        assert_eq!(color_at(&trial, 75.0, 50.0), 0x0000ff);
    }
}
