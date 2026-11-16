//! A window that is on no screen, for the tests to click, type into and look at.
#![allow(dead_code)] // each file of tests uses its own part of this
use taut::geom::{Point, Size, pt};
use taut::text::BuiltInFont;
use taut::{Button, Event, Key, LocalClipboard, Mods, Rect, Surface, Ui};

/// It keeps the picture a real window would be showing: after every run only the part that
/// is said to have changed is drawn again, and the picture is then held against one of the
/// whole window drawn afresh.
pub struct Window {
    pub ui: Ui,
    pub screen: Surface,
    /// How many times the app described the window during the last run.
    pub passes: usize,
}
impl Window {
    pub fn run(&mut self, build: &mut dyn FnMut(&mut Ui)) -> Rect {
        self.passes = 0;
        let changed = self.ui.run(&mut |ui| {
            self.passes += 1;
            build(ui);
        });
        self.screen.redraw(&mut self.ui, changed);
        let afresh = Surface::draw(&mut self.ui, 64);
        if self.screen.pixels != afresh.pixels {
            panic!("drawing only what changed ({changed:?}) left the window different from drawing all of it, in pixels {:?}", self.differs(&afresh));
        }
        changed
    }
    /// The smallest block of pixels that holds every pixel in which the picture differs from
    /// `other`: left, top, right, bottom.
    fn differs(&self, other: &Surface) -> [usize; 4] {
        let wide = self.screen.width.max(1);
        let places = self.screen.pixels.iter().zip(&other.pixels).enumerate().filter(|(_, (a, b))| a != b).map(|(at, _)| (at % wide, at / wide));
        places.fold([usize::MAX, usize::MAX, 0, 0], |[left, top, right, bottom], (x, y)| [left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1)])
    }
}
impl std::ops::Deref for Window {
    type Target = Ui;
    fn deref(&self) -> &Ui {
        &self.ui
    }
}
impl std::ops::DerefMut for Window {
    fn deref_mut(&mut self) -> &mut Ui {
        &mut self.ui
    }
}

/// Such a window, 420 by 300, with the built-in letters: each 12 wide and 20 high in the
/// normal font.
pub fn window(scale: f32) -> Window {
    let mut ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 420.0, h: 300.0 }, scale);
    let screen = Surface::draw(&mut ui, 64);
    Window { ui, screen, passes: 0 }
}
pub fn middle(of: Rect) -> Point {
    pt(of.x + of.w / 2.0, of.y + of.h / 2.0)
}
/// Runs the interface twice: the second time, every box knows where the first run put it.
pub fn settle(ui: &mut Window, mut build: impl FnMut(&mut Ui)) {
    ui.run(&mut build);
    ui.run(&mut build);
}
/// Takes in one thing the person did, and runs the interface.
pub fn does(ui: &mut Window, event: Event, mut build: impl FnMut(&mut Ui)) {
    ui.event(event);
    ui.run(&mut build);
}
pub fn click(ui: &mut Window, at: Point, mut build: impl FnMut(&mut Ui)) {
    for event in [Event::Move(at), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
        does(ui, event, &mut build);
    }
}
/// Presses the left button at `from`, takes the pointer to `to`, and lets go.
pub fn drag(ui: &mut Window, from: Point, to: Point, mut build: impl FnMut(&mut Ui)) {
    for event in [Event::Move(from), Event::Press(Button::Left, Mods::NONE), Event::Move(to), Event::Release(Button::Left)] {
        does(ui, event, &mut build);
    }
}
pub fn key(ui: &mut Window, key: Key, mods: Mods, build: impl FnMut(&mut Ui)) {
    does(ui, Event::Key(key, mods), build);
}
pub fn typed(ui: &mut Window, text: &str, build: impl FnMut(&mut Ui)) {
    does(ui, Event::Text(text.into()), build);
}
