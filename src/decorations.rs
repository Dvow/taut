//! Client-side window decorations, for Wayland compositors that leave them to the app (GNOME's,
//! and any without server-side decorations).
//!
//! A title bar over the window: the title, and minimize, maximize/restore and close buttons. It
//! is a `Ui` of its own in the app's theme. It moves the window, and its edges resize it. The
//! window's host shows it, feeds it pointer input, and carries out its buttons.
use crate::canvas::coverage;
use crate::color::Color;
use crate::layout::{Align, Length};
use crate::paint::Mask;
use crate::path::Path;
use crate::platform::LocalClipboard;
use crate::text::TextSystem;
use crate::theme::Theme;
use crate::ui::{Icon, Ui};

/// The bar's height, and each button's width, in units.
pub(crate) const HIGH: i32 = 34;
const BUTTON: f32 = 44.0;

/// The button icons in a 16 by 16 square, as SVG paths: a line; a square, or two overlapping
/// for restore; a cross. Drawn from `lines`.
const MINIMIZE: Icon = Icon::line("M4 8.5h8");
const FILL: Icon = Icon::line("M4.5 4.5h7v7h-7z");
const BACK: Icon = Icon::line("M4.5 6.5h5v5h-5zM6.5 6.5V4.5h5v5h-2");
const CLOSE: Icon = Icon::line("M4.5 4.5l7 7M11.5 4.5l-7 7");

/// An icon's path, built by hand rather than parsed: every Wayland program may show the bar,
/// and most never parse SVG, so they should not carry a parser for four fixed shapes.
fn lines(icon: Icon) -> Path {
    let mut path = Path::new();
    match icon.path {
        shape if shape == MINIMIZE.path => _ = path.move_to(4.0, 8.5).line_to(12.0, 8.5),
        shape if shape == FILL.path => _ = path.move_to(4.5, 4.5).line_to(11.5, 4.5).line_to(11.5, 11.5).line_to(4.5, 11.5).close(),
        shape if shape == BACK.path => {
            path.move_to(4.5, 6.5).line_to(9.5, 6.5).line_to(9.5, 11.5).line_to(4.5, 11.5).close();
            path.move_to(6.5, 6.5).line_to(6.5, 4.5).line_to(11.5, 4.5).line_to(11.5, 9.5).line_to(9.5, 9.5);
        }
        _ => _ = path.move_to(4.5, 4.5).line_to(11.5, 11.5).move_to(11.5, 4.5).line_to(4.5, 11.5),
    }
    path
}

/// An icon drawn at `px` by `px` pixels, as any icon is (`paint::draw_mask`).
fn drawn(icon: Icon, px: usize) -> Mask {
    Mask { width: px, cover: coverage(&lines(icon), px as f32 / icon.view, icon.stroke, (0, 0), px, px) }
}

/// What a bar button asked of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    Minimize,
    /// Maximize, or restore.
    Fill,
    Close,
}

/// One window's title bar and its state: title, maximized, and focused (the title is fainter
/// when not).
pub(crate) struct TitleBar {
    pub(crate) ui: Ui,
    pub(crate) title: String,
    pub(crate) filling: bool,
    pub(crate) active: bool,
}

impl TitleBar {
    /// A bar that writes with `text`.
    pub(crate) fn new(text: Box<dyn TextSystem>) -> Self {
        let mut ui = Ui::new(text, Box::new(LocalClipboard::default()));
        ui.shows.icon = Some(drawn);
        TitleBar { ui, title: String::new(), filling: false, active: true }
    }

    /// Runs the bar in the window's theme; returns the button pressed, if any.
    pub(crate) fn run(&mut self, theme: Theme) -> Option<Asked> {
        let (title, filling, active, mut asked) = (self.title.as_str(), self.filling, self.active, None);
        self.ui.theme = theme;
        self.ui.run(&mut |ui: &mut Ui| asked = asked.or(describe(ui, title, filling, active)));
        asked
    }
}

/// The bar: title at the start, three buttons at the end, on the panel colour. Dragging it
/// moves the window.
fn describe(ui: &mut Ui, title: &str, filling: bool, active: bool) -> Option<Asked> {
    let t = ui.theme;
    let ink = if active { t.text } else { t.text_faint };
    let strip = ui.row().w(Length::Grow(1.0)).h(Length::Px(HIGH as f32)).align(Align::Start, Align::Center).fill(t.panel).sense().open();
    ui.el().text(title, t.font, ink).pad((12.0, 0.0)).shrink(1).show();
    ui.spacer();
    let buttons = [(MINIMIZE, Asked::Minimize), (if filling { BACK } else { FILL }, Asked::Fill), (CLOSE, Asked::Close)];
    let pressed = buttons.map(|(icon, asked)| button(ui, icon, asked == Asked::Close, ink).then_some(asked));
    ui.close();
    ui.set_title_bar(&strip);
    pressed.into_iter().flatten().next()
}

/// A bar button: highlighted under the pointer (close in red). Returns whether it was clicked.
fn button(ui: &mut Ui, icon: Icon, closes: bool, ink: Color) -> bool {
    let t = ui.theme;
    let state = ui.press(ui.next_id());
    let (fill, ink) = match (state.hovered || state.pressed, closes) {
        (true, true) => (t.red, Color::WHITE),
        (true, false) => (t.hover, t.text),
        _ => (Color::NONE, ink),
    };
    ui.el().size(BUTTON, HIGH as f32).align(Align::Center, Align::Center).fill(fill).sense().open();
    ui.el().drawn_icon(icon, 16.0, ink).show();
    ui.close();
    state.clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Button, Event, Mods};
    use crate::frameless::HitTest;
    use crate::geom::{Size, pt};
    use crate::text::BuiltInFont;

    /// A bar 400 units wide, laid out once.
    fn title_bar() -> TitleBar {
        let mut bar = TitleBar::new(Box::new(BuiltInFont));
        bar.title = "Notes".into();
        bar.ui.resize(Size { w: 400.0, h: HIGH as f32 }, 1.0);
        bar.run(Theme::dark());
        bar.run(Theme::dark());
        bar
    }

    /// Clicks the bar at `x`; returns what that asked.
    fn click(bar: &mut TitleBar, x: f32) -> Option<Asked> {
        let mut asked = None;
        for event in [Event::Move(pt(x, 17.0)), Event::Press(Button::Left, Mods::NONE), Event::Release(Button::Left)] {
            bar.ui.event(event);
            asked = asked.or(bar.run(Theme::dark()));
        }
        asked
    }

    #[test]
    fn the_strip_is_taken_hold_of_where_its_buttons_are_not() {
        let bar = title_bar();
        assert_eq!(bar.ui.hit_test(pt(100.0, 17.0), false), HitTest::TitleBar, "beside the title: the window moves");
        assert_eq!(bar.ui.hit_test(pt(20.0, 17.0), false), HitTest::TitleBar, "on the title too");
        assert_eq!(bar.ui.hit_test(pt(400.0 - BUTTON / 2.0, 17.0), false), HitTest::Client, "a button is pressed as ever");
        assert_eq!(bar.ui.hit_test(pt(200.0, 2.0), true), HitTest::Edge(0, -1), "its top edge sizes the window");
    }

    /// The icons match their paths drawn as icons, pixel for pixel, at every size, without the
    /// bar compiling in the path reader.
    #[test]
    fn the_buttons_shapes_are_their_paths_drawn_as_icons_are() {
        for icon in [MINIMIZE, FILL, BACK, CLOSE] {
            assert_eq!(lines(icon), Path::svg(icon.path), "{}", icon.path);
            for px in 8..=64 {
                assert_eq!(drawn(icon, px).cover, crate::paint::draw_mask(icon, px).cover, "{} at {px} pixels", icon.path);
            }
        }
        assert!(title_bar().ui.shows.icon.is_some_and(|draws| std::ptr::fn_addr_eq(draws, drawn as fn(Icon, usize) -> Mask)));
    }

    #[test]
    fn each_button_asks_for_its_own() {
        let mut bar = title_bar();
        let middle = |nth: f32| 400.0 - BUTTON * (3.0 - nth) + BUTTON / 2.0;
        assert_eq!(click(&mut bar, middle(0.0)), Some(Asked::Minimize));
        assert_eq!(click(&mut bar, middle(1.0)), Some(Asked::Fill));
        assert_eq!(click(&mut bar, middle(2.0)), Some(Asked::Close));
        assert_eq!(click(&mut bar, 100.0), None, "the bar itself is no button");
    }
}
