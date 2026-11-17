//! Picking a colour: a swatch that opens a picker under it. A square sets saturation and value,
//! a strip the hue, another strip the alpha (over a checkerboard). Under them the colour as
//! "#rrggbb" (plus two digits for alpha) and as numbers (RGB 0–255, alpha 0–100), each typed
//! in; then the colours picked last in this window.
//!
//! Tab reaches everything. Arrows move the square's ring or a strip's line by 1% (a degree of
//! hue), ten times as far with Shift. Screen readers get the square, strips and numbers as
//! sliders and spin buttons named S, H, A, R, G, B.
use crate::accessibility::Role;
use crate::color::Color;
use crate::event::{Key, Mods};
use crate::geom::Rect;
use crate::id::Id;
use crate::layout::{Flow, Length};
use crate::path::Path;
use crate::ui::{Picture, Response, Ui};

/// The square's size, a strip's width, and the side in pixels of the pictures in them
/// (stretched smoothly).
const SQUARE: [f32; 2] = [168.0, 120.0];
const STRIP: f32 = 16.0;
const FINE: usize = 32;
/// How many recent colours are kept.
const KEPT: usize = 8;
/// Each part of the picker that is dragged: its name for screen readers, its range as told
/// (saturation in %, hue in degrees, alpha in %), and what a key moves it, as a fraction of the
/// way.
const PARTS: [(&str, f32, f32); 3] = [("S", 100.0, 0.01), ("H", 360.0, 1.0 / 360.0), ("A", 100.0, 0.01)];
const SQUARE_PART: usize = 0;
const HUES: usize = 1;
const SOLID: usize = 2;

/// The open colour picker: whose it is, and its colour as hue, saturation and value. (A grey
/// has no hue and black no saturation; kept here, they are not lost.)
#[derive(Default)]
pub(crate) struct Picking {
    of: Id,
    hsv: [f32; 3],
    /// The pictures of the square, the hues and the alpha; and the colour they show.
    pictures: Option<[Picture; 3]>,
    drawn: Option<(f32, Color)>,
    /// The swatch whose picker is open and its colour at opening: if the colour differs when it
    /// closes, it joins the recent ones. And whether the picker just opened, to put the
    /// keyboard on its square.
    open: Option<(Id, Color)>,
    fresh: bool,
    used: Vec<Color>,
}

/// Where the pointer is in `in_box`, each from 0 to 1, while it holds the box.
fn spot(ui: &Ui, held: &Response, in_box: Rect) -> Option<[f32; 2]> {
    let at = ui.pointer().filter(|_| held.pressed)?;
    Some([((at.x - in_box.x) / in_box.w.max(1.0)).clamp(0.0, 1.0), ((at.y - in_box.y) / in_box.h.max(1.0)).clamp(0.0, 1.0)])
}

/// Every other 4-unit square over a box this size: the dark squares of a checkerboard.
fn checks([wide, high]: [f32; 2]) -> Path {
    let mut checks = Path::new();
    for (row, column) in (0..(high / 4.0).ceil() as usize).flat_map(|row| (0..(wide / 4.0).ceil() as usize).map(move |column| (row, column))) {
        let (x, y) = (column as f32 * 4.0, row as f32 * 4.0);
        if (row + column) % 2 == 1 {
            checks.move_to(x, y).line_to(x + 4.0, y).line_to(x + 4.0, y + 4.0).line_to(x, y + 4.0).close();
        }
    }
    checks
}

/// Colours, picked.
impl Ui {
    /// A swatch of `color` that opens a picker under it. `changed` says when the person picked
    /// another colour.
    pub fn color_picker(&mut self, color: &mut Color) -> Response {
        let (t, was) = (self.theme, *color);
        let mut state = self.press(self.next_id());
        if state.clicked {
            self.open_popup(state.id, state.rect);
        }
        let edge = if state.hovered { t.text_faint } else { t.line_strong };
        self.row().size(t.control_height * 1.6, t.control_height).pad(3.0).border(edge, 1.0).radius(t.radius).sense().focusable().open();
        self.swatch(*color, [t.control_height * 1.6 - 6.0, t.control_height - 6.0], (t.radius - 2.0).max(0.0));
        self.close();
        // A screen reader is told it opens a choice, named by the colour as "#rrggbb", which is
        // also what is chosen.
        if self.listening {
            let open = self.popups.iter().any(|popup| popup.id == state.id);
            let written = color.to_string();
            self.keep_note(&state, Role::ComboBox, Some(open), &written, &written);
        }
        if !self.popup(state.id) {
            // Closed: the colour it was set to, if any, joins the recent ones.
            if let Some((_, opened_with)) = self.picking.open.take_if(|(of, _)| *of == state.id) {
                self.remember_color(*color, opened_with);
            }
            return state;
        }
        if self.picking.open.is_none_or(|(of, _)| of != state.id) {
            (self.picking.open, self.picking.fresh) = (Some((state.id, *color)), true);
        }
        // Never larger than the window: what does not fit is scrolled.
        let [wide, high] = self.popup_most();
        self.column().pad(10.0).gap(8.0).max(wide, high).scroll().fill(t.raised).border(t.line_strong, 1.0).radius(8.0).shadow(16.0).sense().open();
        self.pick_color(state.id, color);
        self.close();
        state.changed = *color != was;
        self.again |= state.changed;
        state
    }

    /// Puts a picked colour first among the recent ones, unless it is the one the picker opened
    /// with.
    fn remember_color(&mut self, color: Color, opened_with: Color) {
        if color == opened_with {
            return;
        }
        let used = &mut self.picking.used;
        used.retain(|kept| *kept != color);
        used.insert(0, color);
        used.truncate(KEPT);
    }

    /// A colour `size` large, over a checkerboard where it is see-through.
    fn swatch(&mut self, color: Color, size: [f32; 2], radius: f32) {
        if color.a() == 255 {
            self.el().size(size[0], size[1]).radius(radius).fill(color).show();
            return;
        }
        self.el().flow(Flow::Stack).size(size[0], size[1]).radius(radius).clip().open();
        self.el().grow().fill(Color::WHITE).shape(&checks(size), Color::rgb(0xc8c8c8)).show();
        self.el().grow().radius(radius).fill(color).show();
        self.close();
    }

    /// The picker's content: the square and strips, the colour as text and numbers, and the
    /// recent colours.
    fn pick_color(&mut self, of: Id, color: &mut Color) {
        // Another picker, or a colour changed elsewhere: start from that colour.
        let [hue, strong, light] = self.picking.hsv;
        if self.picking.of != of || Color::hsv(hue, strong, light) != color.solid() {
            (self.picking.of, self.picking.hsv) = (of, color.to_hsv());
        }
        let [square, hues, solids] = self.picker_pictures(*color);
        let ([mut hue, mut strong, mut light], mut solid) = (self.picking.hsv, color.a() as f32 / 255.0);
        // Smaller where the window lacks room for the whole picker (larger text, a small
        // window): the square gets what is left after the text box, the numbers with their
        // letters, the recent colours, and the spacing.
        let small = self.text.metrics(self.theme.small_font, self.scale).height().ceil();
        let under = 2.0 * self.line_part() + small + 38.0 + if self.picking.used.is_empty() { 0.0 } else { 28.0 };
        let high = SQUARE[1].min(self.popup_most()[1] - under).max(2.0 * STRIP);
        self.row().gap(8.0).open();
        if let Some([along, down]) = self.picker_part(square, [SQUARE[0], high], [strong, 1.0 - light], SQUARE_PART) {
            (strong, light) = (along, 1.0 - down);
        }
        if let Some([_, down]) = self.picker_part(hues, [STRIP, high], [0.5, hue], HUES) {
            hue = down;
        }
        if let Some([_, down]) = self.picker_part(solids, [STRIP, high], [0.5, 1.0 - solid], SOLID) {
            solid = 1.0 - down;
        }
        self.close();
        let picked = Color::hsv(hue, strong, light).with_alpha(solid);
        if [hue, strong, light] != self.picking.hsv || picked.a() != color.a() {
            (self.picking.hsv, *color) = ([hue, strong, light], picked);
        }
        // As text, to read or type: "#rrggbb", plus two digits for alpha.
        let typed = self.entry_box(format_args!("{color}"), '.', "#rrggbb").changed;
        if let Some(written) = typed.then(|| Color::parse(&self.entry.1)).flatten() {
            *color = written;
        }
        self.color_numbers(color);
        self.colors_used(color);
    }

    /// The colour as numbers to type or step: RGB 0–255 and alpha 0–100, each under its letter.
    fn color_numbers(&mut self, color: &mut Color) {
        let t = self.theme;
        let mut parts = [16, 8, 0].map(|shift| (color.0 >> shift & 0xff) as u8);
        let mut solid = (color.a() as f32 / 2.55).round() as u8;
        let (mut changed, mut solid_changed) = (false, false);
        self.row().gap(6.0).open();
        for (letter, part) in ["R", "G", "B"].into_iter().zip(&mut parts) {
            self.column().w(Length::Grow(1.0)).gap(2.0).open();
            self.text(letter, t.small_font, t.text_faint);
            changed |= self.number_in(part, 0..=255, 1, "").changed;
            self.close();
        }
        self.column().w(Length::Grow(1.0)).gap(2.0).open();
        self.text("A", t.small_font, t.text_faint);
        solid_changed |= self.number_in(&mut solid, 0..=100, 1, "%").changed;
        self.close();
        self.close();
        // Alpha is recomputed from the percentage only when that changed, so typing red keeps
        // alpha as it was.
        let alpha = if solid_changed { (solid as f32 * 2.55).round() as u32 } else { color.a() as u32 };
        if changed || solid_changed {
            let [red, green, blue] = parts.map(u32::from);
            *color = Color(alpha << 24 | red << 16 | green << 8 | blue);
        }
    }

    /// The recent colours in a row, newest first; pressing one picks it again.
    fn colors_used(&mut self, color: &mut Color) {
        if self.picking.used.is_empty() {
            return;
        }
        let t = self.theme;
        self.row().gap(4.0).open();
        for at in 0..self.picking.used.len() {
            let used = self.picking.used[at];
            let state = self.press(self.next_id());
            let edge = if state.hovered || state.focused { t.text_faint } else { t.line };
            self.row().size(20.0, 20.0).pad(2.0).border(edge, 1.0).radius(4.0).sense().focusable().open();
            self.swatch(used, [16.0, 16.0], 2.0);
            self.close();
            if self.listening {
                self.tell(&state, Role::Button, None, &used.to_string());
            }
            if state.clicked {
                *color = used;
            }
        }
        self.close();
    }

    /// One part of the picker, `which` of `PARTS`: a picture `size` large with a mark at `at`
    /// (0 to 1 across and down): a ring, or a line across a strip. Gives back where the mark
    /// goes: under the pointer while held, or where keys or a screen reader moved it.
    fn picker_part(&mut self, picture: Picture, size: [f32; 2], at: [f32; 2], which: usize) -> Option<[f32; 2]> {
        let (t, (name, most, fine)) = (self.theme, PARTS[which]);
        let held = self.interact(self.next_id());
        // The keyboard goes to the square as the picker opens.
        if which == SQUARE_PART && std::mem::take(&mut self.picking.fresh) {
            self.set_focus(held.id);
        }
        let mut to = spot(self, &held, held.rect);
        if held.focused {
            let mut key = |pressed: Key| (self.key(pressed, Mods::NONE) as i32 + 10 * self.key(pressed, Mods::SHIFT) as i32) as f32;
            let [right, left, down, up] = [Key::Right, Key::Left, Key::Down, Key::Up].map(&mut key);
            let by = [(right - left) * (which == SQUARE_PART) as u8 as f32, down - up].map(|by| by * fine);
            to = to.or((by != [0.0; 2]).then(|| [at[0] + by[0], at[1] + by[1]]));
        }
        // Screen readers are told the position across the square, down the hue strip and up the
        // alpha strip; where one asks for a value, it is set.
        let value = [at[0], at[1], 1.0 - at[1]][which];
        if let Some((_, set)) = self.reader_sets.take_if(|(of, _)| *of == held.id) {
            let along = (set / most).clamp(0.0, 1.0);
            to = Some([[along, at[1]], [at[0], along], [at[0], 1.0 - along]][which]);
        }
        let [x, y] = to.unwrap_or(at).map(|part| part.clamp(0.0, 1.0));
        let mut mark = Path::new();
        match which {
            SQUARE_PART => mark.circle(x * size[0], y * size[1], 5.0),
            _ => mark.move_to(1.0, y * size[1]).line_to(size[0] - 1.0, y * size[1]),
        };
        self.el().flow(Flow::Stack).size(size[0], size[1]).radius(4.0).clip().sense().focusable().open();
        self.keep_note(&held, Role::Slider, None, name, "");
        self.retell_as_value(&held, Role::Slider, [value * most, 0.0, most]);
        self.tell_step(fine * most);
        if which == SOLID {
            self.el().grow().fill(Color::WHITE).shape(&checks(size), Color::rgb(0xc8c8c8)).show();
        }
        self.el().picture(picture).grow().show();
        self.el().grow().outline(&mark, 4.0, Color::BLACK.with_alpha(0.55)).show();
        let line = if held.focused && self.focus_visible() { t.accent } else { Color::WHITE };
        self.el().grow().outline(&mark, 2.0, line).show();
        self.close();
        to
    }

    /// The picker's pictures: made the first time, redrawn when the colour changes.
    fn picker_pictures(&mut self, color: Color) -> [Picture; 3] {
        let (hue, plain) = (self.picking.hsv[0], color.solid());
        let pictures = match self.picking.pictures {
            Some(pictures) => pictures,
            None => [self.add_picture(FINE, vec![0; FINE * FINE]), self.add_picture(1, vec![0; FINE]), self.add_picture(1, vec![0; 2])],
        };
        if self.picking.pictures.is_none() {
            // Every hue, top to bottom: drawn once.
            let hues = (0..FINE).map(|row| Color::hsv(row as f32 / (FINE - 1) as f32, 1.0, 1.0).0).collect();
            self.set_picture(pictures[1], 1, hues);
        }
        if self.picking.drawn != Some((hue, plain)) {
            // Saturation grows to the right, value falls downwards; alpha goes from solid to
            // clear.
            let part = |at: usize| at as f32 / (FINE - 1) as f32;
            let square = (0..FINE * FINE).map(|at| Color::hsv(hue, part(at % FINE), 1.0 - part(at / FINE)).0).collect();
            self.set_picture(pictures[0], FINE, square);
            self.set_picture(pictures[2], 1, vec![plain.0, plain.0 & 0x00ff_ffff]);
        }
        (self.picking.pictures, self.picking.drawn) = (Some(pictures), Some((hue, plain)));
        pictures
    }
}
