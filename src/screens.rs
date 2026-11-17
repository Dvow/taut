//! The person's screens: where each lies, its size, its work area and scale; which one a window
//! is on; and where a window opens on them.
//!
//! The desktop is all screens side by side as the person arranged them, in pixels from the top
//! left of the main screen: a screen to its left or above lies at negative places. Whoever
//! shows the windows asks the system for the screens at start and whenever they change, and
//! tells the interface ([`set_screens`](Ui::set_screens)). All windows of a program share one
//! list, main screen first.
//!
//! Where a window opens is worked out here, the same on every system (`window_origin`), from
//! what its app asked: at a place (kept if a screen still shows it, else moved onto the nearest
//! screen); in the middle of a screen; or in the middle of its owner. It always lies within its
//! screen's work area as far as it fits. Only a window whose owner is on no screen stays with
//! its owner, where nobody sees either.
use crate::geom::{Point, Rect, pt, rect};
use crate::ui::Ui;

/// One of the person's screens, as the system has it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Screen {
    /// Where it lies on the desktop and its size, in desktop pixels.
    pub rect: Rect,
    /// Its work area, in the same pixels: all but what the system keeps for its bars (task bar,
    /// dock, panel).
    pub work_area: Rect,
    /// Pixels per unit: 1 at 100%, 1.5 at 150%. A window moved onto it is drawn at this scale
    /// and keeps its size in units.
    pub scale: f32,
}

/// Where an app asked a window to open, as handed on by whoever shows it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum OpenAt {
    /// Wherever the system, or the windows already open, would put it.
    #[default]
    Any,
    /// With the top left of its frame at this desktop place.
    At(i32, i32),
    /// In the middle of this screen, or of the main one if there is no such screen.
    Middle(usize),
    /// In the middle of its owner, which lies here on the desktop, frame and all.
    Over(Rect),
}

/// How much of a window's top strip must lie on a screen for its place to count as there, in
/// pixels: enough of the title to grab.
const TITLE: Rect = rect(0.0, 0.0, 160.0, 30.0);

/// Which screen an area of the desktop is on: the one with most of it, or if none has any, the
/// nearest. Nothing if there are no screens.
pub(crate) fn nearest(screens: &[Screen], area: Rect) -> Option<usize> {
    let covered = |screen: &Screen| {
        let part = screen.rect.intersect(area);
        part.w * part.h
    };
    // How far an area is from a screen: along each axis, the gap between their edges.
    let apart = |screen: &Screen| {
        let gap = |from: f32, to: f32, start: f32, end: f32| (start - to).max(from - end).max(0.0);
        gap(area.x, area.right(), screen.rect.x, screen.rect.right()).hypot(gap(area.y, area.bottom(), screen.rect.y, screen.rect.bottom()))
    };
    let most = (0..screens.len()).max_by(|a, b| covered(&screens[*a]).total_cmp(&covered(&screens[*b])).then(b.cmp(a)))?;
    if covered(&screens[most]) > 0.0 {
        return Some(most);
    }
    (0..screens.len()).min_by(|a, b| apart(&screens[*a]).total_cmp(&apart(&screens[*b])))
}

/// Where the top left of a window's frame goes to open as `asked`, and on which screen, so the
/// system can create it there at that screen's scale. `size` gives the window's size with
/// frame, in pixels at a scale. Nothing if the app asked for no place or there are no screens:
/// then the system places it.
pub(crate) fn window_origin(screens: &[Screen], asked: OpenAt, size: impl Fn(f32) -> [f32; 2]) -> Option<(usize, Point)> {
    let (on, middle) = match asked {
        OpenAt::Any => return None,
        OpenAt::At(x, y) => (nearest(screens, TITLE.moved(x as f32, y as f32))?, None),
        OpenAt::Middle(nth) => (if nth < screens.len() { nth } else { 0 }, None),
        OpenAt::Over(owner) => (nearest(screens, owner)?, Some(pt(owner.x + owner.w / 2.0, owner.y + owner.h / 2.0))),
    };
    let screen = *screens.get(on)?;
    let [wide, high] = size(screen.scale);
    let work = screen.work_area;
    let middle = middle.unwrap_or(pt(work.x + work.w / 2.0, work.y + work.h / 2.0));
    let corner = match asked {
        OpenAt::At(x, y) => pt(x as f32, y as f32),
        _ => pt((middle.x - wide / 2.0).round(), (middle.y - high / 2.0).round()),
    };
    // A window over an owner on no screen stays with it, out of sight too.
    if matches!(asked, OpenAt::Over(owner) if screen.rect.intersect(owner).is_empty()) {
        return Some((on, corner));
    }
    let x = corner.x.min(work.right() - wide).max(work.x);
    Some((on, pt(x, corner.y.min(work.bottom() - high).max(work.y))))
}

/// The person's screens: how many, where, and how big.
impl Ui {
    /// The person's screens, main one first: place on the desktop, work area and scale. The
    /// interface runs when they change (a screen plugged in or removed, a task bar moved).
    /// Empty where no system has said, as for a surface.
    pub fn screens(&self) -> &[Screen] {
        &self.screens
    }

    /// Which screen the window is on: the index (as in `screens`) of the one with most of it,
    /// or the nearest. Nothing until the system has said where the window is.
    pub fn screen(&self) -> Option<usize> {
        let place = self.place?;
        nearest(&self.screens, rect(place.position.0 as f32, place.position.1 as f32, place.width * self.scale, place.height * self.scale))
    }

    /// For the code that runs windows: the person's screens as the system has them, main one
    /// first. Says whether that is news (then the interface should run, for the app to hear).
    pub fn set_screens(&mut self, screens: Vec<Screen>) -> bool {
        let news = self.screens != screens;
        self.screens = screens;
        news
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalClipboard;
    use crate::geom::Size;
    use crate::platform::WindowPlacement;
    use crate::text::BuiltInFont;

    /// Three screens: the main one, 1,920 by 1,080 at 100%, a 40-pixel task bar along its
    /// bottom; a 2,560 by 1,440 one at 150% to its right, 200 lower; and a small one at 100% to
    /// the main one's left, 300 higher, with a 30-pixel bar down its left side.
    fn desk() -> Vec<Screen> {
        vec![
            Screen { rect: rect(0.0, 0.0, 1920.0, 1080.0), work_area: rect(0.0, 0.0, 1920.0, 1040.0), scale: 1.0 },
            Screen { rect: rect(1920.0, 200.0, 2560.0, 1440.0), work_area: rect(1920.0, 200.0, 2560.0, 1440.0), scale: 1.5 },
            Screen { rect: rect(-1280.0, -300.0, 1280.0, 1024.0), work_area: rect(-1250.0, -300.0, 1250.0, 1024.0), scale: 1.0 },
        ]
    }

    /// A window 400 by 300 units inside, with an 8-pixel frame on each side and a 30-pixel
    /// title at 100%, both growing with the scale.
    fn framed(scale: f32) -> [f32; 2] {
        [((400.0 + 16.0) * scale).round(), ((300.0 + 38.0) * scale).round()]
    }

    #[test]
    fn an_area_is_on_the_screen_that_has_the_most_of_it_or_else_on_the_nearest() {
        let screens = desk();
        // Wholly on one; across two, on the one with more of it; on none, the nearest.
        assert_eq!(nearest(&screens, rect(100.0, 100.0, 400.0, 300.0)), Some(0));
        assert_eq!(nearest(&screens, rect(1800.0, 300.0, 400.0, 300.0)), Some(1));
        assert_eq!(nearest(&screens, rect(1600.0, 300.0, 400.0, 300.0)), Some(0));
        assert_eq!(nearest(&screens, rect(-200.0, 500.0, 400.0, 300.0)), Some(0), "on the main one, which has more of it");
        assert_eq!(nearest(&screens, rect(-200.0, -600.0, 100.0, 100.0)), Some(2), "above the small screen, and nearest to it");
        assert_eq!(nearest(&screens, rect(5000.0, 900.0, 100.0, 100.0)), Some(1), "beyond the sharp screen's right edge");
        assert_eq!(nearest(&screens, rect(500.0, 1500.0, 100.0, 100.0)), Some(0), "below the main screen");
        assert_eq!(nearest(&[], rect(0.0, 0.0, 10.0, 10.0)), None);
        // An empty area is on the screen it lies on, or the nearest.
        assert_eq!(nearest(&screens, Rect::EMPTY), Some(0));
    }

    #[test]
    fn a_place_a_screen_still_has_is_kept_and_any_other_is_moved_onto_the_nearest_screen() {
        let screens = desk();
        let at = |x, y| window_origin(&screens, OpenAt::At(x, y), framed);
        // Where it was, on its screen, at that screen's scale.
        assert_eq!(at(300, 200), Some((0, pt(300.0, 200.0))));
        assert_eq!(at(2500, 600), Some((1, pt(2500.0, 600.0))));
        assert_eq!(at(-1000, -100), Some((2, pt(-1000.0, -100.0))));
        // Partly over its screen's edge: moved as little as needed to lie on it. The main
        // screen's task bar is not for windows.
        assert_eq!(at(1700, 900), Some((0, pt(1920.0 - 416.0, 1040.0 - 338.0))));
        assert_eq!(at(4400, 1500), Some((1, pt(1920.0 + 2560.0 - 624.0, 200.0 + 1440.0 - 507.0))));
        // On no screen that exists now (one was removed): onto the nearest, wholly.
        assert_eq!(at(7000, 300), Some((1, pt(1920.0 + 2560.0 - 624.0, 300.0))));
        assert_eq!(at(-3000, 2000), Some((2, pt(-1250.0, -300.0 + 1024.0 - 338.0))));
        // With its title on the 150% screen though its corner is just off it: on that one.
        assert_eq!(at(1900, 600), Some((1, pt(1920.0, 600.0))));
        // No place asked for, or no screens: the system places it.
        assert_eq!(window_origin(&screens, OpenAt::Any, framed), None);
        assert_eq!(window_origin(&[], OpenAt::At(300, 200), framed), None);
    }

    #[test]
    fn a_window_opens_in_the_middle_of_a_screen_or_of_its_owner() {
        let screens = desk();
        // In the middle of each screen's work area, at its scale; a missing screen means the
        // main one.
        let middle = |nth| window_origin(&screens, OpenAt::Middle(nth), framed);
        assert_eq!(middle(0), Some((0, pt(960.0 - 208.0, 520.0 - 169.0))));
        assert_eq!(middle(1), Some((1, pt(1920.0 + 1280.0 - 312.0, (200.0 + 720.0 - 253.5f32).round()))));
        assert_eq!(middle(2), Some((2, pt(-1250.0 + 625.0 - 208.0, -300.0 + 512.0 - 169.0))));
        assert_eq!(middle(7), middle(0));
        // Over an owner: in its middle, on its screen; kept on that screen where the owner lies
        // near its edge.
        let over = |owner| window_origin(&screens, OpenAt::Over(owner), framed);
        assert_eq!(over(rect(200.0, 100.0, 1000.0, 800.0)), Some((0, pt(700.0 - 208.0, 500.0 - 169.0))));
        assert_eq!(over(rect(2000.0, 300.0, 600.0, 400.0)), Some((1, pt(2300.0 - 312.0, (500.0 - 253.5f32).round()))));
        assert_eq!(over(rect(1500.0, 850.0, 300.0, 200.0)), Some((0, pt(1650.0 - 208.0, 1040.0 - 338.0))));
        // An owner on no screen (a test window far off them all) keeps it with it.
        assert_eq!(over(rect(-30000.0, -30000.0, 600.0, 400.0)), Some((2, pt(-30000.0 + 300.0 - 208.0, -30000.0 + 200.0 - 169.0))));
        // A window larger than its screen's work area lies at its top left.
        let huge = |_: f32| [3000.0, 2000.0];
        assert_eq!(window_origin(&screens, OpenAt::Middle(0), huge), Some((0, pt(0.0, 0.0))));
    }

    #[test]
    fn a_window_knows_which_screen_it_is_on_and_every_window_has_the_same_list() {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 400.0, h: 300.0 }, 1.5);
        // Nothing is known until a system says.
        assert_eq!((ui.screens(), ui.screen()), (&[][..], None));
        assert!(ui.set_screens(desk()) && !ui.set_screens(desk()), "the same list again is no news");
        assert_eq!((ui.screens().len(), ui.screen()), (3, None));
        // At 150% the window is 600 by 450 pixels: with its corner at 1,700, most of it is on
        // the 150% screen.
        let placed = |ui: &mut Ui, x, y| _ = ui.set_window_placement(WindowPlacement { position: (x, y), width: 400.0, height: 300.0, maximized: false });
        placed(&mut ui, 1700, 300);
        assert_eq!(ui.screen(), Some(1));
        placed(&mut ui, 1000, 300);
        assert_eq!(ui.screen(), Some(0));
        placed(&mut ui, -30000, -30000);
        assert_eq!(ui.screen(), Some(2), "off them all: the nearest");
        // A screen goes: the list is news.
        let mut fewer = desk();
        fewer.pop();
        assert!(ui.set_screens(fewer) && ui.screens().len() == 2);
    }
}
