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
//! `a.end-1s`, `a.repeat(2)`, `a.beginEvent`), events (`click`, `a.mouseover+0.5s`) and
//! `accessKey(x)`; `dur`, `repeatCount`, `repeatDur`, `min`, `max`, `restart` and
//! `fill="freeze"`. Each animation's intervals are worked out in time order (see
//! `Timeline::schedule`); of animations of one attribute, the one begun last applies last.
//! Events come from a window only ([`AnimatedDrawing`]): a file drawn at a moment plays what is
//! timed.
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
//! In a window, the pointer is over the topmost shape whose fill or stroke would paint there
//! (clips and masks aside; text as its whole element), unless `pointer-events` is `none`.
//!
//! Deliberate difference from Edge: a fixed `rotate` angle turns the element, as the spec says
//! and Firefox does (Edge ignores it).
use crate::color::Color;
use crate::el::El;
use crate::geom::{Point, pt};
use crate::path::{Path, Step};
use crate::svg::{Drawing, File, Hits, ParsedDrawing, Track, color, number, numbers};
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

/// How many moments (begins, ends, repeats told) a schedule may take per minute worked out: so
/// many, and so many more per animation. Past them it stops: a chain of animations beginning
/// each other at no distance would never end.
const MOMENTS: usize = 1024;
const MOMENTS_EACH: usize = 64;

/// How often a drawing in a window is drawn anew while something in it moves.
const FRAME: Duration = Duration::from_micros(16_667);

/// The events a drawing in a window takes from the pointer (`click` comes with `activate` and
/// `DOMActivate`). Others never come.
const EVENTS: [&str; 10] = ["click", "activate", "DOMActivate", "mousedown", "mouseup", "mouseover", "mouseout", "mouseenter", "mouseleave", "mousemove"];

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
/// it; or a key typed.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Condition<'a> {
    Clock(f32),
    Synced(usize, Moment, f32),
    Event(usize, &'a str, f32),
    Key(char, f32),
}

/// When an animation runs: what begins and ends it, each cycle `simple` seconds long (none: no
/// end, and no change), for `active` seconds in all from each begin (maybe infinite), and
/// whether it may begin again.
struct Timing<'a> {
    begins: Vec<Condition<'a>>,
    ends: Vec<Condition<'a>>,
    simple: Option<f32>,
    active: f32,
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

/// A file's animations, in document order, and each element's parent (the root's is itself).
pub(crate) struct Timeline<'a> {
    file: File<'a>,
    animations: Vec<Animation<'a>>,
    parents: Vec<usize>,
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
        Timeline { file, animations, parents }
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
    /// waiting on it, at most `MOMENTS` and `MOMENTS_EACH` per animation per minute.
    pub(crate) fn schedule(&self, fired: &[Fired], until: f32) -> Schedule {
        let mut scheduling = Scheduling::new(self, fired);
        let minutes = (until / 60.0).ceil().clamp(1.0, 1e6) as usize;
        let most = (MOMENTS + MOMENTS_EACH * self.animations.len()).saturating_mul(minutes);
        let (mut taken, mut unfinished) = (0, false);
        while let Some(due) = scheduling.due.pop() {
            if due.version != scheduling.clocks[due.animation].version {
                continue;
            }
            if due.time > until || taken >= most {
                unfinished = due.time.is_finite();
                break;
            }
            taken += 1;
            scheduling.take(due);
        }
        let changing = self.animations.iter().map(|animation| animation.timing.simple.is_some() && animation.values.len() > 1).collect();
        Schedule { intervals: scheduling.intervals, until, unfinished, changing }
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
        let (mut events, mut keys) = (Vec::new(), false);
        let conditions = self.animations.iter().flat_map(|animation| animation.timing.begins.iter().chain(&animation.timing.ends));
        for condition in conditions {
            match condition {
                Condition::Event(_, name, _) if !events.iter().any(|event| event == name) => events.extend(EVENTS.iter().find(|event| *event == name)),
                Condition::Key(..) => keys = true,
                _ => {}
            }
        }
        let pointer = (0..self.file.elements.len()).map(|at| self.file.own_property(at, "pointer-events").map(|said| said != "none")).collect();
        Listening { parents: self.parents.clone(), pointer, events, keys }
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
/// intervals so far; who waits on each one's moments (the animation waiting, whether as its
/// end, which moment, the offset); and the moments due.
struct Scheduling<'t, 'a> {
    timeline: &'t Timeline<'a>,
    clocks: Vec<Clock>,
    intervals: Vec<Vec<[f32; 2]>>,
    waiting: Vec<Vec<(usize, bool, Moment, f32)>>,
    due: BinaryHeap<Due>,
}

impl<'t, 'a> Scheduling<'t, 'a> {
    /// The start: the times each animation's clock values and the events fired give it, and the
    /// first moment of each due.
    fn new(timeline: &'t Timeline<'a>, fired: &[Fired]) -> Scheduling<'t, 'a> {
        let count = timeline.animations.len();
        let (mut clocks, begun, ended) = (Vec::with_capacity(count), by_app(fired, false), by_app(fired, true));
        for animation in &timeline.animations {
            let mut begins = times(&animation.timing.begins, fired, begun.get(&animation.element));
            let mut ends = times(&animation.timing.ends, fired, ended.get(&animation.element));
            begins.sort_by(f32::total_cmp);
            ends.sort_by(f32::total_cmp);
            clocks.push(Clock { begins, ends, running: None, done: f32::NEG_INFINITY, version: 0 });
        }
        let mut scheduling = Scheduling { timeline, clocks, intervals: vec![Vec::new(); count], waiting: waiting(timeline), due: BinaryHeap::new() };
        (0..count).for_each(|n| scheduling.plan(n));
        scheduling
    }

    /// Takes a moment due.
    fn take(&mut self, due: Due) {
        let n = due.animation;
        match due.next {
            Next::Begin => self.begin(n, due.time),
            Next::End => self.end(n, due.time),
            Next::Restart => {
                self.end(n, due.time);
                self.begin(n, due.time);
            }
            Next::Repeat => self.repeat(n, due.time),
        }
        self.plan(n);
    }

    /// Animation `n`'s next moment, made due (any due before is stale).
    fn plan(&mut self, n: usize) {
        self.clocks[n].version += 1;
        if let Some((time, next)) = self.next(n) {
            self.due.push(Due { time, animation: n, version: self.clocks[n].version, next });
        }
    }

    /// What happens next to animation `n`, and when: running, it ends, begins again or repeats;
    /// else it begins at its next begin time (unless it may never begin again).
    fn next(&self, n: usize) -> Option<(f32, Next)> {
        let (clock, timing) = (&self.clocks[n], &self.timeline.animations[n].timing);
        let Some((begin, told)) = clock.running else {
            let never = timing.restart == Restart::Never && !self.intervals[n].is_empty();
            let at = clock.begins.partition_point(|time| *time < clock.done);
            return clock.begins.get(at).filter(|_| !never).map(|time| (*time, Next::Begin));
        };
        let mut next = (self.end_of(n, begin).unwrap_or(begin), Next::End);
        let again = clock.begins.get(clock.begins.partition_point(|time| *time <= begin)).copied();
        if let Some(again) = again.filter(|again| timing.restart == Restart::Always && *again < next.0) {
            next = (again, Next::Restart);
        }
        let heard = self.repeats_heard(n);
        if let Some(repeat) = timing.simple.map(|simple| begin + simple * (told + 1) as f32).filter(|repeat| heard && *repeat < next.0) {
            next = (repeat, Next::Repeat);
        }
        Some(next)
    }

    /// Whether any animation waits for animation `n` to repeat.
    fn repeats_heard(&self, n: usize) -> bool {
        self.waiting[n].iter().any(|(.., moment, _)| matches!(moment, Moment::Repeat(_)))
    }

    /// Where an interval of animation `n` begun at `begin` ends: at its first end time after
    /// the begin, or when its active time is up, whichever comes first. Nothing where its end
    /// list holds clock values only and none after the begin: it does not begin then (as SMIL
    /// says).
    fn end_of(&self, n: usize, begin: f32) -> Option<f32> {
        let (timing, ends) = (&self.timeline.animations[n].timing, &self.clocks[n].ends);
        let first = ends.get(ends.partition_point(|end| *end <= begin)).copied();
        let timed = !timing.ends.is_empty() && timing.ends.iter().all(|end| matches!(end, Condition::Clock(_)));
        match first {
            Some(end) => Some(end.min(begin + timing.active)),
            None if timed => None,
            None => Some(begin + timing.active),
        }
    }

    /// Begins an interval of animation `n` at `time` (its begin times up to then are used).
    fn begin(&mut self, n: usize, time: f32) {
        let clock = &mut self.clocks[n];
        let used = clock.begins.partition_point(|begin| *begin <= time);
        clock.begins.drain(..used);
        let Some(end) = self.end_of(n, time) else { return };
        self.clocks[n].running = Some((time, 0));
        self.intervals[n].push([time, end]);
        self.tell(n, Moment::Begin, time);
    }

    /// Ends animation `n`'s interval at `time`.
    fn end(&mut self, n: usize, time: f32) {
        let clock = &mut self.clocks[n];
        let Some((begin, _)) = clock.running.take() else { return };
        clock.done = time.max(begin);
        if let Some(last) = self.intervals[n].last_mut() {
            last[1] = time.max(begin);
        }
        self.tell(n, Moment::End, time.max(begin));
    }

    /// Tells animation `n`'s next repeat, at `time`.
    fn repeat(&mut self, n: usize, time: f32) {
        let Some((_, told)) = &mut self.clocks[n].running else { return };
        *told += 1;
        let count = *told;
        self.tell(n, Moment::Repeat(count), time);
    }

    /// Gives the animations waiting on animation `n`'s moment their begin or end times.
    fn tell(&mut self, n: usize, moment: Moment, time: f32) {
        for at in 0..self.waiting[n].len() {
            let (waiting, as_end, heard, offset) = self.waiting[n][at];
            if heard == moment || (heard == Moment::Repeat(0) && matches!(moment, Moment::Repeat(_))) {
                let list = if as_end { &mut self.clocks[waiting].ends } else { &mut self.clocks[waiting].begins };
                list.insert(list.partition_point(|other| *other <= time + offset), time + offset);
                self.plan(waiting);
            }
        }
    }
}

/// Who waits on each animation's moments (see `Scheduling`): for each, the animations whose
/// begin or end lists name one of its moments.
fn waiting(timeline: &Timeline) -> Vec<Vec<(usize, bool, Moment, f32)>> {
    let by_element: HashMap<usize, usize> = timeline.animations.iter().enumerate().map(|(n, animation)| (animation.element, n)).collect();
    let mut waiting = vec![Vec::new(); timeline.animations.len()];
    for (n, animation) in timeline.animations.iter().enumerate() {
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

/// The times a `begin` or `end` list gives from the start: its clock values, those of the
/// events fired that it waits for (each at the event's time plus its offset), and those the app
/// gave (`by_app`).
fn times(list: &[Condition], fired: &[Fired], by_app: Option<&Vec<f32>>) -> Vec<f32> {
    let mut times: Vec<f32> = by_app.cloned().unwrap_or_default();
    for condition in list {
        match condition {
            Condition::Clock(time) => times.push(*time),
            Condition::Event(..) | Condition::Key(..) => times.extend(fired.iter().filter_map(|event| heard(condition, &event.what).map(|offset| event.seconds + offset))),
            Condition::Synced(..) => {}
        }
    }
    times
}

/// When the app began (`Begun`) or ended (`Ended`) each animation, by its element.
fn by_app(fired: &[Fired], ended: bool) -> HashMap<usize, Vec<f32>> {
    let mut times: HashMap<usize, Vec<f32>> = HashMap::new();
    for event in fired {
        let element = match event.what {
            Happened::Begun(element) if !ended => element,
            Happened::Ended(element) if ended => element,
            _ => continue,
        };
        times.entry(element).or_default().push(event.seconds);
    }
    times
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
    fn value_at(&self, intervals: &[[f32; 2]], seconds: f32) -> Option<(f32, String)> {
        let [begin, end] = *intervals.get(intervals.partition_point(|[begin, _]| *begin <= seconds).checked_sub(1)?)?;
        if seconds >= end && !self.freeze {
            return None;
        }
        Some((begin, self.at(seconds - begin, end - begin)?))
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
    let (least, most) = (attr("min").and_then(clock).unwrap_or(0.0), attr("max").and_then(clock).unwrap_or(f32::INFINITY));
    let active = if least <= most { active.max(least).min(most) } else { active };
    let restart = match attr("restart") {
        Some("never") => Restart::Never,
        Some("whenNotActive") => Restart::WhenNotActive,
        _ => Restart::Always,
    };
    Timing { begins, ends, simple, active: active.max(0.0), restart }
}

/// A `begin` or `end` list's entries (see `condition`), those it cannot wait for left out.
fn conditions<'a>(file: &File<'a>, list: &'a str, target: usize) -> Vec<Condition<'a>> {
    list.split(';').filter_map(|entry| condition(file, entry, target)).collect()
}

/// One entry of a `begin` or `end` list: a clock value; `accessKey(x)`; or `id.what` or `what`
/// (at `target`), `what` being `begin`, `end`, `repeat(n)`, `beginEvent`, `endEvent`,
/// `repeatEvent` or an event; each but the first with an optional offset (`+1s`, `- 0.5s`).
/// Nothing for `indefinite`, wall-clock times, unknown events and ids naming nothing.
fn condition<'a>(file: &File<'a>, entry: &'a str, target: usize) -> Option<Condition<'a>> {
    let entry = entry.trim();
    if let Some(time) = clock(entry) {
        return Some(Condition::Clock(time));
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

/// How many of the events a drawing in a window waits for it keeps: past them, the oldest go
/// (and what they began with them). Each frame worked out takes in all it keeps.
const KEPT_EVENTS: usize = 4096;

/// The elements an event at `element` comes to: it and those it lies in (`parents`: each
/// element's parent, the root's being itself), then each `<use>` that brought it (`uses`,
/// innermost first) and those that lies in, each once.
fn bubbling(parents: &[usize], element: usize, uses: &[u32]) -> Vec<usize> {
    let mut chain: Vec<usize> = Vec::new();
    for from in std::iter::once(element).chain(uses.iter().map(|at| *at as usize)) {
        let mut at = from;
        while at < parents.len() && !chain.contains(&at) {
            chain.push(at);
            at = if parents[at] == at { usize::MAX } else { parents[at] };
        }
    }
    chain
}

/// What a drawing in a window needs to know of its file between frames: each element's parent
/// (see `bubbling`) and whether it says it takes the pointer (`pointer-events`: `none`, another
/// value, or nothing said); and the events and keys its animations wait for.
struct Listening {
    parents: Vec<usize>,
    pointer: Vec<Option<bool>>,
    events: Vec<&'static str>,
    keys: bool,
}

impl Listening {
    /// Whether anything waits for the pointer to cross an element, so that every move counts.
    fn follows(&self) -> bool {
        self.events.iter().any(|name| matches!(*name, "mouseover" | "mouseout" | "mouseenter" | "mouseleave" | "mousemove"))
    }

    /// Whether a shape of `element` (brought by `uses`) takes the pointer: as the nearest of
    /// the elements an event there comes to that says.
    fn takes_pointer(&self, element: usize, uses: &[u32]) -> bool {
        bubbling(&self.parents, element, uses).iter().find_map(|at| self.pointer.get(*at).copied().flatten()) != Some(false)
    }
}

/// An SVG file playing its SMIL animation in a window ([`Ui::animated_drawing`]): what is
/// timed, and what waits for the pointer and keys over its box (`click`, `mouseover`,
/// `accessKey(x)` and the other events) or for the app ([`begin_element`](Self::begin_element)).
/// Its clock runs while it shows. Each frame is the file at that moment, read and drawn as any
/// drawing is (see [the module](crate::svg)); only its box is redrawn, and only while something
/// in it changes. [`Ui::drop_animated_drawing`] frees its last frame.
pub struct AnimatedDrawing {
    text: std::sync::Arc<str>,
    serial: u64,
    listening: Listening,
    /// What happened to it that its animations wait for (the last `KEPT_EVENTS`), and how much
    /// ever happened.
    fired: std::collections::VecDeque<Fired>,
    heard: usize,
    /// Its clock (seconds), the window's time it last ran at, and whether it stands still.
    seconds: f32,
    last: Option<Duration>,
    paused: bool,
    /// The schedule last worked out, and how much had happened then.
    schedule: Option<(Schedule, usize)>,
    /// The frame on show: its text, when it is (seconds, and how much had happened), its
    /// drawing in the window, and what lies where in it.
    shown: String,
    shown_for: Option<(f32, usize)>,
    drawing: Option<Drawing>,
    hits: Hits,
    /// The elements under the pointer, as an event there comes to them.
    under: Vec<usize>,
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
            fired: std::collections::VecDeque::new(),
            heard: 0,
            seconds: 0.0,
            last: None,
            paused: false,
            schedule: None,
            shown: String::new(),
            shown_for: None,
            drawing: None,
            hits: Hits::default(),
            under: Vec::new(),
        })
    }

    /// Where its clock stands: seconds since it began.
    pub fn seconds(&self) -> f32 {
        self.seconds
    }

    /// Sets its clock, as `setCurrentTime` does: it shows as it is then, with what happened so
    /// far.
    pub fn seek(&mut self, seconds: f32) {
        self.seconds = if seconds.is_finite() { seconds.max(0.0) } else { 0.0 };
    }

    /// Stops its clock, as `pauseAnimations` does.
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Starts its clock again, as `unpauseAnimations` does.
    pub fn play(&mut self) {
        self.paused = false;
    }

    /// Whether its clock stands still.
    pub fn is_paused(&self) -> bool {
        self.paused
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

    /// Sends a pointer event (`click`, `mousedown`, `mouseover` and the others a window gives)
    /// to the element with this id now, as `dispatchEvent` does: it comes to that element and
    /// to those it lies in. False where no element has the id, or the event is none of those.
    pub fn dispatch(&mut self, id: &str, event: &str) -> bool {
        let element = File::read(&self.text).named.get(id).copied();
        let (Some(element), Some(name)) = (element, EVENTS.iter().find(|name| **name == event)) else { return false };
        self.fire(Happened::Event(name, bubbling(&self.listening.parents, element, &[])));
        true
    }

    /// The drawing as it is at its clock, `width` by `height` pixels, as
    /// [`decode`](crate::svg::decode) draws a file.
    pub fn decode(&mut self, width: usize, height: usize) -> Option<(usize, Vec<u32>)> {
        let text = self.text.clone();
        let timeline = Timeline::read(&text);
        let seconds = self.seconds;
        crate::svg::decode(&timeline.frame(self.schedule(&timeline), seconds), width, height)
    }

    /// Takes in what happened, at the clock's time: the oldest goes once `KEPT_EVENTS` are kept.
    fn fire(&mut self, what: Happened) {
        if self.fired.len() >= KEPT_EVENTS {
            self.fired.pop_front();
        }
        self.fired.push_back(Fired { seconds: self.seconds, what });
        self.heard += 1;
    }

    /// Moves the clock on to the window's time `now`, unless paused.
    fn advance(&mut self, now: Duration) {
        let last = self.last.replace(now);
        if let Some(last) = last.filter(|_| !self.paused) {
            self.seconds += now.saturating_sub(last).as_secs_f32();
        }
    }

    /// The schedule at the clock's time: worked out afresh once something happened, or the clock
    /// nears where the last one ends (it reaches a minute further).
    fn schedule(&mut self, timeline: &Timeline) -> &Schedule {
        let (seconds, heard) = (self.seconds, self.heard);
        if self.schedule.as_ref().is_some_and(|(schedule, known)| *known != heard || seconds >= schedule.until - 1.0) {
            self.schedule = None;
        }
        &self.schedule.get_or_insert_with(|| (timeline.schedule(self.fired.make_contiguous(), seconds + LONGEST), heard)).0
    }

    /// Takes in what the pointer and keys did over its box (`response`), as events at the
    /// elements under the pointer: those the file waits for.
    fn take_input(&mut self, ui: &mut Ui, response: &Response) {
        let under = if response.hovered { self.under_pointer(ui) } else { Vec::new() };
        let mut events = crossed(&self.under, &under);
        let on = |happened: bool| happened && !under.is_empty();
        let input = &ui.input;
        let happened = [(on(input.moved), "mousemove"), (on(response.hovered && input.pressed[0]), "mousedown"), (on(response.hovered && input.released[0]), "mouseup")];
        events.extend(happened.into_iter().filter(|(happened, _)| *happened).map(|(_, name)| (name, under.clone())));
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

    /// The elements under the pointer, the one it is over first, then those an event there comes
    /// to (none where it is over nothing that takes the pointer).
    fn under_pointer(&self, ui: &Ui) -> Vec<usize> {
        let kept = self.drawing.and_then(|drawing| ui.drawings.get(drawing.0));
        let takes = |element: usize, uses: &[u32]| self.listening.takes_pointer(element, uses);
        let found = kept.zip(ui.input.pointer).and_then(|(kept, at)| kept.element_at(&self.hits, at, takes));
        found.map_or_else(Vec::new, |(element, uses)| bubbling(&self.listening.parents, element, uses))
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
        let text = self.text.clone();
        let timeline = Timeline::read(&text);
        let frame = timeline.frame(self.schedule(&timeline), now.0);
        self.shown_for = Some(now);
        if showing.is_some() && frame == self.shown {
            return;
        }
        let Some((mut parsed, hits)) = ParsedDrawing::read_with_hits(&frame) else { return };
        if let Some(within) = noted {
            parsed.note(within);
        }
        let drawing = Drawing(ui.drawings.put(parsed));
        if let Some(last) = self.drawing.replace(drawing) {
            ui.drawings.take(last.0);
        }
        (self.shown, self.hits) = (frame, hits);
    }

    /// How long until the drawing next changes (the schedule being that of the frame on show):
    /// a frame while something moves, else until the next begin or end. Nothing while paused,
    /// or where nothing more happens.
    fn wait(&self) -> Option<Duration> {
        let schedule = self.schedule.as_ref().map(|(schedule, _)| schedule).filter(|_| !self.paused)?;
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
    /// `currentColor`. The pointer over it, and keys while it has focus (where the file waits
    /// for keys, it takes focus), begin and end the animations waiting for them. Only this box
    /// is redrawn per frame, and none while nothing in it changes.
    pub fn animated_drawing(&mut self, drawing: &mut AnimatedDrawing, color: Color) -> El<'_> {
        let key = ("taut animated drawing", drawing.serial);
        let id = self.id(key);
        drawing.advance(self.now);
        let response = self.interact(id);
        if !drawing.listening.events.is_empty() || drawing.listening.keys {
            drawing.take_input(self, &response);
        }
        if response.hovered && drawing.listening.follows() {
            self.follow_pointer();
        }
        drawing.show_frame(self);
        if let Some(wait) = drawing.wait().filter(|_| !self.window_minimized()) {
            self.wake_after(wait);
        }
        self.shows.drawing = Some(ParsedDrawing::show_noted);
        let (shown, keys) = (drawing.drawing, drawing.listening.keys);
        let el = self.el().key(key).sense();
        let el = if keys { el.focusable() } else { el };
        match shown {
            Some(shown) => el.drawing(shown, color),
            None => el.size(0.0, 0.0),
        }
    }

    /// Frees an animated drawing's last frame: where it still shows, nothing does (until it is
    /// shown again).
    pub fn drop_animated_drawing(&mut self, drawing: &mut AnimatedDrawing) {
        if let Some(shown) = drawing.drawing.take() {
            self.drop_drawing(shown);
        }
        (drawing.shown, drawing.shown_for) = (String::new(), None);
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
        // Nothing to wait for: never.
        assert_eq!(["indefinite", "nothing.end", "a.wobble", "wallclock(2026-01-01T00:00:00Z)", "a.repeat(0)", "click+x"].map(read), [None; 6]);
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
        let click_c = Fired { seconds: 2.0, what: Happened::Event("click", bubbling(&timeline.parents, 3, &[])) };
        let runs = timeline.schedule(&[click_c], 100.0).intervals;
        assert_eq!(runs, [vec![], vec![[2.5, 3.5]], vec![]]);
        // On the rectangle, both; a key, the third.
        let click_r = Fired { seconds: 1.0, what: Happened::Event("click", bubbling(&timeline.parents, 2, &[])) };
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
        assert!(!drawing.begin_element("nothing") && !drawing.dispatch("a", "wobble"));
        drawing.seek(1.0);
        assert!(drawing.begin_element("a"));
        drawing.seek(2.0);
        let (_, pixels) = drawing.decode(4, 4).unwrap();
        assert_eq!(pixels[5], 0xff800080);
        drawing.seek(9.0);
        assert_eq!(drawing.decode(4, 4).unwrap().1[5], 0xff0000ff);
    }
}

#[cfg(test)]
mod pointer_tests {
    use super::*;
    use crate::testing::Trial;

    #[test]
    fn a_click_on_what_a_use_brings_comes_to_the_use_and_passes_through_what_takes_no_pointer() {
        // The left half: a symbol brought by a `<use>`; over it, a shape taking no pointer. The
        // right half: plain.
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="100" height="100"><defs><symbol id="s"><rect width="50" height="100" fill="#ff0000"/></symbol></defs><use id="u" href="#s"/><rect width="50" height="100" fill="#000000" fill-opacity="0.01" pointer-events="none"/><rect id="r" x="50" width="50" height="100" fill="#00ff00"/><set href="#r" attributeName="fill" to="#0000ff" begin="u.click"/></svg>"##;
        let mut trial = Trial::new(AnimatedDrawing::new(svg).unwrap(), |drawing, ui| _ = ui.animated_drawing(drawing, Color::BLACK).show());
        let within = trial.app.drawing.and_then(|drawing| trial.ui.drawings.get(drawing.0)).and_then(ParsedDrawing::noted).unwrap();
        let right = |trial: &Trial<AnimatedDrawing>| trial.screen.pixels[(within.y as usize + 50) * trial.screen.width + within.x as usize + 75] & 0xff_ffff;
        trial.click(pt(within.x + 75.0, within.y + 50.0));
        assert_eq!(right(&trial), 0x00ff00, "a click on the right half is not on the use");
        trial.click(pt(within.x + 25.0, within.y + 50.0));
        assert_eq!(right(&trial), 0x0000ff);
    }
}
