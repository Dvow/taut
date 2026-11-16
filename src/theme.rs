//! The colours and sizes an interface is drawn with: dark and light sets, and a pair for high
//! contrast that Windows' own contrast colours can replace. Any can be built around one colour
//! (`accented`), made tighter or looser (`dense`, `roomy`), or given larger text
//! (`text_scaled`); `following` makes the one that follows all the person asked their system
//! for. An app can change any part, or bring its own.
//!
//! Everything made here keeps text readable on what it is drawn on, by the WCAG contrast ratio
//! (1 for equal luminance, 21 for black on white). Text keeps 7:1 in high contrast; a person's
//! own colour is lightened or darkened until it reads as text on the background.
use crate::color::Color;
use crate::text::Font;
use crate::wishes::Wishes;

/// The colours and sizes everything is drawn with: a plain value, to change any part of.
#[derive(Clone, Copy, Debug, PartialEq)]
#[doc(alias("style"))]
pub struct Theme {
    /// The window behind everything.
    pub bg: Color,
    /// Panels and cards.
    pub panel: Color,
    /// What rises above a panel: a button, a menu.
    pub raise: Color,
    /// What the pointer is over.
    pub hover: Color,
    /// What is picked.
    pub selected: Color,
    /// Lines between things.
    pub line: Color,
    /// Stronger lines.
    pub line_strong: Color,
    /// Normal text.
    pub text: Color,
    /// Quieter text.
    pub text_dim: Color,
    /// The quietest text: hints, disabled parts.
    pub text_faint: Color,
    /// The accent colour.
    pub accent: Color,
    /// Text on the accent.
    pub on_accent: Color,
    /// Red: failure, removal.
    pub red: Color,
    /// Amber: needs attention.
    pub amber: Color,
    /// Green: success, addition.
    pub green: Color,
    /// Blue: information.
    pub blue: Color,
    /// Normal text. (Scale text with `text_scaled`, which keeps the heights holding it in
    /// step.)
    pub font: Font,
    /// Smaller text beside it.
    pub small: Font,
    /// Corner radius.
    pub radius: f32,
    /// The height of a button, a text box and every part holding a line of text: that line in
    /// the normal font, plus padding.
    pub control: f32,
    /// The usual gap between things.
    pub gap: f32,
}

/// A high-contrast theme's colours by role, as Windows' contrast themes let a person pick them:
/// background and its text, links, disabled, selected (or pressed, or active) and its text, and
/// buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contrast {
    /// The window background.
    pub ground: Color,
    /// Text on the background, and outlines.
    pub text: Color,
    /// Links.
    pub link: Color,
    /// Disabled parts.
    pub off: Color,
    /// Selected, pressed or active parts.
    pub picked: Color,
    /// Text on selected parts.
    pub on_picked: Color,
    /// Buttons.
    pub button: Color,
    /// Text on buttons.
    pub button_text: Color,
}

impl Contrast {
    /// White on black, selection in cyan and links in yellow, as Windows' oldest and best-known
    /// contrast theme ("High Contrast Black"); disabled in the grey of the newer ones.
    pub const DARK: Contrast = Contrast {
        ground: Color::BLACK,
        text: Color::WHITE,
        link: Color::rgb(0xffff00),
        off: Color::rgb(0xa6a6a6),
        picked: Color::rgb(0x1aebff),
        on_picked: Color::BLACK,
        button: Color::BLACK,
        button_text: Color::WHITE,
    };
    /// Black on white, selection in deep purple ("High Contrast White"), disabled in grey.
    pub const LIGHT: Contrast = Contrast {
        ground: Color::WHITE,
        text: Color::BLACK,
        link: Color::rgb(0x00009f),
        off: Color::rgb(0x676767),
        picked: Color::rgb(0x37006e),
        on_picked: Color::WHITE,
        button: Color::WHITE,
        button_text: Color::BLACK,
    };
}

impl Theme {
    /// Light text on a dark ground.
    pub fn dark() -> Self {
        Theme {
            bg: Color::rgb(0x0f1115),
            panel: Color::rgb(0x151821),
            raise: Color::rgb(0x1b1f2a),
            hover: Color::rgb(0x1d2130),
            selected: Color::rgb(0x232840),
            line: Color::rgb(0x262b38),
            line_strong: Color::rgb(0x323848),
            text: Color::rgb(0xe6e8ee),
            text_dim: Color::rgb(0xa4acbd),
            text_faint: Color::rgb(0x6f778a),
            accent: Color::rgb(0x8b93ff),
            on_accent: Color::rgb(0x0f1115),
            red: Color::rgb(0xf2706b),
            amber: Color::rgb(0xe8b04b),
            green: Color::rgb(0x5fcf8f),
            blue: Color::rgb(0x5fb2f5),
            font: Font::new(13.0),
            small: Font::new(11.5),
            radius: 6.0,
            control: 28.0,
            gap: 8.0,
        }
    }

    /// Dark text on a light ground.
    pub fn light() -> Self {
        Theme {
            bg: Color::rgb(0xf5f6f8),
            panel: Color::rgb(0xffffff),
            raise: Color::rgb(0xffffff),
            hover: Color::rgb(0xe6e9f1),
            selected: Color::rgb(0xdfe3fa),
            line: Color::rgb(0xe0e3ea),
            line_strong: Color::rgb(0xcfd4de),
            text: Color::rgb(0x1b1f2c),
            text_dim: Color::rgb(0x535c70),
            text_faint: Color::rgb(0x8189a0),
            accent: Color::rgb(0x4b52d6),
            on_accent: Color::rgb(0xffffff),
            red: Color::rgb(0xcf423d),
            amber: Color::rgb(0xa86f08),
            green: Color::rgb(0x1a8f52),
            blue: Color::rgb(0x1b76c4),
            ..Theme::dark()
        }
    }

    /// High contrast in these colours: [`Contrast::DARK`], [`Contrast::LIGHT`], or the person's
    /// system colours. Text is never dimmed; selected or hovered parts are the background moved
    /// toward the selection colour, never so far that text on it drops below 7:1; that colour
    /// is the accent, with its text; lines are strong.
    pub fn high_contrast(colors: Contrast) -> Self {
        let Contrast { ground, text, picked, .. } = colors;
        // The four status colours keep their hue, lightened or darkened as this background
        // needs.
        let usual = if ground.is_dark() { Theme::dark() } else { Theme::light() };
        let said = |color: Color| readable(color, ground, 4.5);
        let selected = tinted(ground, picked, text, 0.4);
        Theme {
            bg: ground,
            panel: ground,
            raise: colors.button,
            hover: ground.toward(selected, 0.5),
            selected,
            // A strong line is also an off switch's track, with a knob in the text colour: as
            // far from both as can be.
            line: readable(text.toward(ground, 0.65), ground, 3.0),
            line_strong: between(text, ground),
            text,
            text_dim: text,
            text_faint: colors.off,
            accent: readable(picked, ground, 3.0),
            on_accent: colors.on_picked,
            red: said(usual.red),
            amber: said(usual.amber),
            green: said(usual.green),
            blue: said(usual.blue),
            ..usual
        }
    }

    /// The same around `color`, which becomes the accent (lightened or darkened as needed to
    /// read as text on the background), with white or near black on it, whichever reads better;
    /// selected and hovered parts lean toward it.
    pub fn accented(self, color: Color) -> Self {
        let accent = readable(color.solid(), self.bg, 4.5);
        let dark = if self.bg.is_dark() { self.bg } else { self.text };
        let on_accent = if contrast(Color::WHITE, accent) >= contrast(dark, accent) { Color::WHITE } else { dark };
        Theme { accent, on_accent, hover: self.panel.toward(accent, 0.08), selected: self.panel.toward(accent, 0.15), ..self }
    }

    /// The same, tighter: smaller buttons and boxes, smaller gaps, less rounded corners.
    pub fn dense(self) -> Self {
        Theme { control: self.control * 6.0 / 7.0, gap: self.gap * 0.5, radius: self.radius * 2.0 / 3.0, ..self }
    }

    /// The same, looser: larger buttons and boxes, larger gaps, rounder corners.
    pub fn roomy(self) -> Self {
        Theme { control: self.control * 8.0 / 7.0, gap: self.gap * 1.5, radius: self.radius * 4.0 / 3.0, ..self }
    }

    /// The same with text `by` times as large (1: unchanged), and buttons and boxes taller as
    /// the text needs. Gaps and corners stay.
    pub fn text_scaled(self, by: f32) -> Self {
        let by = if by.is_finite() { by.max(0.5) } else { 1.0 };
        let grown = |font: Font| Font { size: font.size * by, ..font };
        // A line of text is about 1.35 times its font size.
        let control = self.control + self.font.size * (by - 1.0) * 1.35;
        Theme { font: grown(self.font), small: grown(self.small), control, ..self }
    }

    /// The theme following all the person asked their system for: high contrast in their
    /// colours if asked; else dark or light, around their accent if any, with text as large as
    /// asked. (High contrast keeps its own colours: the accent is left out there.)
    pub fn following(wishes: &Wishes) -> Self {
        let base = match (wishes.contrast, wishes.dark) {
            (Some(colors), _) => Theme::high_contrast(colors),
            (None, true) => Theme::dark(),
            (None, false) => Theme::light(),
        };
        let base = match (wishes.contrast, wishes.accent) {
            (None, Some(color)) => base.accented(color),
            _ => base,
        };
        if wishes.text == 1.0 { base } else { base.text_scaled(wishes.text) }
    }
}

/// A colour's relative luminance, 0 for black to 1 for white: each channel linearized from
/// sRGB, then weighted as the eye weighs them.
fn light_of(color: Color) -> f32 {
    let linear = |part: u32| {
        let part = (part & 0xff) as f32 / 255.0;
        if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * linear(color.0 >> 16) + 0.7152 * linear(color.0 >> 8) + 0.0722 * linear(color.0)
}

/// The WCAG contrast ratio of two colours, 1 to 21. Text reads well from 4.5 and very well at
/// 7; a line or shape stands out at 3.
pub(crate) fn contrast(one: Color, other: Color) -> f32 {
    let (one, other) = (light_of(one), light_of(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// `color` lightened (on a dark background) or darkened (on a light one) just enough to reach
/// `least` against `ground`, in twentieths of the way to white or black.
fn readable(color: Color, ground: Color, least: f32) -> Color {
    let end = if ground.is_dark() { Color::WHITE } else { Color::BLACK };
    (0..=20).map(|step| color.toward(end, step as f32 / 20.0)).find(|color| contrast(*color, ground) >= least).unwrap_or(end)
}

/// The colour between `one` and `other` furthest from both to the eye, in twentieths of the
/// way.
fn between(one: Color, other: Color) -> Color {
    let apart = |color: &Color| contrast(*color, one).min(contrast(*color, other));
    (0..=20).map(|step| one.toward(other, step as f32 / 20.0)).max_by(|a, b| apart(a).total_cmp(&apart(b))).unwrap_or(one)
}

/// `ground` moved toward `color` by at most `most` of the way, as far as `text` still reads on
/// it at 7:1, in twentieths. (With a person's colours where text reads worse than that even on
/// the background, it may lose a quarter of that.)
fn tinted(ground: Color, color: Color, text: Color, most: f32) -> Color {
    let (steps, least) = ((most * 20.0) as u32, (contrast(text, ground) * 0.75).min(7.0));
    (0..=steps).rev().map(|step| ground.toward(color, step as f32 / 20.0)).find(|fill| contrast(text, *fill) >= least).unwrap_or(ground)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows' four contrast themes as Windows 11's theme files
    /// (`C:\Windows\Resources\Ease of Access Themes`) hold them: "Aquatic", "Desert", "Dusk",
    /// "Night sky".
    pub(crate) const WINDOWS: [(&str, Contrast); 4] = [
        ("Aquatic", windows([0x202020, 0xffffff, 0x75e9fc, 0xa6a6a6, 0x8ee3f0, 0x263b50, 0x202020, 0xffffff])),
        ("Desert", windows([0xfffaef, 0x3d3d3d, 0x1c5e75, 0x676767, 0x903909, 0xfff5e3, 0xfffaef, 0x202020])),
        ("Dusk", windows([0x2d3236, 0xffffff, 0x70ebde, 0xa6a6a6, 0xa1bfde, 0x212d3b, 0x2d3236, 0xb6f6f0])),
        ("Night sky", windows([0x000000, 0xffffff, 0x8080ff, 0xa6a6a6, 0xd6b4fd, 0x2b2b2b, 0x000000, 0xffee32])),
    ];

    const fn windows([ground, text, link, off, picked, on_picked, button, button_text]: [u32; 8]) -> Contrast {
        let [ground, text, link, off] = [Color::rgb(ground), Color::rgb(text), Color::rgb(link), Color::rgb(off)];
        let [picked, on_picked, button, button_text] = [Color::rgb(picked), Color::rgb(on_picked), Color::rgb(button), Color::rgb(button_text)];
        Contrast { ground, text, link, off, picked, on_picked, button, button_text }
    }

    /// Accent colours a person may pick: Windows' default first, then colours at every edge of
    /// the range.
    const PICKED: [u32; 9] = [0x0078d4, 0xff0000, 0xffff00, 0x00ff00, 0x0000ff, 0xffffff, 0x000000, 0x808080, 0xe3008c];

    /// Every pair of colours the library's parts draw one on the other, and the contrast each
    /// needs. `strong` for high contrast, where text keeps 7 everywhere.
    fn readings(t: &Theme, strong: bool) -> Vec<(&'static str, Color, Color, f32)> {
        let text = if strong { 7.0 } else { 4.5 };
        let mut pairs = Vec::new();
        for (name, fill) in [("ground", t.bg), ("panel", t.panel), ("raise", t.raise), ("hover", t.hover), ("selected", t.selected)] {
            pairs.push((name, t.text, fill, text));
            pairs.push((name, t.text_dim, fill, 4.5));
        }
        // Links and the accent on the background; text on the accent; text in a selected part
        // (the colour over the background at a third).
        pairs.extend([("accent", t.accent, t.bg, 4.5), ("on accent", t.on_accent, t.accent, 4.5)]);
        // An off switch's knob on its track.
        pairs.push(("switch", t.text, t.line_strong, 3.0));
        pairs.push(("selected text", t.text, Color(t.accent.alpha(0.35).over(t.bg.0, 255)), 4.5));
        pairs.extend([("red", t.red, t.bg, 3.0), ("amber", t.amber, t.bg, 3.0), ("green", t.green, t.bg, 3.0), ("blue", t.blue, t.bg, 3.0)]);
        if strong {
            pairs.extend([("line", t.line, t.bg, 3.0), ("strong line", t.line_strong, t.bg, 3.0), ("faint", t.text_faint, t.bg, 4.5)]);
            pairs.extend([("red", t.red, t.bg, 4.5), ("amber", t.amber, t.bg, 4.5), ("green", t.green, t.bg, 4.5), ("blue", t.blue, t.bg, 4.5)]);
        }
        pairs
    }

    fn unreadable(t: &Theme, strong: bool) -> Vec<String> {
        let far = |(name, ink, fill, least): (&str, Color, Color, f32)| (contrast(ink, fill) < least).then(|| format!("{name}: {ink} on {fill} is {:.2}", contrast(ink, fill)));
        readings(t, strong).into_iter().filter_map(far).collect()
    }

    #[test]
    fn light_is_weighed_as_the_web_weighs_it() {
        assert!((contrast(Color::BLACK, Color::WHITE) - 21.0).abs() < 1e-4 && contrast(Color::WHITE, Color::BLACK) == contrast(Color::BLACK, Color::WHITE));
        assert_eq!(contrast(Color::rgb(0x123456), Color::rgb(0x123456)), 1.0);
        // As page checkers count it: #767676 on white is just above 4.5.
        assert!((contrast(Color::rgb(0x767676), Color::WHITE) - 4.54).abs() < 0.01);
        assert!((contrast(Color::rgb(0x0078d4), Color::WHITE) - 4.53).abs() < 0.01);
    }

    #[test]
    fn every_theme_keeps_its_text_readable() {
        for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            assert_eq!(unreadable(&theme, false), Vec::<String>::new(), "{name}");
        }
        let mut contrasts = vec![("dark", Contrast::DARK), ("light", Contrast::LIGHT)];
        contrasts.extend(WINDOWS);
        for (name, colors) in contrasts {
            let theme = Theme::high_contrast(colors);
            assert_eq!(unreadable(&theme, true), Vec::<String>::new(), "high contrast: {name}");
            // The system's own colours are kept where they can be.
            assert_eq!([theme.bg, theme.text, theme.text_faint, theme.raise, theme.on_accent], [colors.ground, colors.text, colors.off, colors.button, colors.on_picked], "{name}");
            assert_eq!(theme.accent, colors.picked, "{name}");
            // Selected can be told from the background, and from merely hovered.
            assert!(contrast(theme.selected, theme.bg) > contrast(theme.hover, theme.bg) && contrast(theme.hover, theme.bg) > 1.1, "{name}");
        }
        for picked in PICKED.map(Color::rgb) {
            for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
                let accented = theme.accented(picked);
                assert_eq!(unreadable(&accented, false), Vec::<String>::new(), "{name} around {picked}");
            }
        }
    }

    #[test]
    fn a_theme_is_made_around_one_colour() {
        let (dark, blue) = (Theme::dark(), Color::rgb(0x0078d4));
        let around = dark.accented(blue);
        // Windows' blue is too dark on the dark background: lightened, hue kept, just enough.
        assert!((around.accent.to_hsv()[0] - blue.to_hsv()[0]).abs() < 0.005);
        assert_eq!(around.accent, blue.toward(Color::WHITE, 0.1));
        assert!(contrast(around.accent, dark.bg) >= 4.5 && contrast(blue.toward(Color::WHITE, 0.05), dark.bg) < 4.5);
        // On the light background (slightly greyer than white) it is darkened a little; on
        // white it reads as it is.
        assert_eq!(Theme::light().accented(blue).accent, blue.toward(Color::BLACK, 0.05));
        assert_eq!(Theme { bg: Color::WHITE, ..Theme::light() }.accented(blue).accent, blue);
        // Only what goes with the accent changes; backgrounds and text stay.
        let same = |t: &Theme| (t.bg, t.panel, t.raise, t.line, t.text, t.text_dim, t.red, t.font, t.control);
        assert_eq!(same(&around), same(&dark));
        assert!(around.hover != dark.hover && around.selected != dark.selected && around.selected != around.hover);
        // Around its own accent, a theme is unchanged, within a few levels each.
        for theme in [Theme::dark(), Theme::light()] {
            let again = theme.accented(theme.accent);
            assert_eq!((again.accent, again.on_accent), (theme.accent, theme.on_accent));
            let near = |one: Color, other: Color| (0..3).all(|at| ((one.0 >> (8 * at)) & 0xff).abs_diff((other.0 >> (8 * at)) & 0xff) <= 12);
            assert!(near(again.selected, theme.selected) && near(again.hover, theme.hover), "{again:?}");
        }
        // White or near black on it, whichever reads better: on a dark background, where it is
        // lightened to read, the background; on a light one, white.
        for color in PICKED.map(Color::rgb) {
            assert_eq!((dark.accented(color).on_accent, Theme::light().accented(color).on_accent), (dark.bg, Color::WHITE), "{color}");
        }
    }

    #[test]
    fn spacing_and_text_grow_and_shrink() {
        let t = Theme::dark();
        assert_eq!((t.dense().control, t.dense().gap, t.dense().radius), (24.0, 4.0, 4.0));
        assert_eq!((t.roomy().control, t.roomy().gap, t.roomy().radius), (32.0, 12.0, 8.0));
        assert_eq!((t.dense().font, t.roomy().accent), (t.font, t.accent));
        // Text twice as large: its boxes grow by as much as a line of it does.
        let large = t.text_scaled(2.0);
        assert_eq!((large.font.size, large.small.size, large.gap, large.radius), (26.0, 23.0, t.gap, t.radius));
        assert!((large.control - (28.0 + 13.0 * 1.35)).abs() < 1e-4);
        assert_eq!(t.text_scaled(1.0), t);
        // Both ways round, the same.
        assert_eq!(t.dense().text_scaled(1.5).font, t.text_scaled(1.5).dense().font);
        // A meaningless factor leaves text as it is; none makes it vanish.
        assert_eq!([f32::NAN, f32::INFINITY].map(|by| t.text_scaled(by)), [t, t]);
        assert_eq!(t.text_scaled(0.0).font.size, 6.5);
    }

    #[test]
    fn the_theme_follows_what_the_person_asked_for() {
        let wished = |change: fn(&mut Wishes)| {
            let mut wishes = Wishes::default();
            change(&mut wishes);
            Theme::following(&wishes)
        };
        assert_eq!(wished(|_| {}), Theme::light());
        assert_eq!(wished(|w| w.dark = true), Theme::dark());
        let red = Color::rgb(0xd13438);
        assert_eq!(wished(|w| (w.dark, w.accent) = (true, Some(Color::rgb(0xd13438)))), Theme::dark().accented(red));
        // High contrast comes before dark or light, and keeps its own colours.
        let night = WINDOWS[3].1;
        assert_eq!(wished(|w| (w.dark, w.contrast, w.accent) = (false, Some(WINDOWS[3].1), Some(Color::rgb(0xd13438)))), Theme::high_contrast(night));
        // Larger text, with any of them.
        assert_eq!(wished(|w| (w.text, w.contrast) = (1.5, Some(Contrast::LIGHT))), Theme::high_contrast(Contrast::LIGHT).text_scaled(1.5));
    }
}
