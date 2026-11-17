//! What the person arranged, as text: where a panel floats, how a dock's panels lie, a table's
//! shown columns and widths, where a window was left, where a pan-and-zoom view looks and how
//! near. Each value is written with `to_string()` and read with `parse()`, so an app that
//! reopens as the person left it writes a few lines to a file of its own:
//!
//! ```ignore
//! std::fs::write(file, format!("{}\n{}\n", app.dock, app.table))?;
//! // and the next day:
//! let mut lines = saved.lines();
//! app.dock = lines.next().and_then(|line| line.parse().ok()).unwrap_or_else(first_dock);
//! app.table = lines.next().and_then(|line| line.parse().ok()).unwrap_or_default();
//! ```
//!
//! Each value is one line of plain letters and numbers. A line the library did not write is
//! refused whole (`parse` errs): the app starts as on its first run, never half laid out. A
//! dock from another version of the app, with other panels, is brought up to date with
//! [`Dock::only`].
use crate::dock::{Dock, DockNode, Home};
use crate::geom::rect;
use crate::pan_zoom::PanZoom;
use crate::panel::Panel;
use crate::platform::WindowPlacement;
use crate::second_window::SecondWindow;
use crate::table::Table;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

/// The numbers in a text, separated by `between`. Nothing if any is not a number, or is
/// infinite or NaN.
fn numbers(text: &str, between: char) -> Option<Vec<f32>> {
    let number = |written: &str| written.parse::<f32>().ok().filter(|number| number.is_finite());
    text.split(between).filter(|written| !written.is_empty()).map(number).collect()
}

/// Whole, non-negative numbers written the same way. (Read as integers: an `f32` holds them
/// exactly only up to 2^24.)
fn counts(text: &str, between: char) -> Option<Vec<usize>> {
    text.split(between).filter(|written| !written.is_empty()).map(|written| written.parse::<usize>().ok()).collect()
}

/// A name as it stands among a line's commas and bars: each such mark in it is written as `%`
/// and its number. An empty name is written as `%` alone, else it would be an empty list entry,
/// which is skipped, and the panel lost.
fn marked(name: &str) -> String {
    if name.is_empty() {
        return "%".to_string();
    }
    let mut written = String::with_capacity(name.len());
    for letter in name.chars() {
        match letter {
            '%' | ',' | ';' | '|' | '\n' | '\r' => written.push_str(&format!("%{:02X}", letter as u32)),
            _ => written.push(letter),
        }
    }
    written
}

/// The name such text stands for; nothing if a `%` is followed by no number (except a lone `%`,
/// the empty name).
fn unmarked(written: &str) -> Option<String> {
    if written == "%" {
        return Some(String::new());
    }
    let (mut name, mut rest) = (String::with_capacity(written.len()), written);
    while let Some(at) = rest.find('%') {
        name.push_str(&rest[..at]);
        let mark = rest.get(at + 1..at + 3).and_then(|number| u8::from_str_radix(number, 16).ok())?;
        name.push(mark as char);
        rest = &rest[at + 3..];
    }
    name.push_str(rest);
    Some(name)
}

/// A panel's place, size, open and folded: six numbers.
impl Display for Panel {
    fn fmt(&self, to: &mut Formatter) -> fmt::Result {
        let Panel { rect, open, folded } = self;
        write!(to, "{} {} {} {} {} {}", rect.x, rect.y, rect.w, rect.h, *open as u8, *folded as u8)
    }
}

impl FromStr for Panel {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let [x, y, w, h, open, folded] = numbers(text, ' ').ok_or(())?[..] else { return Err(()) };
        // No size is negative: such a line was not written here.
        if w < 0.0 || h < 0.0 {
            return Err(());
        }
        Ok(Panel { rect: rect(x, y, w, h), open: open != 0.0, folded: folded != 0.0 })
    }
}

/// A second window's size, least size, whether open, and its place once it has been on screen:
/// five numbers, or seven.
impl Display for SecondWindow {
    fn fmt(&self, to: &mut Formatter) -> fmt::Result {
        write!(to, "{} {} {} {} {}", self.width, self.height, self.min_width, self.min_height, self.open as u8)?;
        self.position.map_or(Ok(()), |(x, y)| write!(to, " {x} {y}"))
    }
}

impl FromStr for SecondWindow {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let all = numbers(text, ' ').ok_or(())?;
        let (&[width, height, min_width, min_height, open], at) = all.split_first_chunk::<5>().ok_or(())?;
        if [width, height, min_width, min_height].iter().any(|size| *size < 0.0) {
            return Err(());
        }
        let at = match at {
            [] => None,
            [x, y] => Some((*x as i32, *y as i32)),
            _ => return Err(()),
        };
        // Its kind and first screen are the app's to say: they are not written.
        Ok(SecondWindow { position: at, open: open != 0.0, ..SecondWindow::new(width, height).min_size(min_width, min_height) })
    }
}

/// Where a pan-and-zoom view looks, and its zoom: three numbers.
impl Display for PanZoom {
    fn fmt(&self, to: &mut Formatter) -> fmt::Result {
        write!(to, "{} {} {}", self.x, self.y, self.zoom)
    }
}

impl FromStr for PanZoom {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let [x, y, zoom] = numbers(text, ' ').ok_or(())?[..] else { return Err(()) };
        // A view's zoom is always positive.
        (zoom > 0.0).then_some(PanZoom { x, y, zoom }).ok_or(())
    }
}

/// A window's place, size, and whether it is maximized: five numbers.
impl Display for WindowPlacement {
    fn fmt(&self, to: &mut Formatter) -> fmt::Result {
        write!(to, "{} {} {} {} {}", self.position.0, self.position.1, self.width, self.height, self.maximized as u8)
    }
}

impl FromStr for WindowPlacement {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let [x, y, width, height, maximized] = numbers(text, ' ').ok_or(())?[..] else { return Err(()) };
        if width < 0.0 || height < 0.0 {
            return Err(());
        }
        Ok(WindowPlacement { position: (x as i32, y as i32), width, height, maximized: maximized != 0.0 })
    }
}

/// What the person did to a table, as five lists separated by bars: dragged column widths; the
/// sort (column numbers, negative for descending); column order; hidden columns; how many stay
/// in view.
impl Display for Table {
    fn fmt(&self, to: &mut Formatter) -> fmt::Result {
        let list = |numbers: &mut dyn Iterator<Item = String>| numbers.collect::<Vec<_>>().join(",");
        let sort = list(&mut self.sort.iter().map(|(column, down)| format!("{}{column}", if *down { "-" } else { "" })));
        let (widths, order, hidden) = (list(&mut self.widths.iter().map(f32::to_string)), list(&mut self.order.iter().map(usize::to_string)), list(&mut self.hidden.iter().map(usize::to_string)));
        write!(to, "{widths}|{sort}|{order}|{hidden}|{}", self.frozen)
    }
}

impl FromStr for Table {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let lists: Vec<&str> = text.split('|').collect();
        let [widths, sort, order, hidden, kept] = lists[..] else { return Err(()) };
        // A descending column has a minus before its number.
        let by = |written: &str| counts(written.trim_start_matches('-'), ',').and_then(|column| Some((*column.first()?, written.starts_with('-'))));
        let sort: Option<Vec<(usize, bool)>> = sort.split(',').filter(|written| !written.is_empty()).map(by).collect();
        let [kept] = counts(kept, ',').ok_or(())?[..] else { return Err(()) };
        Ok(Table { widths: numbers(widths, ',').ok_or(())?, sort: sort.ok_or(())?, order: counts(order, ',').ok_or(())?, hidden: counts(hidden, ',').ok_or(())?, frozen: kept, focused_cell: None })
    }
}

/// How a dock's panels lie. Its places, separated by semicolons: `t`, the front tab, and the
/// panels' names; or `s`, whether the split is side by side, the first part's share, and the
/// numbers of the two parts; or `g` for a place that is gone. Then a bar and the floating
/// panels, each as its name and a panel's text. Then, if any, three more lists after bars:
/// closed panels; the panel filling the dock; and where each floating or closed panel returns
/// to: its name, the panel beside it, the side (0 to 3: left, top, right, bottom; 4: behind its
/// tabs) and its share.
impl Display for Dock {
    fn fmt(&self, to: &mut Formatter) -> fmt::Result {
        let place = |place: &DockNode| match place {
            DockNode::Tabs { names, front } => names.iter().fold(format!("t,{front}"), |written, name| written + "," + &marked(name)),
            DockNode::Split { across, share, first, second } => format!("s,{},{share},{first},{second}", *across as u8),
            DockNode::Gone => "g".to_string(),
        };
        let floats = |(name, panel): &(String, Panel)| format!("{},{}", marked(name), panel.to_string().replace(' ', ","));
        let (places, floating): (Vec<String>, Vec<String>) = (self.nodes.iter().map(place).collect(), self.floating.iter().map(floats).collect());
        write!(to, "{}|{}", places.join(";"), floating.join(";"))?;
        if self.closed.is_empty() && self.maximized.is_none() && self.homes.is_empty() {
            return Ok(());
        }
        let homes: Vec<String> = self.homes.iter().map(|home| format!("{},{},{},{}", marked(&home.name), marked(&home.beside), home.side, home.share)).collect();
        let closed: Vec<String> = self.closed.iter().map(|name| marked(name)).collect();
        write!(to, "|{}|{}|{}", closed.join(","), self.maximized.as_deref().map_or(String::new(), marked), homes.join(";"))
    }
}

/// One place of a dock as written. Nothing if it is no place, or names a place beyond the
/// `count` there are.
fn place(written: &str, count: usize) -> Option<DockNode> {
    let (kind, rest) = written.split_once(',').unwrap_or((written, ""));
    match kind {
        "g" if rest.is_empty() => Some(DockNode::Gone),
        "s" => {
            let [across, share, first, second] = numbers(rest, ',')?[..] else { return None };
            let [first, second] = [counts(&first.to_string(), ',')?[0], counts(&second.to_string(), ',')?[0]];
            (first < count && second < count && (0.0..=1.0).contains(&share)).then_some(DockNode::Split { across: across != 0.0, share, first, second })
        }
        "t" => {
            let (front, names) = rest.split_once(',').unwrap_or((rest, ""));
            let names: Vec<String> = names.split(',').filter(|name| !name.is_empty()).map(unmarked).collect::<Option<_>>()?;
            let front = counts(front, ',')?.first().copied().filter(|front| *front < names.len().max(1))?;
            Some(DockNode::Tabs { names, front })
        }
        _ => None,
    }
}

/// Where a panel returns to, as written; nothing if malformed.
fn home(written: &str) -> Option<Home> {
    let [name, beside, side, share] = written.split(',').collect::<Vec<_>>()[..] else { return None };
    let (Some(&[side]), Some(&[share])) = (counts(side, ',').as_deref(), numbers(share, ',').as_deref()) else { return None };
    let fits = side <= 4 && (0.0..=1.0).contains(&share);
    fits.then_some(Home { name: unmarked(name)?, beside: unmarked(beside)?, side, share })
}

/// Whether a dock's places form a tree: every used place but the first is one of the two halves
/// of exactly one other, the first is none, and all are reached from the first. (A cycle would
/// be laid out forever; an orphan's panels would get no room.)
fn hang_together(places: &[DockNode]) -> bool {
    let mut halves = vec![0u32; places.len()];
    for place in places {
        let DockNode::Split { first, second, .. } = place else { continue };
        (halves[*first], halves[*second]) = (halves[*first] + 1, halves[*second] + 1);
    }
    let used = |place: &DockNode| *place != DockNode::Gone;
    if places.iter().zip(&halves).enumerate().any(|(at, (place, halves))| *halves != u32::from(at > 0 && used(place))) {
        return false;
    }
    // No place is half of two others, so walking from the first never meets one twice.
    let (mut reached, mut waiting) = (0, Vec::from_iter(places.first().map(|_| 0)));
    while let Some(at) = waiting.pop() {
        reached += 1;
        let DockNode::Split { first, second, .. } = places[at] else { continue };
        waiting.extend([first, second]);
    }
    reached == places.iter().filter(|place| used(place)).count()
}

impl FromStr for Dock {
    type Err = ();
    fn from_str(text: &str) -> Result<Self, ()> {
        let lists: Vec<&str> = text.split('|').collect();
        let ([places, floating], [closed, filled, homes]) = match lists[..] {
            [places, floating] => ([places, floating], [""; 3]),
            [places, floating, closed, filled, homes] => ([places, floating], [closed, filled, homes]),
            _ => return Err(()),
        };
        let written: Vec<&str> = places.split(';').filter(|written| !written.is_empty()).collect();
        let places = written.iter().map(|one| place(one, written.len())).collect::<Option<Vec<DockNode>>>().ok_or(())?;
        // A floating panel: its name, then its six numbers, separated by commas.
        let floats = |written: &str| {
            let (name, panel) = written.split_once(',')?;
            Some((unmarked(name)?, panel.replace(',', " ").parse::<Panel>().ok()?))
        };
        let floating = floating.split(';').filter(|written| !written.is_empty()).map(floats).collect::<Option<Vec<(String, Panel)>>>().ok_or(())?;
        let closed = closed.split(',').filter(|written| !written.is_empty()).map(unmarked).collect::<Option<Vec<String>>>().ok_or(())?;
        let filled = Some(filled).filter(|written| !written.is_empty()).map(|written| unmarked(written).ok_or(())).transpose()?;
        let homes = homes.split(';').filter(|written| !written.is_empty()).map(home).collect::<Option<Vec<Home>>>().ok_or(())?;
        // No panel appears twice (the second would be an empty tab), or is both shown and
        // closed; the panel filling the dock is docked; and only a floating or closed panel has
        // a place to return to, one at most.
        let dock = Dock { nodes: places, floating, closed, maximized: filled, homes };
        let names: Vec<&str> = dock.panels().chain(dock.closed.iter().map(String::as_str)).collect();
        let once = |names: &[&str]| names.iter().enumerate().all(|(at, name)| !names[..at].contains(name));
        let docked = names.len() - dock.floating.len() - dock.closed.len();
        let fills = dock.maximized.as_ref().is_none_or(|filled| names[..docked].contains(&filled.as_str()));
        let away: Vec<&str> = dock.homes.iter().map(|home| home.name.as_str()).collect();
        let homes = once(&away) && away.iter().all(|name| names[docked..].contains(name));
        if once(&names) && fills && homes && hang_together(&dock.nodes) { Ok(dock) } else { Err(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each value comes back from its text unchanged, and its text is one line.
    fn there_and_back<T: Display + FromStr + PartialEq + std::fmt::Debug>(value: T) -> String {
        let written = value.to_string();
        assert!(!written.contains('\n'), "{written:?}");
        assert_eq!(written.parse::<T>().ok().as_ref(), Some(&value), "{written:?}");
        written
    }

    #[test]
    fn what_the_person_arranged_is_written_as_a_line_and_read_back_as_it_was() {
        assert_eq!(there_and_back(Panel { folded: true, ..Panel::new(40.5, 60.0, 260.0, 320.0) }), "40.5 60 260 320 1 1");
        assert_eq!(there_and_back(SecondWindow { open: false, ..SecondWindow::new(420.0, 300.0) }), "420 300 200 120 0");
        assert_eq!(there_and_back(SecondWindow::new(420.0, 300.0).position(-12, 640)), "420 300 200 120 1 -12 640");
        assert_eq!(there_and_back(WindowPlacement { position: (300, 200), width: 1000.0, height: 640.5, maximized: true }), "300 200 1000 640.5 1");
        assert_eq!(there_and_back(Table::default()), "||||0");
        assert_eq!(there_and_back(Table { widths: vec![0.0, 90.5], sort: vec![(2, true), (0, false)], order: vec![1, 0, 2], hidden: vec![3], frozen: 1, focused_cell: None }), "0,90.5|-2,0|1,0,2|3|1");
        // A dock: two places side by side, the second split again; and a floating panel.
        let mut dock = Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Notes"]).above(Dock::of(&["Log"]), 0.7), 0.25);
        dock.floating.push(("Tools".to_string(), Panel::new(10.0, 20.0, 200.0, 100.0)));
        assert_eq!(there_and_back(dock), "s,1,0.25,1,2;t,0,Files;s,0,0.7,3,4;t,0,Editor,Notes;t,0,Log|Tools,10,20,200,100,1,0");
        assert_eq!(there_and_back(Dock::default()), "|");
        // A name may hold anything, also the marks the line is made of.
        let odd = Dock { nodes: vec![DockNode::Tabs { names: vec!["a,b;c|d%e".into(), "plain name".into(), "Größe".into()], front: 1 }, DockNode::Gone], ..Default::default() };
        assert_eq!(there_and_back(odd), "t,1,a%2Cb%3Bc%7Cd%25e,plain name,Größe;g|");
        // And an empty name: as a tab, closed, filling the dock, floating, and as a place to
        // return to and the panel beside it.
        let mut empty = Dock::of(&["", "Files"]);
        assert_eq!(there_and_back(empty.clone()), "t,0,%,Files|");
        empty.maximized = Some(String::new());
        assert_eq!(there_and_back(empty), "t,0,%,Files|||%|");
        let mut away = Dock::of(&["Files"]);
        away.closed.push(String::new());
        assert_eq!(there_and_back(away.clone()), "t,0,Files||%||");
        away.closed.clear();
        away.floating.push((String::new(), Panel::new(10.0, 20.0, 200.0, 100.0)));
        away.homes.push(Home { name: String::new(), beside: "Files".into(), side: 2, share: 0.5 });
        assert_eq!(there_and_back(away), "t,0,Files|%,10,20,200,100,1,0|||%,Files,2,0.5");
        let beside = Dock { closed: vec!["Files".into()], homes: vec![Home { name: "Files".into(), beside: String::new(), side: 4, share: 1.0 }], ..Dock::of(&[""]) };
        assert_eq!(there_and_back(beside), "t,0,%||Files||Files,%,4,1");
    }

    #[test]
    fn panels_that_were_closed_or_float_or_fill_the_dock_are_written_after_the_rest() {
        let mut dock = Dock::of(&["Files"]).beside(Dock::of(&["Editor", "Notes"]).above(Dock::of(&["Log"]), 0.7), 0.25);
        dock.float("Log", Panel::new(10.0, 20.0, 200.0, 100.0));
        dock.close("Notes");
        dock.maximized = Some("Editor".into());
        // Log returns under Editor with its old share, Notes behind Editor's tabs.
        let written = "s,1,0.25,1,2;t,0,Files;t,0,Editor;g;g|Log,10,20,200,100,1,0|Notes|Editor|Log,Editor,3,0.3;Notes,Editor,4,0.5";
        assert_eq!(there_and_back(dock), written);
        // A line written before these were kept still reads.
        let old: Dock = "s,1,0.25,1,2;t,0,Files;t,0,Editor,Notes|".parse().unwrap();
        assert!(old.closed.is_empty() && old.maximized.is_none() && old.homes.is_empty() && old.panels().count() == 3);
        assert_eq!(there_and_back(old), "s,1,0.25,1,2;t,0,Files;t,0,Editor,Notes|");
        // A closed panel with nowhere to return to is still a panel.
        assert!("t,0,A||B||".parse::<Dock>().is_ok());
        // Refused: a panel filling the dock but not docked; a panel shown and closed; a return
        // place with no side or share, for a docked panel, or twice; and a line of four lists.
        let bad = ["t,0,A|||B|", "t,0,A||B|B|", "t,0,A||A||", "t,0,A|C,1,2,3,4,1,0|C||", "t,0,A||B||B,A,9,0.5", "t,0,A||B||B,A,4,2", "t,0,A||B||A,B,4,0.5"];
        for bad in bad.iter().chain(&["t,0,A||B||B,A,4,0.5;B,A,4,0.5", "t,0,A|||", "t,0,A||B||B,A,4", "t,0,A||B%Z||"]) {
            assert!(bad.parse::<Dock>().is_err(), "{bad:?} was taken for a dock");
        }
    }

    #[test]
    fn a_line_that_is_not_what_was_written_is_refused_whole() {
        for bad in ["", "1 2 3", "1 2 3 4 5 6 7", "a b c d e f", "1 2 3 inf 1 0", "1 2 3 NaN 1 0"] {
            assert!(bad.parse::<Panel>().is_err(), "{bad:?} was taken for a panel");
        }
        for bad in ["", "1 2 3 4", "1 2 3 4 5 6", "1 2 3 4 5 6 7 8"] {
            assert!(bad.parse::<SecondWindow>().is_err(), "{bad:?} was taken for a window");
        }
        assert!("1 2 3 4".parse::<WindowPlacement>().is_err());
        // No negative size for a panel, a second window or a window: a place may be negative.
        for bad in ["1 2 -3 4 1 0", "1 2 3 -4 1 0"] {
            assert!(bad.parse::<Panel>().is_err(), "{bad:?} was taken for a panel");
        }
        for bad in ["-420 300 200 120 1", "420 -300 200 120 1", "420 300 -200 120 1", "420 300 200 -120 1 -12 640"] {
            assert!(bad.parse::<SecondWindow>().is_err(), "{bad:?} was taken for a window");
        }
        for bad in ["300 200 -1000 640 0", "300 200 1000 -640 0"] {
            assert!(bad.parse::<WindowPlacement>().is_err(), "{bad:?} was taken for a window's place");
        }
        assert!("-300 -200 1000 640 0".parse::<WindowPlacement>().is_ok() && "-5 -6 140 72 1 0".parse::<Panel>().is_ok());
        for bad in ["", "|||", "x||||0", "|1.5|||0", "||-1|||0", "||||one", "||||1,2"] {
            assert!(bad.parse::<Table>().is_err(), "{bad:?} was taken for a table");
        }
        // A dock whose places name missing places, whose front tab is not one of its tabs, or
        // whose share is out of range would be laid out wrongly: refused.
        for bad in ["", "t,0,Files", "s,1,0.25,1,2;t,0,Files|", "t,3,Files|", "s,1,2.5,1,2;t,0,A;t,0,B|", "q|", "t,0,Files%Z|", "t,0,Files|Tools,1,2,3"] {
            assert!(bad.parse::<Dock>().is_err(), "{bad:?} was taken for a dock");
        }
        // Nor one whose places form a cycle (laid out forever), hang nowhere, or are gone yet
        // named; nor one with a panel twice.
        let rings = ["s,1,0.5,0,0|", "s,1,0.5,1,1;t,0,A|", "s,1,0.5,1,2;s,0,0.5,1,2;t,0,A|", "t,0,A;s,1,0.5,2,3;s,1,0.5,1,4;t,0,B;t,0,C|"];
        let loose = ["t,0,A;t,0,B|", "s,1,0.5,1,2;t,0,A;g|", "g|", "t,0,A;g;t,0,B|"];
        let twice = ["t,0,A,A|", "s,1,0.5,1,2;t,0,A;t,0,A|", "t,0,A|A,1,2,3,4,1,0", "t,0,A|B,1,2,3,4,1,0;B,1,2,3,4,1,0"];
        for bad in rings.iter().chain(&loose).chain(&twice) {
            assert!(bad.parse::<Dock>().is_err(), "{bad:?} was taken for a dock");
        }
        // Places that are gone and named by nothing are what a dock has after a panel left.
        assert!("s,1,0.5,2,3;g;t,0,A;t,0,B|".parse::<Dock>().is_ok());
    }
}
