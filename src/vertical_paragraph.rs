//! Vertical labels and paragraphs: text written top to bottom in columns right to left, laid
//! out by `vertical_layout.rs` and drawn as its glyphs' outlines.
//!
//! A label breaks into columns at its newlines only; a paragraph also where its box's height
//! ends, as CJK line breaking allows. Rich paragraphs take [`Span`]s: their sizes, faces,
//! colours, backgrounds and links; with the feature `ruby`, readings and runs set upright across
//! one em (tate-chu-yoko). Screen readers hear the text (each reading after its base, in brackets),
//! read it by column as by line, and find each character where it is drawn.
use crate::accessibility::{AccessBox, AccessNode, Letters, Role};
use crate::color::Color;
use crate::geom::{Rect, rect};
use crate::id::{ById, hash};
use crate::layout::{Flow, Length};
use crate::path::Path;
use crate::rich_text::{Span, Writings, face_of, piece_key};
use crate::scripts::Language;
use crate::text::Font;
use crate::ui::{Measure, Response, Ui};
use crate::vertical_layout::{Asked, Laid, middles};
use std::sync::Arc;

/// The code vertical text runs outside its own calls, handed over by the first vertical text
/// described (`VERTICAL`), so a program with none carries none of it.
pub(crate) struct Hooks {
    /// After layout, before scrolling.
    pub settle: fn(&mut Ui),
    /// Gives the nodes of vertical text their characters' places.
    pub tell_letters: fn(&Writings, &mut [AccessNode], &[AccessBox]),
    /// Frees what vertical text keeps.
    free: fn(&mut Held),
}

/// The hooks, once vertical text is described.
pub(crate) static VERTICAL: std::sync::OnceLock<&'static Hooks> = std::sync::OnceLock::new();

/// The one set of hooks.
static HOOKS: Hooks = Hooks { settle, tell_letters, free };

/// What vertical text keeps, held with rich text's: none until vertical text is described.
/// Freed through `VERTICAL`, so a program with no vertical text carries no code for it.
#[derive(Default)]
pub(crate) struct Held(std::mem::ManuallyDrop<Option<Box<Kept>>>);

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(vertical) = VERTICAL.get() {
            (vertical.free)(self);
        }
    }
}

/// Frees a `Held`'s state, as it is dropped.
fn free(held: &mut Held) {
    // SAFETY: this is called once, from `Held::drop`; the state is not touched after.
    unsafe { std::mem::ManuallyDrop::drop(&mut held.0) };
}

/// What is kept: each paragraph and text box as last laid out, by its box's id; how many
/// digits in a row are set across one em; the characters' places told (by the box told to
/// readers, the box holding the text, and the places from that box's corner): in this pass, and
/// in the last one settled (what screen readers are told); and the text box being edited.
#[derive(Default)]
pub(crate) struct Kept {
    pub shown: ById<Shown>,
    pub digits: u8,
    pub told: Vec<(u32, u32, Arc<Letters>)>,
    pub settled: Vec<(u32, u32, Arc<Letters>)>,
    pub edit: crate::vertical_text_box::Edited,
}

/// A paragraph or text box as last laid out: a hash of what it was given, the room it was laid
/// out in, its layout, each piece's outlines (from the paragraph's top left) and its
/// characters' places; for a text box, the box holding its text (by index this run) and where
/// that box's corner lay in the window at the last layout; and whether it was described this
/// run.
#[derive(Default)]
pub(crate) struct Shown {
    pub key: u64,
    pub room: f32,
    pub laid: Laid,
    pub paths: Vec<Path>,
    pub letters: Option<Arc<Letters>>,
    pub content: u32,
    pub corner: Option<crate::geom::Point>,
    pub used: bool,
}

impl Ui {
    /// Vertical text, top to bottom, in the normal font and colour, as CJK text is written; its
    /// lines (at its line ends) go right to left. Letters of horizontal scripts are rotated a
    /// quarter clockwise to read down the column; digits set across one em as
    /// [`upright_digits`](Self::upright_digits) says. Written in the app's font where it has
    /// the letters, else the system's CJK font. Screen readers read it by column.
    pub fn vertical_label(&mut self, text: &str) -> Response {
        self.vertical_text(&[Span::text(text)], false).0
    }

    /// A paragraph of vertical text: as [`vertical_label`](Self::vertical_label), its columns
    /// also breaking where its box's height ends, as CJK line breaking allows. Its box grows to
    /// fill the height it has.
    pub fn vertical_paragraph(&mut self, text: &str) -> Response {
        self.vertical_text(&[Span::text(text)], true).0
    }

    /// A rich paragraph of vertical text: pieces each with its own style, as [`Ui::rich`] takes
    /// them, in columns as [`vertical_paragraph`](Self::vertical_paragraph) breaks them. With the
    /// feature `ruby`, a piece's reading (`Span::ruby`) stands at its right, and a piece set
    /// `Span::upright` stands across one em (tate-chu-yoko). Returns the index of the link piece
    /// pressed, if any. (Icons and pictures are not written in vertical text.)
    pub fn vertical_rich(&mut self, pieces: &[Span]) -> Option<usize> {
        self.vertical_text(pieces, true).1
    }

    /// Sets runs of up to `most` ASCII digits (and no more digits either side) across one em in
    /// vertical text described from now on: "12" in a date, as CSS `text-combine-upright:
    /// digits` does; at most 4. 0 (the start) sets none.
    pub fn upright_digits(&mut self, most: u8) {
        let mut kept = self.take_vertical();
        kept.digits = most.min(4);
        self.keep_vertical(kept);
    }

    /// Vertical text of `pieces`, in columns at its newlines and (`wrap`) where its box's height
    /// ends: its box, and the link piece pressed.
    fn vertical_text(&mut self, pieces: &[Span], wrap: bool) -> (Response, Option<usize>) {
        let (id, at) = (self.next_id(), self.tree.specs.len() as u32);
        let read = self.rooms.get(&id).copied().unwrap_or([self.size().w, self.size().h, 0.0]);
        let room = if wrap { read[1].max(1.0) } else { f32::INFINITY };
        let mut kept = self.take_vertical();
        let shown = kept.shown.entry(id).or_default();
        self.lay_vertical(shown, pieces, room, kept.digits);
        let (wide, long) = (shown.laid.wide.ceil(), shown.laid.long.ceil());
        let frame = self.el().flow(Flow::Stack).w(Length::Px(wide));
        let frame = if wrap { frame.h(Length::Grow(1.0)) } else { frame.h(Length::Px(long)) };
        let whole = frame.open();
        if wrap {
            self.measures.push(Measure { at, within: at, id, read });
        }
        // (Text holds no other parts: its links come after it.)
        let inside = self.tree.specs.len() as u32;
        let words = self.el().flow(Flow::Stack).size(wide, long).open();
        self.tell(&words, Role::Text, None, &shown.laid.text);
        self.write_backgrounds(&shown.laid);
        self.write_glyphs(shown);
        self.close();
        let pressed = self.write_links(&shown.laid);
        self.close();
        if let Some(letters) = shown.letters.clone() {
            kept.told.push((inside, inside, letters));
        }
        shown.used = true;
        self.keep_vertical(kept);
        (whole, pressed.checked_sub(1).map(|link| link as usize))
    }

    /// Lays a paragraph out afresh where its pieces, theme, digits or room changed.
    pub(crate) fn lay_vertical(&mut self, shown: &mut Shown, pieces: &[Span], room: f32, digits: u8) {
        let t = self.theme;
        let key = pieces.iter().fold(hash((t.font.size.to_bits(), t.font.face, t.text, t.accent, digits)), |key, piece| crate::id::fold(key, piece_key(piece)));
        if key == shown.key && room == shown.room {
            return;
        }
        (shown.key, shown.room) = (key, room);
        let mut laid = Laid::default();
        for piece in pieces {
            let size = if piece.size > 0.0 { piece.size } else { t.font.size };
            let font = Font { face: face_of(piece.face.unwrap_or(t.font.face), piece.look), size, decoration: piece.look.rule };
            let link = if piece.look.link != 0 { laid.pieces.len() as u32 + 1 } else { 0 };
            let color = piece.color.unwrap_or(if link != 0 { t.accent } else { t.text });
            #[cfg(feature = "ruby")]
            let (reading, upright) = (piece.ruby, piece.upright);
            #[cfg(not(feature = "ruby"))]
            let (reading, upright) = ("", false);
            let given = laid.add(piece.words, font, color, reading);
            (given.back, given.link, given.upright) = (piece.back.unwrap_or(Color::NONE), link, upright);
        }
        let system = crate::vertical_text::system_font();
        laid.lay(self.text.as_mut(), system.as_deref(), Asked { language: Language::of_the_system(), digits, room });
        shown.paths = vec![Path::new(); laid.pieces.len()];
        let middles = middles(&laid.columns, laid.wide.ceil());
        for (column, x) in laid.columns.iter().zip(&middles) {
            crate::vertical_layout::outlines(&laid, self.text.as_mut(), system.as_deref(), column, [*x, 0.0], &mut shown.paths);
        }
        shown.letters = Some(Arc::new(letters_of(&laid, &middles, key)));
        shown.laid = laid;
    }

    /// Each piece's background, behind each column's stretch of it.
    fn write_backgrounds(&mut self, laid: &Laid) {
        for (column, place) in stretches(laid, |piece| laid.pieces[piece].back != Color::NONE) {
            let color = laid.pieces[column].back;
            self.el().offset(place.x, place.y).size(place.w, place.h).fill(color).radius(3.0).show();
        }
    }

    /// The glyphs: one shape per colour.
    fn write_glyphs(&mut self, shown: &Shown) {
        let mut colors: Vec<Color> = shown.laid.pieces.iter().map(|piece| piece.color).collect();
        colors.dedup();
        let (wide, long) = (shown.laid.wide.ceil(), shown.laid.long.ceil());
        for color in colors {
            let mut path = Path::new();
            let pieces = shown.laid.pieces.iter().zip(&shown.paths).filter(|(piece, _)| piece.color == color);
            pieces.for_each(|(_, outline)| _ = path.add(outline));
            self.el().size(wide, long).shape(&path, color).show();
        }
    }

    /// A box over each column's stretch of each link: pressable, its first a Tab stop told as
    /// the link; and a line beside it while hovered or focused. Returns the link pressed, from 1
    /// (0 for none).
    fn write_links(&mut self, laid: &Laid) -> u32 {
        let mut pressed = 0;
        let mut part = (0, 0);
        for (piece, place) in stretches(laid, |piece| laid.pieces[piece].link != 0) {
            let link = laid.pieces[piece].link;
            part = if part.0 == link { (link, part.1 + 1) } else { (link, 0) };
            let state = self.link_state(link, part.1);
            if state.hovered || state.focused {
                self.el().offset(place.x - 1.0, place.y).size(1.0, place.h).fill(laid.pieces[piece].color).show();
            }
            pressed = if state.clicked { link } else { pressed };
            let words = &laid.text[laid.pieces[piece].words[0] as usize..laid.pieces[piece].words[1] as usize];
            self.link_box(link, part.1, place, words);
        }
        pressed
    }

    /// The vertical text state, made on first use; and its hooks handed over.
    pub(crate) fn take_vertical(&mut self) -> Box<Kept> {
        VERTICAL.get_or_init(|| &HOOKS);
        let mut writings = self.take_writings();
        let kept = writings.vertical.0.take();
        self.keep_writings(writings);
        kept.unwrap_or_default()
    }

    /// Puts the vertical text state back.
    pub(crate) fn keep_vertical(&mut self, kept: Box<Kept>) {
        let mut writings = self.take_writings();
        *writings.vertical.0 = Some(kept);
        self.keep_writings(writings);
    }
}

/// What a window's vertical text keeps, if it has any.
fn kept_of(writings: &Writings) -> Option<&Kept> {
    let rich = writings.kept.as_ref()?.downcast_ref::<crate::rich_text::Kept>()?;
    rich.vertical.0.as_deref()
}

/// After layout, in a window with vertical text: the paragraphs not described are forgotten,
/// the rest marked unused for the next run; where boxes holding text lie is kept; a vertical
/// text box's cursor is placed (see `vertical_text_box`).
fn settle(ui: &mut Ui) {
    if kept_of(&ui.writings).is_none() {
        return;
    }
    let mut kept = ui.take_vertical();
    kept.shown.retain(|_, shown| std::mem::take(&mut shown.used));
    kept.settled = std::mem::take(&mut kept.told);
    for shown in kept.shown.values_mut().filter(|shown| shown.content != 0) {
        let corner = ui.tree.rect_of(shown.content);
        shown.corner = Some(crate::geom::pt(corner.x, corner.y));
    }
    crate::vertical_text_box::place_cursor(ui, &mut kept);
    ui.keep_vertical(kept);
}

/// Gives the nodes of vertical text settled last their characters' places (see `Letters`): as
/// the box holding them lies now.
fn tell_letters(writings: &Writings, nodes: &mut [AccessNode], boxes: &[AccessBox]) {
    let Some(kept) = kept_of(writings) else { return };
    for node in nodes.iter_mut().filter(|node| node.visible) {
        let Some((_, holder, letters)) = kept.settled.iter().find(|(at, ..)| *at == node.at) else { continue };
        let Some((_, place, ..)) = boxes.iter().find(|(at, ..)| at == holder) else { continue };
        (node.letters, node.text_box, node.text_rect) = (Some(letters.clone()), *holder, *place);
    }
}

/// Where each character of a laid-out paragraph is (each column a line), its columns' middles
/// at `middles` and tops at 0. A character with no place of its own (a newline, a reading's
/// bracket) lies empty where the one before it ends.
pub(crate) fn letters_of(laid: &Laid, middles: &[f32], key: u64) -> Letters {
    let mut letters = Letters { key, lines: Vec::new(), places: Vec::new() };
    let mut places: Vec<(u32, Rect)> = Vec::new();
    for (column, x) in laid.columns.iter().zip(middles) {
        letters.lines.push(laid.text[..laid.steps.get(column.steps[0] as usize).map_or(laid.text.len(), |step| step.bytes[0] as usize)].chars().count() as u32);
        for (n, at) in (column.steps[0]..column.steps[1]).enumerate() {
            let step = laid.steps[at as usize];
            let found = &laid.places[step.places[0] as usize..step.places[1] as usize];
            places.extend(found.iter().map(|place| (place.bytes[0], place.rect.moved(*x, column.tops[n]))));
        }
    }
    places.sort_by_key(|(byte, _)| *byte);
    let mut last = rect(middles.first().copied().unwrap_or(0.0), 0.0, 0.0, 0.0);
    for (byte, _) in laid.text.char_indices() {
        let found = places.binary_search_by_key(&(byte as u32), |(at, _)| *at).ok().map(|n| places[n].1);
        let place = found.unwrap_or(rect(last.x, last.bottom(), last.w, 0.0));
        letters.places.push(place);
        last = place;
    }
    if letters.lines.first() != Some(&0) {
        letters.lines.insert(0, 0);
    }
    letters.lines.dedup();
    letters
}

/// Each column's stretches of the pieces `wanted` picks: the piece, and the stretch's box
/// (from the paragraph's top left, as wide as the piece's line).
pub(crate) fn stretches(laid: &Laid, wanted: impl Fn(usize) -> bool) -> Vec<(usize, Rect)> {
    let middles = middles(&laid.columns, laid.wide.ceil());
    let mut found: Vec<(usize, Rect)> = Vec::new();
    for (column, x) in laid.columns.iter().zip(&middles) {
        let mut last: Option<usize> = None;
        for (n, at) in (column.steps[0]..column.steps[1]).enumerate() {
            let step = laid.steps[at as usize];
            let piece = step.piece as usize;
            let line = laid.lines[piece];
            let place = rect(x - line / 2.0, column.tops[n], line, step.long);
            match found.last_mut().filter(|(was, _)| last == Some(piece) && *was == piece) {
                Some((_, stretch)) => *stretch = stretch.union(place),
                None if wanted(piece) => found.push((piece, place)),
                None => {}
            }
            last = Some(piece);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vertical_text::tests::writing_down;

    /// A window written in the vertical test font, with a screen reader listening.
    fn window() -> Ui {
        let mut ui = Ui::new(Box::new(crate::font_file::AppFonts::new(writing_down(true, false))), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: 400.0, h: 300.0 }, 1.0);
        ui.set_screen_reader(true);
        ui
    }

    #[test]
    fn a_paragraph_breaks_into_columns_where_its_box_ends_and_is_read_by_column() {
        let mut ui = window();
        ui.theme.font.size = 20.0;
        let build = |ui: &mut Ui| {
            ui.row().pad(10.0).h(crate::layout::Length::Px(100.0)).open();
            ui.vertical_paragraph("ああああ。あああああ");
            ui.close();
        };
        ui.run(&mut |ui: &mut Ui| build(ui));
        ui.run(&mut |ui: &mut Ui| build(ui));
        // Four to a column of 80 (the box less its padding), the full stop with the fourth.
        let node = ui.visible_access_nodes().iter().find(|node| node.name == "ああああ。あああああ").cloned().expect("told");
        let letters = node.letters.clone().expect("its letters");
        assert_eq!(letters.lines, [0, 3, 7]);
        // The first column at the right, its first letter at the top; the next to its left.
        let (first, second) = (node.letter_rect(0), node.letter_rect(3));
        assert!(first.x > second.x && first.y == second.y && first.y == node.rect.y, "{first:?} {second:?}");
    }

    #[test]
    fn a_link_in_vertical_text_is_pressed_and_told() {
        let mut ui = window();
        let mut pressed = None;
        let mut build = |ui: &mut Ui| {
            ui.column().pad(10.0).open();
            pressed = pressed.or(ui.vertical_rich(&[Span::text("ああ"), Span::text("あ").link()]));
            ui.close();
        };
        ui.run(&mut build);
        _ = ui.event(crate::event::Event::Key(crate::event::Key::Tab, crate::event::Mods::NONE));
        ui.run(&mut build);
        assert!(ui.visible_access_nodes().iter().any(|node| node.role == Role::Link && node.name == "あ"));
        _ = ui.event(crate::event::Event::Key(crate::event::Key::Enter, crate::event::Mods::NONE));
        ui.run(&mut build);
        assert_eq!(pressed, Some(1));
    }
}

/// Vertical text held against Chrome's (Edge's) rendering of the same text with CSS
/// `writing-mode: vertical-rl`, `text-combine-upright` and `<ruby>`, in the same font (Yu
/// Gothic), at the same places: every cluster's ink, piece by piece in colours told apart,
/// within a pixel. (Windows, with Edge or Chrome installed; else not tested.)
///
/// Chrome sets a run across one em off the column's middle, by half of the font's line (its
/// Windows ascent and descent, rounded) less 1.1 em; the library centres it, as CSS Writing
/// Modes has it. That difference is taken out before comparing.
#[cfg(all(test, windows, feature = "ruby"))]
mod against_edge {
    use super::*;
    use crate::browser_check::{browser, ink_middle, ink_of, lasting_differences};
    use crate::font_file::FontFile;

    /// The colours pieces are written in, in turn.
    const INKS: [u32; 5] = [0x000000, 0xd00000, 0x008000, 0x0000ff, 0xb000b0];

    /// Yu Gothic's Windows ascent and descent, in its units per em: Chrome's line on Windows.
    const WINDOWS_LINE: [f32; 3] = [2017.0, 619.0, 2048.0];

    /// How far right of the column's middle Chrome sets a run across one em, at a size.
    fn chrome_across(size: f32) -> f32 {
        let [ascent, descent, units] = WINDOWS_LINE;
        ((ascent * size / units).round() + (descent * size / units).round() - 1.1 * size) / 2.0
    }

    /// A piece: its text, reading, and whether set across.
    type Given = (&'static str, &'static str, bool);

    /// A paragraph: where its box is, its height and font size, and its pieces.
    struct Case {
        at: [f32; 2],
        high: f32,
        size: f32,
        pieces: &'static [Given],
    }

    const CASES: [Case; 6] = [
        // Line breaking: kinsoku before closing punctuation, small kana and the ellipsis,
        // after opening brackets.
        Case {
            at: [10.0, 10.0], high: 120.0, size: 20.0, pieces: &[("あいうえお。かきくけこ、さしすせそ", "", false), ("「たちつてと」なにぬねの…はひふへほっ", "", false)]
        },
        // Latin and digits rotated among upright text, and punctuation in vertical forms.
        Case { at: [190.0, 10.0], high: 300.0, size: 20.0, pieces: &[("縦書きでRust 1.90と", "", false), ("「（注）」ー〜……、。", "", false)] },
        // Tate-chu-yoko: two digits fit an em; three and four take narrower forms.
        Case {
            at: [260.0, 10.0],
            high: 300.0,
            size: 20.0,
            pieces: &[("あ", "", false), ("12", "", true), ("い", "", false), ("123", "", true), ("う", "", false), ("2026", "", true), ("え", "", false)],
        },
        // Ruby in a first column: the column makes room for the readings at its right.
        Case { at: [320.0, 10.0], high: 300.0, size: 20.0, pieces: &[("あい", "", false), ("漢", "かん", false), ("字う", "", false), ("書", "か", false), ("く", "", false)] },
        // Readings spread over a longer base; a longer reading reaching over its neighbours.
        Case { at: [380.0, 10.0], high: 300.0, size: 20.0, pieces: &[("あ", "", false), ("漢字", "かんじ", false), ("い", "", false), ("漢", "かんじ", false), ("う", "", false)] },
        // Ruby in a later column, in the room the column before leaves.
        Case { at: [440.0, 10.0], high: 100.0, size: 24.0, pieces: &[("あいうえ", "", false), ("漢字", "かんじ", false), ("を書く", "", false)] },
    ];

    /// Rich text with readings over it, a line each: single bases under longer and shorter
    /// readings; a reading spread over its base, and one reaching over its neighbours.
    const ACROSS: [Case; 2] = [
        Case { at: [10.0, 30.0], high: 0.0, size: 20.0, pieces: &[("あい", "", false), ("漢", "かん", false), ("字う", "", false), ("書", "か", false), ("く", "", false)] },
        Case { at: [10.0, 100.0], high: 0.0, size: 20.0, pieces: &[("あ", "", false), ("漢字", "かんじ", false), ("い", "", false), ("漢", "かんじ", false), ("う", "", false)] },
    ];

    /// The cases as a page, each piece a span in its colour, each line as tall as the font's
    /// line in the library (`line`, by size): vertical, or (`across`) horizontal.
    fn page(font: &str, cases: &[Case], across: bool, line: impl Fn(f32) -> f32) -> String {
        let way = if across { "horizontal-tb; white-space: nowrap" } else { "vertical-rl" };
        let mut html = format!(
            "<!doctype html><meta charset=utf-8><style>@font-face {{ font-family: Probe; src: url('file:///{font}'); }} body {{ margin: 0; background: #fff; }} div {{ position: absolute; writing-mode: {way}; font-family: Probe; }}</style>"
        );
        for case in cases {
            let style = format!("left: {}px; top: {}px; height: {}px; font-size: {}px; line-height: {}px", case.at[0], case.at[1], case.high, case.size, line(case.size));
            html += &format!("<div style='{style}'>");
            for (n, (words, reading, across)) in case.pieces.iter().enumerate() {
                let ink = format!("color: #{:06x}", INKS[n % INKS.len()]);
                let combined = if *across { "; text-combine-upright: all" } else { "" };
                html += &match reading.is_empty() {
                    true => format!("<span style='{ink}{combined}'>{words}</span>"),
                    false => format!("<ruby style='{ink}'>{words}<rt>{reading}</rt></ruby>"),
                };
            }
            html += "</div>";
        }
        html
    }

    /// The same, written by the library.
    fn written(ui: &mut Ui) {
        ui.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Grow(1.0)).open();
        for case in &CASES {
            ui.el().offset(case.at[0], case.at[1]).h(Length::Px(case.high)).open();
            let pieces: Vec<Span> = case
                .pieces
                .iter()
                .enumerate()
                .map(|(n, (words, reading, across))| {
                    let span = Span::text(words).ruby(reading).size(case.size).color(Color::rgb(INKS[n % INKS.len()]));
                    if *across { span.upright() } else { span }
                })
                .collect();
            ui.vertical_rich(&pieces);
            ui.close();
        }
        ui.close();
    }

    /// The same horizontally, written by the library: rich text.
    fn written_across(ui: &mut Ui) {
        ui.el().flow(Flow::Stack).w(Length::Grow(1.0)).h(Length::Grow(1.0)).open();
        for case in &ACROSS {
            ui.el().offset(case.at[0], case.at[1]).w(Length::Px(400.0)).open();
            let pieces: Vec<Span> = case.pieces.iter().enumerate().map(|(n, (words, reading, _))| Span::text(words).ruby(reading).size(case.size).color(Color::rgb(INKS[n % INKS.len()]))).collect();
            ui.rich(&pieces);
            ui.close();
        }
        ui.close();
    }

    /// Ours, drawn: the cases `write` describes, in Yu Gothic, `width` by `height`.
    fn drawn(font: FontFile, write: fn(&mut Ui), [width, height]: [usize; 2], name: &str) -> (Ui, Vec<u32>) {
        let mut ui = Ui::new(Box::new(crate::font_file::AppFonts::new(font)), Box::new(crate::platform::LocalClipboard::default()));
        ui.resize(crate::geom::Size { w: width as f32, h: height as f32 }, 1.0);
        ui.theme.bg = Color::WHITE;
        ui.run(&mut |ui: &mut Ui| write(ui));
        let ours = crate::surface::Surface::draw(&mut ui, 64);
        if let Ok(shots) = std::env::var("TAUT_SHOTS") {
            std::fs::write(std::path::Path::new(&shots).join(format!("{name}-taut.png")), crate::png::encode(ours.width, &ours.pixels).unwrap()).unwrap();
        }
        (ui, ours.pixels)
    }

    /// Yu Gothic, and where it is: or none, where not installed.
    fn yu_gothic() -> Option<(FontFile, String)> {
        let path = std::path::Path::new(&std::env::var_os("WINDIR").unwrap_or_default()).join(r"Fonts\YuGothR.ttc");
        Some((FontFile::open(&path)?, path.to_string_lossy().replace('\\', "/")))
    }

    /// Where a piece's ink lies within a line's band: its base's middle, and its reading's (the
    /// ink above the row with least ink between them), if it has one (`reading`).
    fn base_and_reading(pixels: &[u32], width: usize, band: Rect, color: u32, reading: bool) -> Option<([f32; 2], Option<[f32; 2]>)> {
        let ink = |y: usize| (band.x as usize..band.right() as usize).map(|x| ink_of(pixels[y * width + x], color, &INKS)).sum::<f32>();
        let rows: Vec<f32> = (band.y as usize..band.bottom() as usize).map(ink).collect();
        let top = rows.iter().position(|ink| *ink > 0.5)?;
        let cut = |from: usize, to: usize| rect(band.x, band.y + from as f32, band.w, (to - from) as f32);
        if !reading {
            return Some((ink_middle(pixels, width, cut(top, rows.len()), color, &INKS)?, None));
        }
        let gap = (top + 4..top + 14).min_by(|one, other| rows[*one].total_cmp(&rows[*other]))?;
        Some((ink_middle(pixels, width, cut(gap, rows.len()), color, &INKS)?, ink_middle(pixels, width, cut(top, gap), color, &INKS)))
    }

    #[test]
    fn readings_over_rich_text_lie_where_chrome_sets_them() {
        let Some(browser) = browser() else { return eprintln!("no Edge or Chrome here: not compared") };
        let Some((font, path)) = yu_gothic() else { return eprintln!("no Yu Gothic here: not compared") };
        let line = crate::vertical_layout::line_of(&font, 20.0);
        let size = [420, 160];
        let (_, ours) = drawn(font, written_across, size, "ruby");
        let differ = |theirs: &[u32]| {
            let mut wrong = Vec::new();
            for case in &ACROSS {
                let band = rect(case.at[0], case.at[1] - 20.0, 400.0, 60.0);
                let found = |pixels: &[u32], n: usize| base_and_reading(pixels, size[0], band, INKS[n], !case.pieces[n].1.is_empty()).expect("the piece is drawn");
                let (mine, edge) = (found(&ours, 0), found(theirs, 0));
                for (n, piece) in case.pieces.iter().enumerate().skip(1) {
                    let ((my_base, my_reading), (edge_base, edge_reading)) = (found(&ours, n), found(theirs, n));
                    // Each base against the first piece, each reading against its base.
                    let pairs = [([my_base, mine.0], [edge_base, edge.0])].into_iter().chain(my_reading.zip(edge_reading).map(|(mine, edge)| ([mine, my_base], [edge, edge_base])));
                    for (pair, ([one, from], [other, other_from])) in pairs.enumerate() {
                        let (ours, chrome) = ([one[0] - from[0], one[1] - from[1]], [other[0] - other_from[0], other[1] - other_from[1]]);
                        if (ours[0] - chrome[0]).abs() > 1.0 || (ours[1] - chrome[1]).abs() > 1.0 {
                            let what = if pair == 0 { "base" } else { "reading" };
                            wrong.push((format!("{piece:?}'s {what}"), format!("ours {ours:?}, Edge's {chrome:?}")));
                        }
                    }
                }
            }
            wrong
        };
        let wrong = lasting_differences(&browser, &page(&path, &ACROSS, true, |_| line), size, "ruby", differ);
        assert!(wrong.is_empty(), "{} readings or bases lie elsewhere than in Chrome", wrong.len());
    }

    #[test]
    fn vertical_text_lies_where_chrome_lays_it() {
        let Some(browser) = browser() else { return eprintln!("no Edge or Chrome here: not compared") };
        let Some((font, path)) = yu_gothic() else { return eprintln!("no Yu Gothic here: not compared") };
        let html = page(&path, &CASES, false, |size| crate::vertical_layout::line_of(&font, size));
        let (width, height) = (560, 330);
        let (mut ui, ours) = drawn(font, written, [width, height], "vertical");
        let kept = ui.take_vertical();
        let differ = |theirs: &[u32]| {
            let (mut wrong, mut compared) = (Vec::new(), 0);
            for case in &CASES {
                let text: String = case.pieces.iter().map(|(words, reading, _)| if reading.is_empty() { words.to_string() } else { format!("{words}({reading})") }).collect();
                let shown = kept.shown.values().find(|shown| shown.laid.text == text).expect("the case is shown");
                let laid = &shown.laid;
                let middles = middles(&laid.columns, laid.wide.ceil());
                for (column, x) in laid.columns.iter().zip(&middles) {
                    for (n, at) in (column.steps[0]..column.steps[1]).enumerate() {
                        let step = laid.steps[at as usize];
                        let color = laid.pieces[step.piece as usize].color.0 & 0xffffff;
                        for place in &laid.places[step.places[0] as usize..step.places[1] as usize] {
                            let size = laid.pieces[step.piece as usize].font.size * if place.rect.x > 0.0 { crate::ruby::READING } else { 1.0 };
                            let middle = place.rect.x + place.rect.w / 2.0;
                            let region = rect(case.at[0] + x + middle - size / 2.0 - 1.0, case.at[1] + column.tops[n] + place.rect.y - 1.0, size + 2.0, place.rect.h + 2.0);
                            let off = if laid.pieces[step.piece as usize].upright { chrome_across(size) } else { 0.0 };
                            let (mine, edge) = (ink_middle(&ours, width, region, color, &INKS), ink_middle(theirs, width, region.moved(off, 0.0), color, &INKS));
                            let edge = edge.map(|[x, y]| [x - off, y]);
                            let words = &laid.text[place.bytes[0] as usize..place.bytes[1] as usize];
                            compared += 1;
                            let near = match (mine, edge) {
                                (Some(mine), Some(edge)) => (mine[0] - edge[0]).abs() <= 1.0 && (mine[1] - edge[1]).abs() <= 1.0,
                                (None, None) => true,
                                _ => false,
                            };
                            if !near {
                                wrong.push((format!("{words:?} at {region:?}"), format!("ours {mine:?}, Edge's {edge:?}")));
                            }
                        }
                    }
                }
            }
            println!("{compared} clusters compared with {}, {} differ", browser.display(), wrong.len());
            wrong
        };
        let wrong = lasting_differences(&browser, &html, [width, height], "vertical", differ);
        assert!(wrong.is_empty(), "{} clusters lie elsewhere than in Chrome", wrong.len());
    }
}
