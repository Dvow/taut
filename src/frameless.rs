//! Frameless windows and window state (minimized, maximized, full screen), the same on every
//! system.
//!
//! A frameless window has no system frame (`Window { frameless: true, .. }` for the first,
//! [`ui.frameless_window`](Ui::frameless_window) for another). The app draws its own title bar
//! and names that box; the window moves by it, resizes by its edges and corners, and the app's
//! buttons minimize, maximize and close it:
//!
//! ```ignore
//! let bar = ui.row().sense().open();
//! ui.label("Notes");
//! ui.spacer();
//! if ui.button("Down").clicked { ui.minimize(); }
//! if ui.button("Up").clicked { let fills = ui.maximized(); ui.set_maximized(!fills); }
//! if ui.button("Close").clicked { ui.quit(); }
//! ui.close();
//! ui.set_title_bar(&bar);
//! ```
//!
//! [`hit_test`](Ui::hit_test) says, the same on every system, what the pointer grips at a
//! point: an edge, the title bar, or nothing. The window's host asks, and has the system move
//! or resize. The bar is that box only, not what lies on it: a button in it works as ever, and
//! so does everything while a menu or dialog is open over it.
//!
//! Any window, frameless or not, can go full screen
//! ([`set_full_screen`](Ui::set_full_screen)) and back.
use crate::event::Cursor;
use crate::geom::Point;
use crate::id::Id;
use crate::ui::{Response, Ui};

/// How near an edge of a frameless window the pointer grips it, in units; and how near an
/// edge's end it is the corner.
const EDGE: f32 = 6.0;
const CORNER: f32 = 16.0;

/// What the pointer is over in its window at a point: a hit test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HitTest {
    /// Nothing: the point belongs to the UI.
    #[default]
    Client,
    /// The title bar, which moves the window.
    TitleBar,
    /// An edge that resizes the window: left (-1) or right (1), top (-1) or bottom (1), 0 for
    /// neither. A corner is both.
    Edge(i8, i8),
}

impl HitTest {
    /// The pointer shape over it, where the UI does not choose one: arrows along the edge.
    pub fn cursor(self) -> Option<Cursor> {
        match self {
            HitTest::Edge(_, 0) => Some(Cursor::ResizeX),
            HitTest::Edge(0, _) => Some(Cursor::ResizeY),
            HitTest::Edge(across, down) => Some(if across == down { Cursor::ResizeDown } else { Cursor::ResizeUp }),
            _ => None,
        }
    }
}

/// The window's state beyond its place, and what its app asked since the host last took it.
#[derive(Default)]
pub(crate) struct WindowState {
    /// The box that moves the window, as named this run; 0 if none.
    pub title_bar: Id,
    /// Full screen.
    full_screen: bool,
    /// Requests: minimize; maximize or restore; full screen or back.
    request_minimize: bool,
    request_maximize: Option<bool>,
    request_full_screen: Option<bool>,
}

/// Which end of a span `long` a point is within `reach` of: start (-1), end (1), or neither
/// (0).
fn end(at: f32, long: f32, reach: f32) -> i8 {
    (at >= long - reach) as i8 - (at < reach) as i8
}

/// Windows without a system frame: the title bar, and what its buttons do.
impl Ui {
    // ---- for the app ----

    /// Makes this box move the window: the title bar of a frameless window, or any box in a
    /// framed one. Call every run, on a box that senses the pointer (`.sense()`). A press on
    /// the box itself, not on what lies on it, grips the window; a double press maximizes or
    /// restores.
    pub fn set_title_bar(&mut self, of: &Response) {
        self.window_state.title_bar = of.id;
    }
    /// Minimizes the window, as a custom title bar's button does.
    /// [`window_minimized`](Self::window_minimized) then says so, and
    /// [`bring_to_front`](Self::bring_to_front) restores it.
    pub fn minimize(&mut self) {
        self.window_state.request_minimize = true;
    }
    /// Maximizes the window, or restores it. (Not while full screen.)
    pub fn set_maximized(&mut self, on: bool) {
        self.window_state.request_maximize = Some(on);
    }
    /// Whether the window is maximized. The user can change it too; the UI reruns when it does.
    pub fn maximized(&self) -> bool {
        self.window_placement().is_some_and(|place| place.maximized)
    }
    /// Makes the window full screen (no frame, nothing of the system's beside it), or restores
    /// it. ([`window_placement`](Self::window_placement) still says where it goes back to.)
    pub fn set_full_screen(&mut self, on: bool) {
        self.window_state.request_full_screen = Some(on);
    }
    /// Whether the window is full screen. The UI reruns when it changes.
    pub fn full_screen(&self) -> bool {
        self.window_state.full_screen
    }

    // ---- for the part that runs the window ----

    /// Whether the app asked to minimize the window; reset by asking.
    pub fn take_minimize(&mut self) -> bool {
        std::mem::take(&mut self.window_state.request_minimize)
    }
    /// Whether the app asked to maximize or restore the window; reset by asking.
    pub fn take_maximized(&mut self) -> Option<bool> {
        self.window_state.request_maximize.take()
    }
    /// Whether the app asked for full screen or back; reset by asking.
    pub fn take_full_screen(&mut self) -> Option<bool> {
        self.window_state.request_full_screen.take()
    }
    /// For the window's host: whether the window is full screen now. Returns whether that is
    /// news (then the UI should run, for the app to hear).
    pub fn full_screen_now(&mut self, on: bool) -> bool {
        std::mem::replace(&mut self.window_state.full_screen, on) != on
    }

    /// What the pointer grips at `at`: asked by the window's host at every pointer move and
    /// press, before the UI gets it. `sized` says whether the window resizes by its own edges
    /// there (a frameless window does, unless maximized). Anything but `Client` belongs to the
    /// window, not the UI: the pointer has left the UI, and a press moves or resizes the
    /// window. A full-screen window grips nowhere.
    pub fn hit_test(&self, at: Point, sized: bool) -> HitTest {
        if self.window_state.full_screen {
            return HitTest::Client;
        }
        let (size, title_bar) = (self.size(), self.window_state.title_bar);
        // An edge, near one; a corner, near an edge's end.
        let (across, down) = (end(at.x, size.w, EDGE), end(at.y, size.h, EDGE));
        if sized && (across, down) != (0, 0) {
            let (across, down) = (if across == 0 { end(at.x, size.w, CORNER) } else { across }, if down == 0 { end(at.y, size.h, CORNER) } else { down });
            return HitTest::Edge(across, down);
        }
        // The title bar, where it is the topmost box there that senses the pointer.
        let on = self.visible_boxes.hits.iter().rfind(|(_, area)| area.contains(at)).map_or(0, |(id, _)| *id);
        if title_bar != 0 && on == title_bar { HitTest::TitleBar } else { HitTest::Client }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Size, pt};
    use crate::platform::{LocalClipboard, WindowPlacement};
    use crate::text::BuiltInFont;
    use crate::{Length, Rect};

    /// The test app's state: whether it names a title bar, whether a menu is open, and where
    /// its bar and its button are.
    #[derive(Default)]
    struct App {
        no_strip: bool,
        asking: bool,
        strip: Rect,
        button: Rect,
    }

    /// A window with its own title bar across the top, a button at the bar's end, and a page
    /// below.
    fn describe(app: &mut App, ui: &mut Ui) {
        let strip = ui.row().h(Length::Px(30.0)).sense().open();
        ui.label("A title");
        ui.spacer();
        app.button = ui.button("Close").rect;
        ui.close();
        if !app.no_strip {
            ui.set_title_bar(&strip);
        }
        app.strip = strip.rect;
        ui.column().grow().open();
        ui.button("On the page");
        ui.close();
        if app.asking {
            ui.dialog();
            ui.label("Answer this first");
            ui.close();
        }
    }

    /// A 400 by 300 UI, described twice so every box knows its place.
    fn window(app: &mut App) -> Ui {
        let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: 400.0, h: 300.0 }, 1.0);
        ui.run(&mut |ui| describe(app, ui));
        ui.run(&mut |ui| describe(app, ui));
        ui
    }

    #[test]
    fn a_window_is_taken_hold_of_by_its_strip_and_not_by_what_lies_on_it() {
        let mut app = App::default();
        let mut ui = window(&mut app);
        let (strip, button) = (app.strip, app.button);
        assert!(strip.w == 400.0 && strip.h == 30.0 && button.w > 0.0, "{strip:?} {button:?}");
        // The bar itself, also where its title is; not its button, nor the page below.
        assert_eq!([pt(200.0, 15.0), pt(20.0, 15.0)].map(|at| ui.hit_test(at, false)), [HitTest::TitleBar; 2]);
        let on_button = pt(button.x + button.w / 2.0, button.y + button.h / 2.0);
        assert_eq!([on_button, pt(200.0, 150.0), pt(20.0, 60.0)].map(|at| ui.hit_test(at, false)), [HitTest::Client; 3]);
        // Under a dialog, nothing is the bar: the press is the dialog's.
        app.asking = true;
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!(ui.hit_test(pt(200.0, 15.0), false), HitTest::Client);
        app.asking = false;
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!(ui.hit_test(pt(200.0, 15.0), false), HitTest::TitleBar);
        // An app that stops naming a bar has none from then on.
        app.no_strip = true;
        ui.run(&mut |ui| describe(&mut app, ui));
        assert_eq!(ui.hit_test(pt(200.0, 15.0), false), HitTest::Client);
    }

    #[test]
    fn a_bare_window_is_sized_by_its_edges_and_corners() {
        let ui = window(&mut App::default());
        let grip = |x: f32, y: f32| ui.hit_test(pt(x, y), true);
        // Each edge, as far in as it reaches, and just past that.
        assert_eq!([grip(0.0, 150.0), grip(5.9, 150.0), grip(399.9, 150.0), grip(394.0, 150.0)], [HitTest::Edge(-1, 0), HitTest::Edge(-1, 0), HitTest::Edge(1, 0), HitTest::Edge(1, 0)]);
        assert_eq!([grip(200.0, 0.0), grip(200.0, 5.9), grip(200.0, 299.9), grip(200.0, 294.0)], [HitTest::Edge(0, -1), HitTest::Edge(0, -1), HitTest::Edge(0, 1), HitTest::Edge(0, 1)]);
        assert_eq!([grip(6.0, 150.0), grip(393.9, 150.0), grip(200.0, 293.9)], [HitTest::Client; 3]);
        // The top edge lies over the bar, which starts where the edge ends.
        assert_eq!([grip(200.0, 6.0), grip(200.0, 29.0)], [HitTest::TitleBar; 2]);
        // Corners: where two edges meet, and each edge's end.
        assert_eq!([grip(0.0, 0.0), grip(399.0, 0.0), grip(0.0, 299.0), grip(399.0, 299.0)], [HitTest::Edge(-1, -1), HitTest::Edge(1, -1), HitTest::Edge(-1, 1), HitTest::Edge(1, 1)]);
        assert_eq!([grip(15.9, 2.0), grip(16.0, 2.0), grip(2.0, 15.9), grip(2.0, 16.0)], [HitTest::Edge(-1, -1), HitTest::Edge(0, -1), HitTest::Edge(-1, -1), HitTest::Edge(-1, 0)]);
        assert_eq!([grip(384.0, 297.0), grip(383.9, 297.0), grip(397.0, 284.0), grip(397.0, 283.9)], [HitTest::Edge(1, 1), HitTest::Edge(0, 1), HitTest::Edge(1, 1), HitTest::Edge(1, 0)]);
        // The pointer shows each direction.
        let shape = |across, down| HitTest::Edge(across, down).cursor();
        assert_eq!([shape(-1, 0), shape(1, 0), shape(0, -1), shape(0, 1)], [Some(Cursor::ResizeX), Some(Cursor::ResizeX), Some(Cursor::ResizeY), Some(Cursor::ResizeY)]);
        assert_eq!([shape(-1, -1), shape(1, 1), shape(1, -1), shape(-1, 1)], [Some(Cursor::ResizeDown), Some(Cursor::ResizeDown), Some(Cursor::ResizeUp), Some(Cursor::ResizeUp)]);
        assert_eq!([HitTest::TitleBar.cursor(), HitTest::Client.cursor()], [None, None]);
        // A window not resized by its own edges has none: a framed one, and a maximized bare
        // one.
        assert_eq!([ui.hit_test(pt(0.0, 150.0), false), ui.hit_test(pt(399.0, 299.0), false), ui.hit_test(pt(200.0, 2.0), false)], [HitTest::Client, HitTest::Client, HitTest::TitleBar]);
    }

    #[test]
    fn what_an_app_asks_of_its_window_is_taken_once_and_how_the_window_stands_is_heard() {
        let mut ui = window(&mut App::default());
        // Nothing is asked until the app asks.
        assert_eq!((ui.take_minimize(), ui.take_maximized(), ui.take_full_screen()), (false, None, None));
        ui.minimize();
        ui.set_maximized(true);
        ui.set_full_screen(true);
        assert_eq!((ui.take_minimize(), ui.take_maximized(), ui.take_full_screen()), (true, Some(true), Some(true)));
        assert_eq!((ui.take_minimize(), ui.take_maximized(), ui.take_full_screen()), (false, None, None));
        ui.set_maximized(false);
        ui.set_full_screen(false);
        assert_eq!((ui.take_maximized(), ui.take_full_screen()), (Some(false), Some(false)));
        // Maximized follows its placement; full screen is what the host says, and only a change
        // is news.
        assert!(!ui.maximized() && !ui.full_screen());
        let there = WindowPlacement { position: (10, 20), width: 400.0, height: 300.0, maximized: true };
        assert!(ui.set_window_placement(there) && ui.maximized());
        assert_eq!((ui.full_screen_now(true), ui.full_screen_now(true), ui.full_screen()), (true, false, true));
        // Full screen grips nowhere: no edge, no title bar.
        assert_eq!([ui.hit_test(pt(0.0, 0.0), true), ui.hit_test(pt(200.0, 15.0), true)], [HitTest::Client; 2]);
        assert_eq!((ui.full_screen_now(false), ui.full_screen(), ui.window_placement()), (true, false, Some(there)));
        assert_eq!(ui.hit_test(pt(200.0, 15.0), false), HitTest::TitleBar);
    }
}
