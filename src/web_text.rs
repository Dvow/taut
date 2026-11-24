//! The browser's own text, as the web's system text: measured and drawn by a 2D canvas
//! (`measureText`, `fillText`) in the loader, then laid into the interface's pixels here. So
//! web pages get what the browser has: the person's fonts, shaping of every script, right-to-left
//! order, colour emoji, and fallback for letters a font lacks.
//!
//! A line is drawn whole, in white on clear, and read back: where the browser drew grey it is
//! coverage, laid in the text's colour (as FreeType's glyphs are on Linux); where it drew
//! colours (emoji) it is a picture. Each line drawn is kept, by font and text, so a line drawn
//! again (a hovered button) costs no call; so are measures.
use crate::canvas::Canvas;
use crate::color::Color;
use crate::text::{Face, Font, Metrics, TextSystem};
use crate::web_host::{text_draw, text_measure, text_metrics, took};
use std::collections::HashMap;

/// How much the kept lines may take, in bytes, before all are let go.
const KEPT: usize = 8 << 20;
/// How many measures are kept before all are let go.
const MEASURES: usize = 4096;

/// A line drawn by the browser: its left edge and baseline within the picture, in pixels, and
/// the picture: coverage, or colours where it has any.
struct Drawn {
    left: i32,
    baseline: i32,
    width: usize,
    ink: Ink,
}

enum Ink {
    Cover(Vec<u8>),
    Colours(Vec<u32>),
}

/// The browser's text, through a canvas of the loader's.
#[derive(Default)]
pub struct SystemText {
    /// Lines drawn, by font and text, and their bytes.
    drawn: HashMap<(String, String), Drawn>,
    bytes: usize,
    /// Widths and character ends, by font and text, in pixels.
    measured: HashMap<(String, String), (f32, Vec<f32>)>,
    metrics: HashMap<String, [f32; 4]>,
}

impl std::fmt::Debug for SystemText {
    /// How many lines and measures are kept.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SystemText").field("drawn", &self.drawn.len()).field("measured", &self.measured.len()).finish_non_exhaustive()
    }
}

impl SystemText {
    /// The browser's text. (Always there; `Option` as on the other systems.)
    pub fn new() -> Option<SystemText> {
        Some(SystemText::default())
    }

    /// The font as the loader takes it: CSS style, weight and size in pixels, then on a line of
    /// its own the family: `sans` or `mono` for the loader's (the page can set them), else a
    /// family's name.
    fn css(font: Font, scale: f32) -> String {
        let px = font.size * scale;
        let (family, style) = match font.face {
            Face::Regular => (None, (400, false)),
            Face::Bold => (None, (700, false)),
            Face::Italic => (None, (400, true)),
            Face::Mono => (Some("mono".to_string()), (400, false)),
            Face::Named(_) => {
                let (family, of, style) = crate::system_fonts::asked(font.face);
                (family.or((of == Face::Mono).then(|| "mono".into())), (style.weight, style.italic))
            }
        };
        format!("{} {} {px}px\n{}", if style.1 { "italic" } else { "normal" }, style.0, family.unwrap_or_else(|| "sans".into()))
    }

    /// A font's ascent, descent, cap height and x-height, in pixels.
    fn sizes(&mut self, css: &str) -> [f32; 4] {
        if let Some(sizes) = self.metrics.get(css) {
            return *sizes;
        }
        let mut sizes = [0.0f32; 4];
        // SAFETY: the loader writes four numbers.
        unsafe { text_metrics(css.as_ptr(), css.len(), sizes.as_mut_ptr()) };
        self.metrics.insert(css.into(), sizes);
        sizes
    }

    /// A line's width and its characters' ends, in pixels.
    fn measured(&mut self, css: &str, text: &str) -> &(f32, Vec<f32>) {
        if self.measured.len() >= MEASURES {
            self.measured.clear();
        }
        let key = (css.to_string(), text.to_string());
        self.measured.entry(key).or_insert_with(|| {
            let mut stops = vec![0.0f32; text.chars().count()];
            // SAFETY: the loader writes one number per character.
            let width = unsafe { text_measure(css.as_ptr(), css.len(), text.as_ptr(), text.len(), stops.as_mut_ptr(), stops.len()) };
            (width, stops)
        })
    }

    /// A line, drawn by the browser once and kept.
    fn drawn(&mut self, css: &str, text: &str) -> Option<&Drawn> {
        let key = (css.to_string(), text.to_string());
        if !self.drawn.contains_key(&key) {
            let drawn = draw_line(css, text)?;
            if self.bytes > KEPT {
                (self.drawn, self.bytes) = (HashMap::new(), 0);
            }
            self.bytes += drawn.width * 4 + text.len();
            self.drawn.insert(key.clone(), drawn);
        }
        self.drawn.get(&key)
    }
}

/// Has the loader draw a line and reads its picture back.
fn draw_line(css: &str, text: &str) -> Option<Drawn> {
    let mut place = [0.0f32; 4];
    // SAFETY: the loader writes four numbers, then hands over as many bytes as it says.
    let bytes = unsafe { text_draw(css.as_ptr(), css.len(), text.as_ptr(), text.len(), place.as_mut_ptr()) };
    let width = place[2] as usize;
    if bytes == 0 || width == 0 {
        return None;
    }
    let rgba = took(bytes);
    let pixels = rgba.as_chunks::<4>().0;
    let coloured = pixels.iter().any(|pixel| pixel[3] > 0 && (pixel[0] != 255 || pixel[1] != 255 || pixel[2] != 255));
    let ink = match coloured {
        false => Ink::Cover(pixels.iter().map(|pixel| pixel[3]).collect()),
        true => Ink::Colours(pixels.iter().map(premultiplied).collect()),
    };
    Some(Drawn { left: place[0] as i32, baseline: place[1] as i32, width, ink })
}

/// A canvas pixel (red, green, blue, alpha, straight) as the interface's.
fn premultiplied(pixel: &[u8; 4]) -> u32 {
    let alpha = u32::from(pixel[3]);
    let times = |channel: u8| (u32::from(channel) * alpha + 127) / 255;
    alpha << 24 | times(pixel[0]) << 16 | times(pixel[1]) << 8 | times(pixel[2])
}

impl TextSystem for SystemText {
    fn metrics(&mut self, font: Font, scale: f32) -> Metrics {
        let [ascent, descent, cap, small] = self.sizes(&SystemText::css(font, scale));
        Metrics { ascent: ascent.ceil() / scale, descent: descent.ceil() / scale, cap_height: cap / scale, x_height: small / scale }
    }

    fn measure(&mut self, text: &str, font: Font, scale: f32, stops: Option<&mut Vec<f32>>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let (width, ends) = self.measured(&SystemText::css(font, scale), text);
        if let Some(stops) = stops {
            stops.extend(ends.iter().map(|end| end / scale));
        }
        width / scale
    }

    fn draw(&mut self, canvas: &mut Canvas, text: &str, x: f32, y: f32, font: Font, color: Color) {
        let scale = canvas.scale();
        let css = SystemText::css(font, scale);
        let ascent = self.sizes(&css)[0].ceil() as i32;
        let Some(drawn) = self.drawn(&css, text) else { return };
        let at = [(x * scale).round() as i32 - drawn.left, (y * scale).round() as i32 + ascent - drawn.baseline];
        match &drawn.ink {
            Ink::Cover(cover) => canvas.cover(at[0], at[1], drawn.width, cover, color),
            Ink::Colours(pixels) => canvas.pixels(pixels, drawn.width, at, false),
        }
    }
}
