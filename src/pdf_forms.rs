//! Form fields in a PDF: text boxes, ticks and choices a person fills in in their viewer, and
//! that print as filled. Each is a field and its widget (the annotation showing it) in one: its
//! name, what a screen reader is told of it, its value, and how it looks (an appearance
//! stream, drawn from the paper's own fonts, as PDF/A asks). In the structure, each is a form
//! part. Only a program that makes a field carries this.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::geom::{Rect, rect};
use crate::paper::{Mark, Paper};
use crate::pdf::{File, Forms, Shared, drawn_marks, num, text};
use crate::text::Font;

/// A form field on a page: what kind, its name (also what readers are told), where, and how it
/// looks: its marks (in page units) for each state, the one shown first.
pub(crate) struct Field {
    pub kind: Kind,
    pub name: String,
    pub r: Rect,
    pub looks: Vec<Vec<Mark>>,
    pub shown: usize,
    /// The font its words are in (by the paper's font index) and their size.
    pub font: Option<(u32, f32)>,
}

/// What kind of field: text, with its value; a tick, on or off; or a choice of words.
pub(crate) enum Kind {
    Text(String),
    Tick(bool),
    Choice(Vec<String>, usize),
}

impl Paper {
    /// A text box the person can fill in, over `r`, holding `value` in `font`. `name` names it
    /// (each field's own) and is what a screen reader says of it.
    pub fn text_field(&mut self, r: Rect, name: &str, value: &str, font: Font) {
        let look = self.field_look(r, value, font, false);
        self.add_field(Field { kind: Kind::Text(value.to_string()), name: name.to_string(), r, looks: vec![look.0], shown: 0, font: look.1 });
    }
    /// A tick box over `r`, ticked or not.
    pub fn tick_field(&mut self, r: Rect, name: &str, ticked: bool) {
        let looks = vec![self.tick_look(r, true), self.tick_look(r, false)];
        self.add_field(Field { kind: Kind::Tick(ticked), name: name.to_string(), r, looks, shown: if ticked { 0 } else { 1 }, font: None });
    }
    /// A choice of `choices` over `r`, `chosen` (an index) shown in `font`: a drop-down list.
    pub fn choice_field(&mut self, r: Rect, name: &str, choices: &[&str], chosen: usize, font: Font) {
        let chosen = chosen.min(choices.len().saturating_sub(1));
        let look = self.field_look(r, choices.get(chosen).copied().unwrap_or(""), font, true);
        let kind = Kind::Choice(choices.iter().map(|choice| choice.to_string()).collect(), chosen);
        self.add_field(Field { kind, name: name.to_string(), r, looks: vec![look.0], shown: 0, font: look.1 });
    }

    /// Puts a field on the page, handing over what writes and draws fields.
    fn add_field(&mut self, field: Field) {
        self.forms = Some(Forms { write, draw });
        self.put(Mark::Field(Box::new(field)));
    }

    /// A text or choice field's look: a white box with a grey edge, its words inside (and an
    /// arrow, for a choice). Also the font its words are in.
    fn field_look(&mut self, r: Rect, words: &str, font: Font, arrow: bool) -> (Vec<Mark>, Option<(u32, f32)>) {
        let mut look = self.edge(r);
        let line = r.y + (r.h + self.metrics(font).ascent - self.metrics(font).descent) / 2.0;
        let room = if arrow { r.h } else { 0.0 };
        let written = self.aside(|paper| {
            paper.clip(rect(r.x + 2.0, r.y, (r.w - 4.0 - room).max(0.0), r.h));
            paper.line(words, r.x + 4.0, line, font, Color::BLACK);
            paper.unclip();
        });
        let font = written.iter().find_map(|mark| if let Mark::Words(words) = mark { Some((words.font, words.size)) } else { None });
        look.extend(written);
        if arrow {
            let (middle, side) = (r.y + r.h / 2.0, r.h / 6.0);
            let mut down = crate::path::Path::new();
            down.move_to(r.right() - r.h / 2.0 - side, middle - side / 2.0).line_to(r.right() - r.h / 2.0 + side, middle - side / 2.0).line_to(r.right() - r.h / 2.0, middle + side / 2.0).close();
            look.extend(self.aside(|paper| paper.shape(&down, Color::rgb(0x444444))));
        }
        (look, font)
    }

    /// A tick's look, on or off: its box, and a tick when on.
    fn tick_look(&mut self, r: Rect, on: bool) -> Vec<Mark> {
        let mut look = self.edge(r);
        if on {
            look.extend(self.aside(|paper| paper.icon(crate::widgets::CHECK, r, Color::BLACK)));
        }
        look
    }

    /// A field's box: white, with a grey edge.
    fn edge(&mut self, r: Rect) -> Vec<Mark> {
        self.aside(|paper| {
            paper.rect(r, Color::WHITE);
            paper.border(r, 0.0, 1.0, Color::rgb(0x8a8a8a));
        })
    }

    /// The marks `draw` puts on the page, taken off it again: part of no structure.
    fn aside(&mut self, draw: impl FnOnce(&mut Paper)) -> Vec<Mark> {
        if self.pages.is_empty() {
            self.page(Paper::A4);
        }
        let (on, before, open) = (self.on, self.pages[self.on].marks.len(), self.open.len());
        self.begin_decoration();
        draw(self);
        self.open.truncate(open);
        self.pages[on].marks.split_off(before)
    }
}

/// Draws a field as shown.
fn draw(paper: &Paper, field: &Field, canvas: &mut Canvas) {
    for mark in &field.looks[field.shown.min(field.looks.len() - 1)] {
        match mark {
            Mark::Shape(drawn) => drawn.draw(canvas),
            Mark::Words(words) => paper.draw_words(words, canvas),
            _ => {}
        }
    }
}

/// Writes a field and its widget: its kind, name, what it is told as, value, appearance in each
/// state, and its place in the structure. `[page, mark]`, then the page's object, the shared
/// resources and its parent-tree key. Gives back its number.
fn write(file: &mut File, paper: &Paper, shared: &mut Shared, [page, at]: [usize; 2], [kid, resources, key]: [u32; 3]) -> u32 {
    let Mark::Field(field) = &paper.pages[page].marks[at] else { return 0 };
    let height = paper.pages[page].size.h;
    let r = field.r;
    let number = file.number();
    let corners = [r.x * 0.75, (height - r.bottom()) * 0.75, r.right() * 0.75, (height - r.y) * 0.75].map(num).join(" ");
    let mut looks = Vec::new();
    for look in &field.looks {
        let shown = file.number();
        // (Drawn in page units, moved so the field's bottom left is the stream's origin.)
        let marks = format!("q 0.75 0 0 -0.75 {} {} cm\n{}Q\n", num(-r.x * 0.75), num(r.bottom() * 0.75), drawn_marks(paper, look, shared));
        let bounds = [0.0, 0.0, r.w * 0.75, r.h * 0.75].map(num).join(" ");
        file.stream(shown, &format!("/Type /XObject /Subtype /Form /BBox [{bounds}] /Resources {resources} 0 R"), marks.as_bytes());
        looks.push(shown);
    }
    let said = match &field.kind {
        Kind::Text(value) => format!("/FT /Tx /V {} /AP << /N {} 0 R >>", text(value), looks[0]),
        Kind::Tick(on) => {
            let state = if *on { "/Yes" } else { "/Off" };
            format!("/FT /Btn /V {state} /AS {state} /AP << /N << /Yes {} 0 R /Off {} 0 R >> >>", looks[0], looks[1])
        }
        Kind::Choice(choices, chosen) => {
            let listed: String = choices.iter().map(|choice| format!("{} ", text(choice))).collect();
            format!("/FT /Ch /Ff 131072 /Opt [{listed}] /V {} /AP << /N {} 0 R >>", text(choices.get(*chosen).map_or("", String::as_str)), looks[0])
        }
    };
    let font = field.font.map_or(String::new(), |(font, size)| format!(" /DA (/F{font} {} Tf 0 g)", num(size * 0.75)));
    let name = text(&field.name);
    file.object(number, &format!("<< /Type /Annot /Subtype /Widget /Rect [{corners}] /F 4 /P {kid} 0 R /StructParent {key} /T {name} /TU {name} {said}{font} >>"));
    number
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::pdf::tests::latin;
    use crate::text::Face;

    /// A page with one field of each kind.
    pub(crate) fn form() -> Paper {
        let mut paper = Paper::new();
        paper.title = "A form".into();
        if let Some(font) = latin() {
            paper.font(Face::Regular, font);
        }
        paper.page(Paper::A4);
        paper.text("Name", 72.0, 72.0, Font::new(14.0), Color::BLACK);
        paper.text_field(rect(160.0, 66.0, 240.0, 28.0), "Name", "Ada Lovelace", Font::new(14.0));
        paper.tick_field(rect(72.0, 120.0, 20.0, 20.0), "Subscribe", true);
        paper.choice_field(rect(160.0, 160.0, 200.0, 28.0), "Colour", &["Red", "Green", "Blue"], 1, Font::new(14.0));
        paper
    }

    #[test]
    fn fields_go_in_the_file_with_their_names_values_and_looks() {
        let paper = form();
        let file = String::from_utf8_lossy(&paper.to_pdf()).into_owned();
        assert_eq!(file.matches("/Subtype /Widget").count(), 3);
        assert!(file.contains("/AcroForm << /Fields [") && file.contains("/FT /Tx") && file.contains("/FT /Btn /V /Yes /AS /Yes") && file.contains("/FT /Ch /Ff 131072"));
        assert_eq!(file.matches("/S /Form").count(), 3, "each a form part in the structure");
        assert_eq!(file.matches("/Type /OBJR").count(), 3);
        // What the person sees, drawn on the page as the viewer draws it.
        let (width, pixels) = paper.render_page(0, 1.0);
        let at = |x: usize, y: usize| pixels[y * width + x] & 0xff_ffff;
        assert_eq!(at(160, 80), 0x8a8a8a, "a text box's edge");
        assert!((120..140).any(|y| (72..92).any(|x| at(x, y) == 0)), "a tick");
    }

    #[test]
    fn a_reader_finds_the_fields_and_their_values() {
        let paper = form();
        let Some(read) = crate::pdf::readers::text_of(&paper.to_pdf()) else { return eprintln!("no pdftotext here: not read back") };
        assert!(read.contains("Ada Lovelace") && read.contains("Green"), "{read}");
    }
}
