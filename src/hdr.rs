//! High dynamic range: colours and pictures brighter than SDR white, shown so on HDR screens
//! (the feature `hdr`).
//!
//! - The CPU draws every window as ever: SDR, the same pixels on every screen.
//! - What is brighter is a gain, per pixel: how many times brighter than its SDR colour. A
//!   box's [`El::brightness`](crate::El::brightness) gives one; an HDR picture a gain map (its
//!   SDR picture, and per pixel how much brighter, as Ultra HDR photos keep it).
//! - A window with gains on an HDR screen is presented in scRGB (linear, 1.0 is 80 nits): each
//!   pixel is its SDR colour made linear, times the screen's SDR white, times its gain, at most
//!   the screen's peak. The rest stays at SDR white, as in other programs' windows.
//! - A window's gains are a plane of 16-bit floats (`Gains`), redrawn where what is lit moved.
//!
//! | Path | HDR |
//! | --- | --- |
//! | Direct3D 11 and 12 | scRGB, `R16G16B16A16_FLOAT` (`hdr_windows`) |
//! | Vulkan, OpenGL, the CPU, X11, Wayland, macOS | SDR |
use crate::ui::{Picture, Ui};

#[cfg(feature = "hdr")]
/// SDR white in PQ and HLG pictures, in nits (ITU-R BT.2408).
const REFERENCE_WHITE: f32 = 203.0;

#[cfg(feature = "hdr")]
/// What a screen shows: whether HDR is on, its SDR white and peak brightness in nits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Screen {
    pub hdr: bool,
    pub white: f32,
    pub peak: f32,
}

#[cfg(feature = "hdr")]
impl Screen {
    /// An SDR screen: white at 80 nits, as scRGB counts.
    pub const SDR: Screen = Screen { hdr: false, white: 80.0, peak: 80.0 };

    /// How many times SDR white it shows at most: 1 on an SDR screen.
    pub fn headroom(self) -> f32 {
        if self.hdr { (self.peak / self.white).max(1.0) } else { 1.0 }
    }
}

#[cfg(feature = "hdr")]
impl Default for Screen {
    fn default() -> Screen {
        Screen::SDR
    }
}

/// A picture's gain map: per pixel, how many times brighter than its SDR picture, in rows of
/// `width`; and a number told apart from every other map's.
#[cfg(feature = "hdr")]
#[derive(Debug)]
pub(crate) struct GainMap {
    pub width: usize,
    pub gains: Vec<f32>,
    pub serial: u32,
}

/// What a window keeps of HDR: this run's bright boxes (each box's index and brightness), the
/// pictures' gain maps by picture number, and its screen.
#[cfg(feature = "hdr")]
#[derive(Debug, Default)]
pub(crate) struct Light {
    pub bright: Vec<(u32, f32)>,
    pub maps: std::collections::HashMap<u32, GainMap>,
    pub screen: Screen,
    serial: u32,
}

#[cfg(feature = "hdr")]
/// Something lighting part of a window, in pixels: where it may change the gains (left, top,
/// right, bottom), its shape (left, top, width, height) and corner radius; how much brighter;
/// a picture's gain map lying in the shape (its picture number and serial); or a box lying
/// over lit ones, which puts them out (`covers`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glow {
    pub area: [i32; 4],
    pub shape: [f32; 4],
    pub radius: f32,
    pub gain: f32,
    pub map: Option<(u32, u32)>,
    pub covers: bool,
}

#[cfg(feature = "hdr")]
/// A window's gains: per pixel, as 16-bit floats in rows of its width; what lit them; and the
/// part that changed since the GPU last took them (left, top, right, bottom).
#[derive(Debug, Default)]
pub(crate) struct Gains {
    pub size: [i32; 2],
    pub plane: Vec<u16>,
    lit: Vec<Glow>,
    pub changed: Option<[i32; 4]>,
}

#[cfg(feature = "hdr")]
/// 1.0 as a 16-bit float.
const ONE: u16 = 0x3C00;

#[cfg(feature = "hdr")]
impl Gains {
    /// Whether anything is lit: the window then needs HDR.
    pub fn any(&self) -> bool {
        !self.lit.is_empty()
    }

    /// Takes in what lights a window `size` pixels now: the plane is drawn again where that
    /// differs (all of it after a resize), and freed once nothing is lit. Gives the part that
    /// changed (left, top, right, bottom), if any.
    #[cfg(feature = "hdr")]
    pub fn take(&mut self, glows: Vec<Glow>, size: [i32; 2], maps: &std::collections::HashMap<u32, GainMap>) -> Option<[i32; 4]> {
        if glows == self.lit && (size == self.size || glows.is_empty()) {
            return None;
        }
        if glows.is_empty() {
            let was = std::mem::take(self).size;
            return Some([0, 0, was[0], was[1]]);
        }
        let whole = [0, 0, size[0], size[1]];
        let part = match size == self.size && !self.plane.is_empty() {
            true => self.lit.iter().chain(&glows).map(|glow| glow.area).reduce(union).unwrap_or(whole),
            false => whole,
        };
        if size != self.size || self.plane.is_empty() {
            (self.size, self.plane) = (size, vec![ONE; size[0].max(0) as usize * size[1].max(0) as usize]);
        }
        let part = cut(part, whole);
        self.reset(part);
        for glow in &glows {
            self.draw(glow, cut(glow.area, part), maps);
        }
        self.changed = Some(self.changed.map_or(part, |was| cut(union(was, part), whole)));
        self.lit = glows;
        Some(part)
    }

    /// Sets `part` (left, top, right, bottom, inside the plane) to 1.
    fn reset(&mut self, [left, top, right, bottom]: [i32; 4]) {
        let wide = self.size[0] as usize;
        for y in top.max(0)..bottom {
            self.plane[y as usize * wide + left as usize..y as usize * wide + right as usize].fill(ONE);
        }
    }

    /// Draws one glow into `part` of the plane: brightening what it covers, or putting it out.
    #[cfg(feature = "hdr")]
    fn draw(&mut self, glow: &Glow, [left, top, right, bottom]: [i32; 4], maps: &std::collections::HashMap<u32, GainMap>) {
        let wide = self.size[0] as usize;
        let map = glow.map.and_then(|(picture, serial)| maps.get(&picture).filter(|map| map.serial == serial && map.width > 0));
        for y in top..bottom {
            let row = &mut self.plane[y as usize * wide..][..wide];
            for x in left..right {
                let cover = covered(glow.shape, glow.radius, [x as f32 + 0.5, y as f32 + 0.5]);
                row[x as usize] = half(lit(from_half(row[x as usize]), glow, map.map_or(1.0, |map| sampled(map, glow.shape, x, y)), cover));
            }
        }
    }
}

#[cfg(feature = "hdr")]
/// A gain once a glow covering so much of the pixel lit it, `map` being its gain map's there.
#[inline]
fn lit(gain: f32, glow: &Glow, map: f32, cover: f32) -> f32 {
    match glow.covers {
        true => gain + (1.0 - gain) * cover,
        false => gain * (1.0 + (glow.gain * map - 1.0) * cover),
    }
}

/// A gain map's gain at window pixel `x`, `y`, the map lying over `shape`: the nearest.
#[cfg(feature = "hdr")]
fn sampled(map: &GainMap, [left, top, wide, high]: [f32; 4], x: i32, y: i32) -> f32 {
    let rows = map.gains.len() / map.width;
    let across = (((x as f32 + 0.5 - left) / wide * map.width as f32) as usize).min(map.width - 1);
    let down = (((y as f32 + 0.5 - top) / high * rows as f32) as usize).min(rows.saturating_sub(1));
    map.gains.get(down * map.width + across).copied().unwrap_or(1.0)
}

#[cfg(feature = "hdr")]
/// How much of a pixel centred at `at` a rounded rectangle covers (left, top, width, height,
/// corner radius): 0 to 1, by its distance from the edge.
fn covered([left, top, wide, high]: [f32; 4], radius: f32, at: [f32; 2]) -> f32 {
    let radius = radius.min(wide / 2.0).min(high / 2.0).max(0.0);
    let [x, y] = [(at[0] - left - wide / 2.0).abs() - (wide / 2.0 - radius), (at[1] - top - high / 2.0).abs() - (high / 2.0 - radius)];
    let outside = x.max(0.0).hypot(y.max(0.0)) + x.max(y).min(0.0) - radius;
    (0.5 - outside).clamp(0.0, 1.0)
}

#[cfg(feature = "hdr")]
/// The smallest rectangle holding both (left, top, right, bottom).
fn union(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

#[cfg(feature = "hdr")]
/// `a` cut to `b` (left, top, right, bottom); empty ones have right at left, bottom at top.
fn cut(a: [i32; 4], b: [i32; 4]) -> [i32; 4] {
    let [left, top] = [a[0].max(b[0]), a[1].max(b[1])];
    [left, top, a[2].min(b[2]).max(left), a[3].min(b[3]).max(top)]
}

#[cfg(feature = "hdr")]
/// A 16-bit float (IEEE 754 half), the nearest to `value`; infinity past the largest, 0 for
/// what is too small (gains are never that small).
pub(crate) fn half(value: f32) -> u16 {
    let bits = value.to_bits();
    let (sign, power, fraction) = ((bits >> 16 & 0x8000) as u16, (bits >> 23 & 0xff) as i32 - 112, bits & 0x7f_ffff);
    if power <= 0 {
        return sign;
    }
    if power >= 31 {
        return sign | 0x7C00;
    }
    // Rounded to nearest, ties to even; a carry into the power is right.
    let (kept, rest) = ((power as u32) << 10 | fraction >> 13, fraction & 0x1fff);
    sign | (kept + (rest > 0x1000 || (rest == 0x1000 && kept & 1 == 1)) as u32).min(0x7C00) as u16
}

#[cfg(feature = "hdr")]
/// A 16-bit float's value (normal ones and zero; others are never written here).
pub(crate) fn from_half(half: u16) -> f32 {
    let (sign, power, fraction) = ((half as u32 & 0x8000) << 16, (half >> 10 & 0x1f) as u32, half as u32 & 0x3ff);
    match power {
        0 => f32::from_bits(sign),
        31 => f32::from_bits(sign | 0x7f80_0000),
        _ => f32::from_bits(sign | (power + 112) << 23 | fraction << 13),
    }
}

/// An HDR picture as an SDR picture and a gain map: each pixel's linear colour (red, green,
/// blue, alpha; 1.0 is SDR white) scaled down, keeping its hue, until no channel passes white,
/// and how much it was scaled. Pixels `0xAARRGGBB`, as `Ui::add_picture` takes them.
pub(crate) fn split(pixels: &[[f32; 4]]) -> (Vec<u32>, Vec<f32>) {
    let (mut sdr, mut gains) = (Vec::with_capacity(pixels.len()), Vec::with_capacity(pixels.len()));
    for [red, green, blue, alpha] in pixels.iter().copied() {
        let [red, green, blue] = [red, green, blue].map(|part| if part.is_nan() { 0.0 } else { part.max(0.0) });
        let gain = red.max(green).max(blue).clamp(1.0, 65_504.0);
        let [r, g, b] = [red, green, blue].map(|part| encoded(part / gain));
        sdr.push(((alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u32) << 24 | r << 16 | g << 8 | b);
        gains.push(gain);
    }
    (sdr, gains)
}

/// A linear value (0 to 1) in sRGB's curve, 0 to 255.
fn encoded(part: f32) -> u32 {
    let part = part.clamp(0.0, 1.0);
    let bent = if part <= 0.003_130_8 { 12.92 * part } else { 1.055 * part.powf(1.0 / 2.4) - 0.055 };
    (bent * 255.0 + 0.5) as u32
}

/// The light of a PQ signal (SMPTE ST 2084), 0 to 1: in nits, up to 10,000.
#[cfg(feature = "hdr")]
fn pq(signal: f32) -> f32 {
    let [m1, m2, c1, c2, c3] = [0.159_301_76, 78.843_75, 0.835_937_5, 18.851_563, 18.6875];
    let raised = signal.clamp(0.0, 1.0).powf(1.0 / m2);
    ((raised - c1).max(0.0) / (c2 - c3 * raised)).powf(1.0 / m1) * 10_000.0
}

/// The scene light of an HLG signal (ITU-R BT.2100), 0 to 1: 0 to 1.
#[cfg(feature = "hdr")]
fn hlg(signal: f32) -> f32 {
    let [a, b, c] = [0.178_832_77, 0.284_668_92, 0.559_910_7];
    let signal = signal.clamp(0.0, 1.0);
    if signal <= 0.5 { signal * signal / 3.0 } else { (((signal - c) / a).exp() + b) / 12.0 }
}

/// Linear BT.709 (sRGB) light from linear light in a picture's primaries (`cICP`): BT.709 (1),
/// BT.2020 (9) or Display P3 (12). Nothing for others.
#[cfg(feature = "hdr")]
fn to_709(primaries: u8) -> Option<[[f32; 3]; 3]> {
    match primaries {
        1 => Some([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
        9 => Some([[1.660_491, -0.587_641_1, -0.072_849_86], [-0.124_550_47, 1.132_899_9, -0.008_349_42], [-0.018_150_76, -0.100_578_9, 1.118_729_7]]),
        12 => Some([[1.224_940_2, -0.224_940_4, 0.0], [-0.042_056_955, 1.042_057_1, 0.0], [-0.019_637_555, -0.078_636_05, 1.098_273_6]]),
        _ => None,
    }
}

/// Reads a PNG marked HDR by its `cICP` chunk (PQ or HLG transfer; BT.709, BT.2020 or P3
/// primaries; RGB, not interlaced) as an SDR picture and its gain map: the picture's width,
/// its pixels as `Ui::add_picture` takes them, and its gains. Nothing for any other picture.
/// (HLG is shown as on a 1,000-nit screen; SDR white is 203 nits, as BT.2408 has it.)
#[cfg(feature = "hdr")]
pub(crate) fn read_png(file: &[u8]) -> Option<(usize, Vec<u32>, Vec<f32>)> {
    let samples = crate::png::samples(file)?;
    let said = samples.chunk(b"cICP");
    let (&[primaries, transfer, 0, _], [width, height]) = (said.first_chunk::<4>()?, samples.size) else { return None };
    let (matrix, values) = (to_709(primaries)?, [1, 0, 3, 0, 2, 0, 4][samples.kind as usize]);
    if !matches!(transfer, 16 | 18) || !matches!(samples.depth, 8 | 16) || samples.kind == 3 {
        return None;
    }
    let row = width * values * samples.depth / 8;
    let mut linear = Vec::with_capacity(width * height);
    for y in 0..height {
        let line = &samples.rows[y * row..][..row];
        linear.extend((0..width).map(|x| light(read(line, x, values, samples.depth), transfer, &matrix)));
    }
    let (sdr, gains) = split(&linear);
    let turn = crate::exif_orientation::turn(samples.chunk(b"eXIf"));
    let (_, gains) = crate::exif_orientation::turned(turn, width, gains.into_iter().map(f32::to_bits).collect());
    let (width, sdr) = crate::exif_orientation::turned(turn, width, sdr);
    Some((width, sdr, gains.into_iter().map(f32::from_bits).collect()))
}

/// Pixel `x` of a row of samples, `values` to a pixel (grey, grey and alpha, RGB, RGBA), each
/// `depth` bits: red, green, blue and alpha, 0 to 1.
#[cfg(feature = "hdr")]
fn read(line: &[u8], x: usize, values: usize, depth: usize) -> [f32; 4] {
    let sample = |n: usize| match depth {
        16 => u16::from_be_bytes([line[(x * values + n) * 2], line[(x * values + n) * 2 + 1]]) as f32 / 65_535.0,
        _ => line[x * values + n] as f32 / 255.0,
    };
    match values {
        1 => [sample(0), sample(0), sample(0), 1.0],
        2 => [sample(0), sample(0), sample(0), sample(1)],
        3 => [sample(0), sample(1), sample(2), 1.0],
        _ => [sample(0), sample(1), sample(2), sample(3)],
    }
}

/// A pixel's linear BT.709 light, 1.0 being SDR white, from its signal in `transfer` (16 PQ,
/// 18 HLG) and its primaries' `matrix`.
#[cfg(feature = "hdr")]
fn light([red, green, blue, alpha]: [f32; 4], transfer: u8, matrix: &[[f32; 3]; 3]) -> [f32; 4] {
    let nits = match transfer {
        16 => [red, green, blue].map(pq),
        _ => {
            // HLG: the scene's light, then the 1,000-nit screen's (system gamma 1.2).
            let scene = [red, green, blue].map(hlg);
            let seen = 0.2627 * scene[0] + 0.678 * scene[1] + 0.0593 * scene[2];
            scene.map(|part| 1000.0 * seen.powf(0.2) * part)
        }
    };
    let [r, g, b] = matrix.map(|row| (row[0] * nits[0] + row[1] * nits[1] + row[2] * nits[2]) / REFERENCE_WHITE);
    [r, g, b, alpha]
}

/// HDR: brightness beyond SDR white, for boxes and pictures, where the screen shows it.
impl Ui {
    /// How many times brighter than SDR white this window's screen shows: 1.0 on an SDR screen,
    /// in a window the CPU shows, in a [`Surface`](crate::Surface), and without the feature
    /// `hdr`. Above it, [`El::brightness`](crate::El::brightness) and HDR pictures are capped.
    pub fn headroom(&self) -> f32 {
        #[cfg(feature = "hdr")]
        return self.light.screen.headroom();
        #[cfg(not(feature = "hdr"))]
        1.0
    }

    /// Takes in a picture brighter than SDR white, to show with
    /// [`El::picture`](crate::El::picture): `width` pixels per row, rows from the top, each
    /// linear red, green, blue (1.0 is SDR white, the sRGB primaries) and alpha. Where the screen
    /// is not HDR, or without the feature `hdr`, a pixel brighter than white is shown at white,
    /// keeping its hue.
    #[doc(alias("scRGB", "EDR", "float"))]
    pub fn add_hdr_picture(&mut self, width: usize, pixels: &[[f32; 4]]) -> Picture {
        let (sdr, gains) = split(pixels);
        let picture = self.add_picture(width, sdr);
        #[cfg(feature = "hdr")]
        self.light.keep_map(picture, width, gains);
        #[cfg(not(feature = "hdr"))]
        drop(gains);
        picture
    }

    /// What lights the window now, in pixels: each box made brighter and each picture with a
    /// gain map, in drawing order; and each box not inside a lit one, with an opaque fill, lying
    /// over a lit one before it.
    #[cfg(feature = "hdr")]
    pub(crate) fn glows(&self) -> Vec<Glow> {
        let (mut glows, mut ends) = (Vec::new(), Vec::new());
        if self.light.bright.is_empty() && self.light.maps.is_empty() {
            return glows;
        }
        for seen in self.visible_boxes.boxes.iter().filter(|seen| !seen.leaving) {
            ends.retain(|end| seen.at < *end);
            if let Some(glow) = self.glow_of(seen.at, seen.rect, seen.area) {
                ends.push(self.tree.specs[seen.at as usize].end);
                glows.push(glow);
                continue;
            }
            let spec = &self.tree.specs[seen.at as usize];
            let area = pixels(seen.area, self.scale);
            if ends.is_empty() && spec.fill.a() == 255 && glows.iter().any(|glow: &Glow| !glow.covers && cut(glow.area, area)[..2] != cut(glow.area, area)[2..]) {
                glows.push(Glow { area, shape: shape(seen.rect, self.scale), radius: spec.radius * self.scale, gain: 1.0, map: None, covers: true });
            }
        }
        glows
    }

    /// How box `at`, at `place` and seen in `area` (units), lights the window: by its brightness
    /// and its picture's gain map. Nothing where it does not.
    #[cfg(feature = "hdr")]
    fn glow_of(&self, at: u32, place: crate::geom::Rect, area: crate::geom::Rect) -> Option<Glow> {
        let (spec, light) = (&self.tree.specs[at as usize], &self.light);
        let gain = light.bright.binary_search_by_key(&at, |(box_at, _)| *box_at).map_or(1.0, |found| light.bright[found].1);
        let picture = match spec.content {
            crate::layout::Content::Picture { picture, keep } => light.maps.get(&picture).map(|map| (picture, map.serial, keep)),
            _ => None,
        };
        let Some((picture, serial, keep)) = picture else {
            let lit = Glow { area: pixels(area, self.scale), shape: shape(place, self.scale), radius: spec.radius * self.scale, gain, map: None, covers: false };
            return (gain != 1.0).then_some(lit);
        };
        let size = self.pictures.get(picture).map_or([0.0; 2], |stored| [stored.width as f32, (stored.pixels.len() / stored.width.max(1)) as f32]);
        let to = crate::paint::fit_aspect(place.inset(spec.frame.pad), size, keep);
        let area = pixels(area.intersect(to), self.scale);
        Some(Glow { area, shape: shape(to, self.scale), radius: spec.radius * self.scale, gain, map: Some((picture, serial)), covers: false })
    }

    /// Takes in the screen the window is on. Says whether its headroom changed: the app is to
    /// run again, to hear it.
    #[cfg(feature = "hdr")]
    pub(crate) fn set_screen(&mut self, screen: Screen) -> bool {
        let was = std::mem::replace(&mut self.light.screen, screen).headroom();
        was != screen.headroom()
    }
}

#[cfg(feature = "hdr")]
impl Light {
    /// Keeps a picture's gain map, in place of any it had.
    pub fn keep_map(&mut self, picture: Picture, width: usize, gains: Vec<f32>) {
        self.serial = self.serial.wrapping_add(1);
        self.maps.insert(picture.0, GainMap { width, gains, serial: self.serial });
    }
}

#[cfg(feature = "hdr")]
/// Where a box lies in window pixels: left, top, width, height.
fn shape(place: crate::geom::Rect, scale: f32) -> [f32; 4] {
    [place.x * scale, place.y * scale, place.w * scale, place.h * scale]
}

#[cfg(feature = "hdr")]
/// The window pixels a part of it (units) touches: left, top, right, bottom.
fn pixels(area: crate::geom::Rect, scale: f32) -> [i32; 4] {
    let [left, top] = [(area.x * scale).floor().max(0.0) as i32, (area.y * scale).floor().max(0.0) as i32];
    [left, top, ((area.right() * scale).ceil() as i32).max(left), ((area.bottom() * scale).ceil() as i32).max(top)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "hdr")]
    fn halves_are_the_nearest_16_bit_floats() {
        for (value, bits) in [(0.0, 0), (1.0, 0x3C00), (2.0, 0x4000), (0.5, 0x3800), (65_504.0, 0x7BFF), (1e6, 0x7C00), (1.0 + 1.0 / 2048.0, 0x3C00), (1.0 + 3.0 / 2048.0, 0x3C02), (2.5, 0x4100)] {
            assert_eq!(half(value), bits, "{value}");
        }
        for bits in (0x0400..0x7C00).step_by(7) {
            assert_eq!(half(from_half(bits)), bits);
        }
        assert_eq!(from_half(0x3C00), 1.0);
        assert_eq!(from_half(0x4500), 5.0);
    }

    #[test]
    #[cfg(feature = "hdr")]
    fn a_rounded_box_covers_its_inside_and_not_its_corners() {
        let shape = [10.0, 10.0, 40.0, 20.0];
        assert_eq!(covered(shape, 8.0, [30.0, 20.0]), 1.0);
        assert_eq!(covered(shape, 8.0, [10.5, 10.5]), 0.0);
        assert_eq!(covered(shape, 0.0, [10.5, 10.5]), 1.0);
        assert_eq!(covered(shape, 8.0, [9.5, 20.0]), 0.0);
        assert!((covered(shape, 8.0, [10.0, 20.0]) - 0.5).abs() < 1e-6, "half a pixel on the edge");
    }

    #[test]
    fn a_picture_brighter_than_white_is_kept_as_its_sdr_picture_and_how_much_brighter() {
        let (sdr, gains) = split(&[[1.0, 1.0, 1.0, 1.0], [4.0, 2.0, 0.0, 1.0], [0.5, 0.25, 0.0, 0.5], [-1.0, f32::NAN, 0.0, 1.0]]);
        assert_eq!(sdr, [0xffff_ffff, 0xffff_bc00, 0x80bc_8900, 0xff00_0000]);
        assert_eq!(gains, [1.0, 4.0, 1.0, 1.0]);
    }

    #[test]
    #[cfg(feature = "hdr")]
    fn an_hdr_screen_shows_up_to_its_peak_over_its_white() {
        assert_eq!(Screen::SDR.headroom(), 1.0);
        assert_eq!(Screen { hdr: true, white: 200.0, peak: 1000.0 }.headroom(), 5.0);
        assert_eq!(Screen { hdr: false, white: 200.0, peak: 1000.0 }.headroom(), 1.0);
        assert_eq!(Screen { hdr: true, white: 480.0, peak: 400.0 }.headroom(), 1.0);
    }
}

#[cfg(all(test, feature = "hdr"))]
mod lit {
    use super::*;
    use crate::testing::Trial;
    use crate::{Color, Flow};

    #[test]
    fn pq_and_hlg_signals_give_the_light_their_standards_say() {
        // PQ: 0 is black, 1 is 10,000 nits, 0.58 about 203 nits (SDR white), 0.5 about 92.
        assert_eq!(pq(0.0), 0.0);
        assert!((pq(1.0) - 10_000.0).abs() < 1.0);
        assert!((pq(0.580_69) - 203.0).abs() < 0.5, "{}", pq(0.580_69));
        assert!((pq(0.5) - 92.25).abs() < 0.1, "{}", pq(0.5));
        // HLG: half the signal is a twelfth of the light; all of it, all.
        assert!((hlg(0.5) - 1.0 / 12.0).abs() < 1e-6);
        assert!((hlg(1.0) - 1.0).abs() < 1e-5);
        // White at 75% HLG is about 203 nits on a 1,000-nit screen.
        let white = light([0.75; 4], 18, &to_709(9).unwrap());
        assert!(white[..3].iter().all(|part| (part - 1.0).abs() < 0.02), "{white:?}");
    }

    /// A PNG of `pixels` (RGB, 16 bits) with a `cICP` chunk.
    fn png(width: usize, pixels: &[[u16; 3]], cicp: [u8; 4]) -> Vec<u8> {
        let height = pixels.len() / width;
        let mut head = (width as u32).to_be_bytes().to_vec();
        head.extend((height as u32).to_be_bytes());
        head.extend([16, 2, 0, 0, 0]);
        let mut rows = Vec::new();
        for row in pixels.chunks(width) {
            rows.push(0);
            rows.extend(row.iter().flat_map(|pixel| pixel.iter().flat_map(|part| part.to_be_bytes())));
        }
        let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
        for (name, body) in [(b"IHDR", head), (b"cICP", cicp.to_vec()), (b"IDAT", crate::png::zlib_compress(&rows)), (b"IEND", Vec::new())] {
            file.extend(crate::png::chunk(name, &body));
        }
        file
    }

    #[test]
    fn a_pq_png_is_read_as_its_sdr_picture_and_gains() {
        // SDR white (203 nits), 4 times it, black, and a BT.2020 red at SDR white.
        let code = |nits: f32| {
            let (m1, m2, c1, c2, c3) = (0.159_301_76f64, 78.843_75f64, 0.835_937_5f64, 18.851_563f64, 18.6875f64);
            let y = (nits as f64 / 10_000.0).powf(m1);
            (((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2) * 65_535.0).round() as u16
        };
        let [white, four, red] = [code(203.0), code(812.0), code(203.0)];
        let file = png(2, &[[white; 3], [four; 3], [0; 3], [red, 0, 0]], [9, 16, 0, 1]);
        let (width, sdr, gains) = read_png(&file).expect("an HDR PNG");
        assert_eq!(width, 2);
        assert_eq!(sdr[..3], [0xffff_ffff, 0xffff_ffff, 0xff00_0000]);
        assert!((gains[0] - 1.0).abs() < 0.01 && (gains[1] - 4.0).abs() < 0.02 && gains[2] == 1.0, "{gains:?}");
        // BT.2020's red lies outside sRGB: more than white in red, the rest cut at 0.
        assert!(gains[3] > 1.6 && sdr[3] == 0xffff_0000, "{:#x} {}", sdr[3], gains[3]);
        // Not HDR: left to the usual reader.
        assert!(read_png(&png(1, &[[1, 2, 3]], [1, 13, 0, 1])).is_none());
    }

    /// A window with a white box, the same twice as bright, an HDR picture, and a box over the
    /// bright one (where `covered`).
    fn bright(covered: &mut bool, ui: &mut Ui) {
        ui.el().flow(Flow::Stack).size(200.0, 100.0).open();
        ui.el().size(40.0, 20.0).fill(Color::WHITE).show();
        ui.el().offset(50.0, 0.0).size(40.0, 20.0).fill(Color::WHITE).brightness(2.0).show();
        if *covered {
            ui.el().offset(60.0, 5.0).size(10.0, 10.0).fill(Color::BLACK).show();
        }
        ui.close();
    }

    #[test]
    fn bright_boxes_and_pictures_light_the_gains_and_boxes_over_them_put_them_out() {
        let mut trial = Trial::new(false, bright);
        let mut gains = Gains::default();
        let gain = |gains: &Gains, x: usize, y: usize| from_half(gains.plane[y * gains.size[0] as usize + x]);
        gains.take(trial.ui.glows(), [640, 480], &trial.ui.light.maps);
        assert!(gains.any());
        assert_eq!([gain(&gains, 10, 10), gain(&gains, 60, 10), gain(&gains, 95, 10)], [1.0, 2.0, 1.0]);
        assert_eq!(gains.changed, Some([0, 0, 640, 480]));
        // A box over it puts out what it covers; only what changed is drawn again.
        gains.changed = None;
        trial.app = true;
        trial.run();
        gains.take(trial.ui.glows(), [640, 480], &trial.ui.light.maps);
        assert_eq!([gain(&gains, 62, 7), gain(&gains, 55, 2)], [1.0, 2.0]);
        assert_eq!(gains.changed, Some([50, 0, 90, 20]));
        // Nothing lit: nothing kept.
        gains.take(Vec::new(), [640, 480], &trial.ui.light.maps);
        assert!(!gains.any() && gains.plane.is_empty());
    }

    #[test]
    fn an_hdr_picture_lights_where_it_shows_by_its_gain_map() {
        let ramp: Vec<[f32; 4]> = (0..8).map(|x| [x as f32, x as f32, x as f32, 1.0]).collect();
        let mut trial = Trial::new(None, move |picture: &mut Option<Picture>, ui: &mut Ui| {
            let shown = *picture.get_or_insert_with(|| ui.add_hdr_picture(8, &ramp));
            ui.el().flow(Flow::Stack).size(200.0, 100.0).open();
            ui.el().offset(16.0, 8.0).picture(shown).size(16.0, 2.0).show();
            ui.close();
        });
        trial.resize(200.0, 100.0, 2.0);
        let mut gains = Gains::default();
        gains.take(trial.ui.glows(), [400, 200], &trial.ui.light.maps);
        // Each picture pixel is four window pixels across at scale 2.
        let row: Vec<f32> = (32..64).step_by(4).map(|x| from_half(gains.plane[17 * 400 + x])).collect();
        assert_eq!(row, [1.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(from_half(gains.plane[17 * 400 + 64]), 1.0);
    }
}
