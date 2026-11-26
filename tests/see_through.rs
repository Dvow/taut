//! A see-through box is drawn once into a layer, and blended again from it while neither its
//! content nor what lies under it changes: its opacity changing redraws none of its text. Every
//! step is held against the window drawn afresh, which draws see-through boxes anew.
use std::cell::Cell;
use std::rc::Rc;
use taut::geom::Size;
use taut::text::{BuiltInFont, Metrics};
use taut::{Canvas, Color, Font, Length, LocalClipboard, Surface, TextSystem, Ui};

/// The built-in letters, counting the lines drawn.
struct Counted {
    letters: BuiltInFont,
    lines: Rc<Cell<usize>>,
}

impl TextSystem for Counted {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        self.letters.metrics(font, scale)
    }
    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        self.letters.measure(text, font, scale, stops)
    }
    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        self.lines.set(self.lines.get() + 1);
        self.letters.draw(canvas, text, x, y, font, color);
    }
}

/// A panel of ten lines over a box whose colour can change, both in a window 420 by 300.
struct Faded {
    solid: f32,
    under: Color,
    first: String,
}

fn faded(app: &Faded, ui: &mut Ui) {
    let t = ui.theme;
    ui.el().flow(taut::Flow::Stack).w(Length::Px(300.0)).h(Length::Px(260.0)).open();
    ui.el().w(Length::Px(300.0)).h(Length::Px(260.0)).fill(app.under).show();
    ui.column().pad(10.0).fill(t.panel).opacity(app.solid).open();
    ui.label(&app.first);
    for line in 1..10 {
        ui.label(&format!("Line {line}"));
    }
    ui.close();
    ui.close();
}

/// Runs the window and redraws what changed; holds that against all of it drawn afresh. Gives
/// how many lines of text the redraw drew.
fn step(ui: &mut Ui, screen: &mut Surface, lines: &Cell<usize>, app: &Faded) -> usize {
    let changed = ui.run(&mut |ui| faded(app, ui));
    let before = lines.get();
    screen.redraw(ui, changed);
    let drawn = lines.get() - before;
    assert!(screen.pixels == Surface::draw(ui, 64).pixels, "a redraw left the window different from drawing all of it");
    drawn
}

#[test]
fn a_see_through_box_is_blended_again_from_its_layer_while_nothing_in_or_under_it_changes() {
    let lines = Rc::new(Cell::new(0));
    let mut ui = Ui::new(Box::new(Counted { letters: BuiltInFont, lines: lines.clone() }), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 420.0, h: 300.0 }, 1.0);
    let mut screen = Surface::draw(&mut ui, 64);
    let mut app = Faded { solid: 0.5, under: Color::rgb(0x2060c0), first: "Line 0".into() };
    step(&mut ui, &mut screen, &lines, &app);
    assert_eq!(step(&mut ui, &mut screen, &lines, &app), 0, "nothing changed");
    // Faded further, step by step: blended from its layer, none of its lines drawn.
    for solid in [0.6, 0.7, 0.8, 0.3] {
        app.solid = solid;
        assert_eq!(step(&mut ui, &mut screen, &lines, &app), 0, "at {solid}");
    }
    // What is under it changes: drawn again, then kept again.
    app.under = Color::rgb(0xc06020);
    assert_eq!(step(&mut ui, &mut screen, &lines, &app), 10);
    app.solid = 0.4;
    assert_eq!(step(&mut ui, &mut screen, &lines, &app), 0);
    // So does its content.
    app.first = "Changed".into();
    assert!(step(&mut ui, &mut screen, &lines, &app) > 0);
    app.solid = 0.9;
    assert_eq!(step(&mut ui, &mut screen, &lines, &app), 0);
    // Solid, it has no layer; see-through again, it is drawn once more.
    app.solid = 1.0;
    step(&mut ui, &mut screen, &lines, &app);
    app.solid = 0.5;
    assert_eq!(step(&mut ui, &mut screen, &lines, &app), 10);
}

/// A box right above a see-through one, touching it: under the pixel its layer has all round.
fn touching(app: &Faded, ui: &mut Ui) {
    let t = ui.theme;
    ui.el().w(Length::Px(300.0)).h(Length::Px(40.0)).fill(app.under).show();
    ui.column().w(Length::Px(300.0)).pad(10.0).fill(t.panel).opacity(app.solid).open();
    ui.label(&app.first);
    ui.close();
}

#[test]
fn a_box_touching_a_see_through_one_changing_draws_its_layer_again() {
    let lines = Rc::new(Cell::new(0));
    let mut ui = Ui::new(Box::new(Counted { letters: BuiltInFont, lines: lines.clone() }), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: 420.0, h: 300.0 }, 1.5);
    let mut screen = Surface::draw(&mut ui, 64);
    let mut app = Faded { solid: 0.5, under: Color::rgb(0x2060c0), first: "Words".into() };
    let mut step = |app: &Faded| {
        let changed = ui.run(&mut |ui| touching(app, ui));
        let before = lines.get();
        screen.redraw(&mut ui, changed);
        let drawn = lines.get() - before;
        assert!(screen.pixels == Surface::draw(&mut ui, 64).pixels, "a redraw left the window different from drawing all of it");
        drawn
    };
    step(&app);
    app.solid = 0.6;
    assert_eq!(step(&app), 0);
    // The box above changes, and the panel fades on: drawn again, not blended from what it
    // kept of the box's old edge.
    (app.under, app.solid) = (Color::rgb(0xc02020), 0.7);
    assert_eq!(step(&app), 1);
}
