//! Colours: RGB and alpha.

/// `0xAARRGGBB`. [`Color::rgb(0x8b93ff)`](Color::rgb) is CSS `#8b93ff`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[must_use]
pub struct Color(pub u32);

impl Color {
    /// Fully transparent: nothing is drawn.
    pub const NONE: Color = Color(0);
    /// Black.
    pub const BLACK: Color = Color::rgb(0x000000);
    /// White.
    pub const WHITE: Color = Color::rgb(0xffffff);

    /// A solid colour, from `0xRRGGBB`.
    pub const fn rgb(rgb: u32) -> Self {
        Color(0xff00_0000 | rgb)
    }
    /// The same colour with this alpha: 0.0 transparent, 1.0 opaque.
    pub const fn alpha(self, alpha: f32) -> Self {
        let a = (self.a() as f32 * alpha.clamp(0.0, 1.0) + 0.5) as u32;
        Color(a << 24 | self.0 & 0x00ff_ffff)
    }
    /// The same colour, opaque.
    pub const fn solid(self) -> Self {
        Color(self.0 | 0xff00_0000)
    }
    /// Alpha, 0 (transparent) to 255.
    pub const fn a(self) -> u8 {
        (self.0 >> 24) as u8
    }
    /// Whether fully transparent.
    pub const fn is_none(self) -> bool {
        self.a() == 0
    }
    /// Whether nearer black than white, by perceived brightness.
    pub const fn is_dark(self) -> bool {
        (self.0 >> 16 & 0xff) * 299 + (self.0 >> 8 & 0xff) * 587 + (self.0 & 0xff) * 114 < 128_000
    }

    /// Linear blend toward `other`: 0 is this, 1 is `other`.
    #[inline]
    pub fn toward(self, other: Color, part: f32) -> Self {
        let t = (part.clamp(0.0, 1.0) * 256.0) as u32;
        // Red and blue blend in one step, alpha and green in another.
        let mix = |a: u32, b: u32| ((a * (256 - t) + b * t) >> 8) & 0x00ff_00ff;
        Color(mix(self.0 & 0x00ff_00ff, other.0 & 0x00ff_00ff) | mix(self.0 >> 8 & 0x00ff_00ff, other.0 >> 8 & 0x00ff_00ff) << 8)
    }

    /// An opaque colour from hue (0 to 1 is one turn: red, yellow, green, cyan, blue, magenta),
    /// saturation and value (0 to 1).
    pub fn hsv(hue: f32, strong: f32, light: f32) -> Self {
        // Red, green and blue each rise and fall once per turn, a third apart.
        let part = |ahead: f32| {
            let ramp = ((hue.rem_euclid(1.0) * 6.0 + ahead).rem_euclid(6.0) - 3.0).abs() - 1.0;
            (light * (1.0 - strong * (1.0 - ramp.clamp(0.0, 1.0))) * 255.0 + 0.5) as u32
        };
        Color(0xff00_0000 | part(0.0) << 16 | part(4.0) << 8 | part(2.0))
    }
    /// Hue, saturation and value, as [`hsv`](Self::hsv) takes them.
    pub fn to_hsv(self) -> [f32; 3] {
        let [r, g, b] = [self.0 >> 16, self.0 >> 8, self.0].map(|part| (part & 0xff) as f32 / 255.0);
        let (most, spread) = (r.max(g).max(b), r.max(g).max(b) - r.min(g).min(b));
        let sixth = match spread {
            _ if spread == 0.0 => 0.0,
            _ if most == r => (g - b) / spread,
            _ if most == g => 2.0 + (b - r) / spread,
            _ => 4.0 + (r - g) / spread,
        };
        [(sixth / 6.0).rem_euclid(1.0), if most == 0.0 { 0.0 } else { spread / most }, most]
    }
    /// Parses CSS hex: `#rrggbb` or `#rrggbbaa`; the `#` is optional.
    pub fn parse(text: &str) -> Option<Self> {
        let digits = text.trim().trim_start_matches('#');
        let value = u32::from_str_radix(digits, 16).ok()?;
        match digits.len() {
            6 => Some(Color::rgb(value)),
            8 => Some(Color(value.rotate_right(8))),
            _ => None,
        }
    }

    /// Perceptual blend in OKLab, where equal steps look equal: `part` of the way to `other`.
    /// [`toward`](Self::toward) blends the numbers instead (blue to yellow then passes through
    /// grey). Alpha weighs each, as CSS `color-mix(in oklab, ..)` does.
    pub fn mix(self, other: Color, part: f32) -> Self {
        let (t, [a, b]) = (part.clamp(0.0, 1.0), [self, other].map(|color| color.a() as f32 / 255.0));
        let solid = a + (b - a) * t;
        if solid <= 0.0 {
            return Color::NONE;
        }
        let ([l1, a1, b1], [l2, a2, b2]) = (self.to_oklab(), other.to_oklab());
        let weigh = |one: f32, two: f32| (one * a + (two * b - one * a) * t) / solid;
        Color::from_oklab([weigh(l1, l2), weigh(a1, a2), weigh(b1, b2)]).alpha(solid)
    }
    /// Lighter by `by` in OKLCH lightness (0 to 1), same hue, chroma clipped to what a screen
    /// shows.
    pub fn lighter(self, by: f32) -> Self {
        let [light, chroma, hue] = self.to_oklch();
        Color::oklch(light + by, chroma, hue).alpha(self.a() as f32 / 255.0)
    }
    /// Darker by `by`; see [`lighter`](Self::lighter).
    pub fn darker(self, by: f32) -> Self {
        self.lighter(-by)
    }
    /// An opaque colour from hue (0 to 1 is one turn), saturation and lightness (0 black, 0.5
    /// the colour, 1 white). CSS `hsl(h s l)` is `Color::hsl(h / 360.0, s, l)`, percentages as
    /// fractions.
    pub fn hsl(hue: f32, strong: f32, light: f32) -> Self {
        let light = light.clamp(0.0, 1.0);
        // The same colour as value and saturation, as `hsv` takes it.
        let most = light + strong.clamp(0.0, 1.0) * light.min(1.0 - light);
        Color::hsv(hue, if most > 0.0 { 2.0 * (1.0 - light / most) } else { 0.0 }, most)
    }
    /// An opaque colour in OKLCH: lightness (0 black, 1 white), chroma (0 grey; few screen
    /// colours exceed 0.3) and hue (0 to 1 is one turn: 0.08 red, 0.3 yellow, 0.4 green, 0.6
    /// cyan, 0.73 blue, 0.9 magenta). CSS `oklch(L C H)` is `Color::oklch(L, C, H / 360.0)`,
    /// `L` as a fraction. Out-of-gamut chroma is reduced to the most a screen shows.
    pub fn oklch(light: f32, chroma: f32, hue: f32) -> Self {
        let (sin, cos) = (hue * std::f32::consts::TAU).sin_cos();
        // Past the ends there is only white or black.
        if light >= 1.0 || light <= 0.0 {
            return if light >= 1.0 { Color::WHITE } else { Color::BLACK };
        }
        let chroma = chroma.max(0.0);
        let fits = |chroma: f32| linear_of([light, chroma * cos, chroma * sin]).iter().all(|part| (-1e-4..=1.0001).contains(part));
        // Bisects for the gamut edge, between all of the chroma and none (which always fits).
        let (mut fit, mut over) = (0.0, chroma);
        if !fits(chroma) {
            for _ in 0..24 {
                let half = (fit + over) / 2.0;
                (fit, over) = if fits(half) { (half, over) } else { (fit, half) };
            }
            return Color::from_oklab([light, fit * cos, fit * sin]);
        }
        Color::from_oklab([light, chroma * cos, chroma * sin])
    }
    /// Lightness, chroma and hue, as [`oklch`](Self::oklch) takes them. Grey's hue is 0.
    pub fn to_oklch(self) -> [f32; 3] {
        let [light, a, b] = self.to_oklab();
        let chroma = a.hypot(b);
        let hue = if chroma < 1e-6 { 0.0 } else { (b.atan2(a) / std::f32::consts::TAU).rem_euclid(1.0) };
        [light, chroma, hue]
    }
    /// WCAG 2 contrast ratio of text in this colour over `other`: 1 (same) to 21 (black on
    /// white). Text needs 4.5; large text and UI marks, 3. A translucent colour is first
    /// composited over `other`, which counts as opaque.
    pub fn contrast(self, other: Color) -> f32 {
        let under = other.solid();
        let over = Color(self.over(under.0, 255));
        let [one, two] = [over, under].map(|color| color.luminance());
        ((one.max(two) + 0.05) / (one.min(two) + 0.05)) as f32
    }
    /// The colour of chart series `nth` on `ground`. The eight are distinguishable by everyone,
    /// including red-green and blue-yellow colour blindness, between neighbours. Darker on
    /// light grounds than on dark ones. They repeat after eight: combine more series than that.
    pub fn series(nth: usize, ground: Color) -> Self {
        Color::rgb(SERIES[ground.is_dark() as usize][nth % 8])
    }

    /// WCAG relative luminance: 0 black to 1 white.
    fn luminance(self) -> f64 {
        let [red, green, blue] = self.linear().map(f64::from);
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    }
    /// Linear-light red, green and blue (0 to 1), from sRGB.
    fn linear(self) -> [f32; 3] {
        let unbend = |part: u32| {
            let part = (part & 0xff) as f32 / 255.0;
            if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
        };
        [unbend(self.0 >> 16), unbend(self.0 >> 8), unbend(self.0)]
    }
    /// OKLab coordinates: lightness, green-red, blue-yellow (Björn Ottosson's, as CSS uses).
    fn to_oklab(self) -> [f32; 3] {
        let [r, g, b] = self.linear();
        let l = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_99 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b).cbrt();
        [0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s, 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s, 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s]
    }
    /// The opaque colour at an OKLab point, each channel clamped to what a screen shows.
    fn from_oklab(lab: [f32; 3]) -> Self {
        let bend = |part: f32| {
            let part = part.clamp(0.0, 1.0);
            let bent = if part <= 0.003_130_8 { 12.92 * part } else { 1.055 * part.powf(1.0 / 2.4) - 0.055 };
            (bent * 255.0 + 0.5) as u32
        };
        let [r, g, b] = linear_of(lab);
        Color(0xff00_0000 | bend(r) << 16 | bend(g) << 8 | bend(b))
    }

    /// This colour over pixel `under`, with coverage `cover` (0 to 255). Pixels are
    /// premultiplied `0xAARRGGBB`, so a picture drawn over nothing can itself be drawn over
    /// anything.
    #[inline(always)]
    pub(crate) fn over(self, under: u32, cover: u32) -> u32 {
        let a = self.a() as u32 * cover / 255;
        if a == 0 {
            return under;
        }
        let (s, d) = (self.0 | 0xff00_0000, under);
        if a == 255 {
            return s;
        }
        // Red and blue blend in one step, alpha and green in another, each in its own byte.
        let mix = |s: u32, d: u32| (d + (s.wrapping_sub(d).wrapping_mul(a) >> 8)) & 0x00ff_00ff;
        mix(s & 0x00ff_00ff, d & 0x00ff_00ff) | mix(s >> 8 & 0x00ff_00ff, d >> 8 & 0x00ff_00ff) << 8
    }
}

/// Linear red, green and blue of an OKLab point; out of gamut, some fall outside 0 to 1.
fn linear_of([light, a, b]: [f32; 3]) -> [f32; 3] {
    let l = (light + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (light - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (light - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s, -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s, -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s]
}

/// Chart series on light and dark grounds: blue, orange, aqua, yellow, magenta, green, violet,
/// red. Ordered so neighbours differ for red-green colour blindness (tested below) and more so
/// for everyone else. On dark ground all reach 3:1 contrast; on light, blue, orange, green,
/// violet and red do, so a chart also names its series in words.
const SERIES: [[u32; 8]; 2] = [[0x2a78d6, 0xeb6834, 0x1baf7a, 0xeda100, 0xe87ba4, 0x008300, 0x6250d6, 0xe34948], [0x3987e5, 0xd95926, 0x199e70, 0xc98500, 0xd55181, 0x008300, 0x9085e9, 0xe66767]];

/// CSS hex: `#rrggbb`, plus two digits when not opaque.
impl std::fmt::Display for Color {
    fn fmt(&self, to: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self.a() {
            255 => write!(to, "#{:06x}", self.0 & 0x00ff_ffff),
            solid => write!(to, "#{:06x}{solid:02x}", self.0 & 0x00ff_ffff),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_goes_to_hue_and_text_and_back() {
        for (color, hsv) in [(0xff0000, [0.0, 1.0, 1.0]), (0x00ff00, [1.0 / 3.0, 1.0, 1.0]), (0x0000ff, [2.0 / 3.0, 1.0, 1.0]), (0x808080, [0.0, 0.0, 128.0 / 255.0])] {
            assert_eq!(Color::rgb(color).to_hsv(), hsv, "{color:06x}");
            assert_eq!(Color::hsv(hsv[0], hsv[1], hsv[2]), Color::rgb(color));
        }
        for color in [0x8b93ff, 0x123456, 0xfedcba, 0x000000, 0xffffff, 0x7f0001] {
            let [hue, strong, light] = Color::rgb(color).to_hsv();
            assert_eq!(Color::hsv(hue, strong, light), Color::rgb(color), "{color:06x} comes back the same");
        }
        assert_eq!(Color::rgb(0x8b93ff).to_string(), "#8b93ff");
        assert_eq!(Color::rgb(0x8b93ff).alpha(0.5).to_string(), "#8b93ff80");
        assert_eq!(Color::parse("#8b93ff"), Some(Color::rgb(0x8b93ff)));
        assert_eq!(Color::parse("8B93FF80"), Some(Color(0x808b_93ff)));
        assert_eq!(Color::parse("#12"), None);
    }

    #[test]
    fn laying_a_colour_over_another() {
        let red = Color::rgb(0xff0000);
        assert_eq!(red.over(0x0000ff, 255), 0xffff0000);
        assert_eq!(red.over(0x0000ff, 0), 0x0000ff);
        let half = red.alpha(0.5).over(0x000000, 255);
        assert!((0x7e7e0000..=0x80800000).contains(&half), "half red, half solid: {half:08x}");
        assert_eq!(Color::rgb(0x123456).alpha(0.0), Color(0x00123456));
    }

    #[test]
    fn colours_lie_in_oklab_where_web_pages_put_them() {
        // OKLCH of known colours, in double precision (red is `oklch(62.8% 0.2577 29.23)` in
        // CSS Color 4 too).
        let known = [
            (0xff0000, [0.62796, 0.25768, 0.08121]),
            (0x00ff00, [0.86644, 0.29483, 0.39582]),
            (0x0000ff, [0.45201, 0.31321, 0.73348]),
            (0x8b93ff, [0.70436, 0.15663, 0.77323]),
            (0xe8b04b, [0.79070, 0.13307, 0.22029]),
            (0x123456, [0.31917, 0.07245, 0.69769]),
        ];
        for (rgb, [light, chroma, hue]) in known {
            let [l, c, h] = Color::rgb(rgb).to_oklch();
            assert!((l - light).abs() < 2e-5 && (c - chroma).abs() < 2e-5 && (h - hue).abs() < 2e-5, "{rgb:06x}: {:?}", [l, c, h]);
            assert_eq!(Color::oklch(light, chroma, hue), Color::rgb(rgb), "{rgb:06x} comes back from its numbers");
        }
        let [white, grey, _] = Color::WHITE.to_oklch();
        assert!((white - 1.0).abs() < 1e-5 && grey < 1e-4);
        // Every colour of a grid round-trips to the bit.
        for rgb in (0..4096u32).map(|i| ((i >> 8) * 17) << 16 | ((i >> 4 & 15) * 17) << 8 | ((i & 15) * 17)) {
            let [l, c, h] = Color::rgb(rgb).to_oklch();
            assert_eq!(Color::oklch(l, c, h), Color::rgb(rgb), "{rgb:06x}");
        }
        // Mid grey to the eye is #636363 (`oklch(50% 0 0)`).
        assert_eq!(Color::oklch(0.5, 0.0, 0.0), Color::rgb(0x636363));
        // Out of gamut: chroma reduced to the edge, same lightness and hue; one step more would
        // not show.
        let [l, c, h] = Color::oklch(0.7, 0.4, 0.4).to_oklch();
        assert!((l - 0.7).abs() < 0.003 && (h - 0.4).abs() < 0.003 && c > 0.1 && c < 0.4, "{:?}", [l, c, h]);
        let past = linear_of([0.7, (c + 0.004) * (0.4 * std::f32::consts::TAU).cos(), (c + 0.004) * (0.4 * std::f32::consts::TAU).sin()]);
        assert!(past.iter().any(|part| !(0.0..=1.0).contains(part)), "{past:?}");
        assert_eq!((Color::oklch(1.5, 0.2, 0.3), Color::oklch(-1.0, 0.2, 0.3)), (Color::WHITE, Color::BLACK));
    }

    #[test]
    fn colours_are_mixed_lightened_and_darkened_as_the_eye_sees_them() {
        let (red, blue) = (Color::rgb(0xff0000), Color::rgb(0x0000ff));
        assert_eq!((red.mix(blue, 0.0), red.mix(blue, 1.0), red.mix(red, 0.4)), (red, blue, red));
        // Black and white half and half make perceptual mid grey; any blend lies where OKLab
        // says.
        assert_eq!(Color::BLACK.mix(Color::WHITE, 0.5), Color::rgb(0x636363));
        for (one, two, part) in [(0xff0000, 0x0000ff, 0.5), (0x8b93ff, 0xe8b04b, 0.3), (0x123456, 0xfedcba, 0.8)] {
            let ([l1, a1, b1], [l2, a2, b2]) = (Color::rgb(one).to_oklab(), Color::rgb(two).to_oklab());
            let [l, a, b] = Color::rgb(one).mix(Color::rgb(two), part).to_oklab();
            let want = [l1 + (l2 - l1) * part, a1 + (a2 - a1) * part, b1 + (b2 - b1) * part];
            assert!((l - want[0]).abs() < 0.004 && (a - want[1]).abs() < 0.004 && (b - want[2]).abs() < 0.004, "{one:06x} {two:06x}");
        }
        // Alpha weighs each: transparent gives no colour, only transparency.
        assert_eq!(red.mix(Color::NONE, 0.5), red.alpha(0.5));
        assert_eq!(Color::NONE.mix(Color::NONE, 0.5), Color::NONE);
        // Lighter and darker by a tenth, same hue.
        let accent = Color::rgb(0x8b93ff);
        let ([l, c, h], [lighter, _, hue], [darker, chroma, _]) = (accent.to_oklch(), accent.lighter(0.1).to_oklch(), accent.darker(0.1).to_oklch());
        assert!((lighter - l - 0.1).abs() < 0.004 && (l - darker - 0.1).abs() < 0.004 && (hue - h).abs() < 0.01 && (chroma - c).abs() < 0.004);
        assert_eq!((Color::WHITE.lighter(0.2), accent.darker(1.0), red.alpha(0.5).lighter(0.0)), (Color::WHITE, Color::BLACK, red.alpha(0.5)));
    }

    #[test]
    fn colours_come_from_hue_strength_and_lightness() {
        // As CSS writes them: `hsl(0 100% 50%)`, `hsl(120 100% 25%)`, `hsl(210 50% 60%)`.
        assert_eq!(Color::hsl(0.0, 1.0, 0.5), Color::rgb(0xff0000));
        assert_eq!(Color::hsl(120.0 / 360.0, 1.0, 0.25), Color::rgb(0x008000));
        assert_eq!(Color::hsl(210.0 / 360.0, 0.5, 0.6), Color::rgb(0x6699cc));
        assert_eq!((Color::hsl(0.3, 0.7, 0.0), Color::hsl(0.3, 0.7, 1.0), Color::hsl(0.3, 0.0, 0.5)), (Color::BLACK, Color::WHITE, Color::rgb(0x808080)));
    }

    #[test]
    fn contrast_is_counted_as_the_accessibility_guidelines_count_it() {
        assert_eq!(Color::BLACK.contrast(Color::WHITE), 21.0);
        assert_eq!(Color::WHITE.contrast(Color::BLACK), 21.0);
        assert_eq!(Color::rgb(0x8b93ff).contrast(Color::rgb(0x8b93ff)), 1.0);
        // #767676 is the lightest grey with enough contrast on white.
        assert!((Color::rgb(0x767676).contrast(Color::WHITE) - 4.54).abs() < 0.005);
        assert!((Color::rgb(0x777777).contrast(Color::WHITE) - 4.48).abs() < 0.005);
        // A translucent colour is composited first: half white on black is the grey it makes.
        assert_eq!(Color::WHITE.alpha(0.5).contrast(Color::BLACK), Color::rgb(0x7f7f7f).contrast(Color::BLACK));
        assert!((Color::rgb(0x808080).contrast(Color::BLACK) - 5.32).abs() < 0.005);
    }

    /// A colour as seen with protanopia, deuteranopia or tritanopia (Machado, Oliveira and
    /// Fernandes, 2009, full severity), as linear red, green and blue.
    fn seen(color: Color, by: usize) -> [f32; 3] {
        let ways = [
            [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]],
            [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.011820, 0.042940, 0.968881]],
            [[1.255528, -0.076749, -0.178779], [-0.078411, 0.930809, 0.147602], [0.004733, 0.691367, 0.303900]],
        ];
        let linear = color.linear();
        ways[by].map(|row| (row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2]).clamp(0.0, 1.0))
    }

    #[test]
    fn the_rows_of_a_chart_are_told_apart_by_everyone() {
        // Two colours are distinguishable at an OKLab distance of 8/100 as seen with colour
        // blindness, 15/100 otherwise. (Closest neighbours here: 9.1 and 19.6 on light, 8.4 and
        // 19.3 on dark.)
        let lab = |linear: [f32; 3]| {
            let [l, a, b] = [
                0.412_221_47 * linear[0] + 0.536_332_54 * linear[1] + 0.051_445_99 * linear[2],
                0.211_903_5 * linear[0] + 0.680_699_5 * linear[1] + 0.107_396_96 * linear[2],
                0.088_302_46 * linear[0] + 0.281_718_84 * linear[1] + 0.629_978_7 * linear[2],
            ]
            .map(f32::cbrt);
            [0.210_454_26 * l + 0.793_617_8 * a - 0.004_072_047 * b, 1.977_998_5 * l - 2.428_592_2 * a + 0.450_593_7 * b, 0.025_904_037 * l + 0.782_771_77 * a - 0.808_675_77 * b]
        };
        let apart = |one: [f32; 3], two: [f32; 3]| 100.0 * (lab(one)[0] - lab(two)[0]).hypot(lab(one)[1] - lab(two)[1]).hypot(lab(one)[2] - lab(two)[2]);
        for (ground, [least_seen, least]) in [(Color::WHITE, [9.1, 19.6]), (Color::rgb(0x151821), [8.4, 19.3])] {
            let rows: Vec<Color> = (0..8).map(|nth| Color::series(nth, ground)).collect();
            let pairs = || rows.windows(2).map(|pair| (pair[0], pair[1]));
            let worst_seen = pairs().flat_map(|(one, two)| [0, 1].map(|by| apart(seen(one, by), seen(two, by)))).fold(f32::INFINITY, f32::min);
            let worst = pairs().map(|(one, two)| apart(one.linear(), two.linear())).fold(f32::INFINITY, f32::min);
            assert!((worst_seen - least_seen).abs() < 0.05 && (worst - least).abs() < 0.05, "on {ground}: {worst_seen:.2} and {worst:.2}");
            // Each stands out from its ground: all on dark, the five named on light.
            let standing: Vec<bool> = rows.iter().map(|row| row.contrast(ground) >= 3.0).collect();
            let want = if ground.is_dark() { [true; 8] } else { [true, true, false, false, false, true, true, true] };
            assert_eq!(standing, want, "on {ground}");
        }
        // Repeats after eight; the same set on any light ground, and on any dark one.
        assert_eq!((Color::series(9, Color::WHITE), Color::series(3, Color::rgb(0xf5f6f8))), (Color::series(1, Color::WHITE), Color::series(3, Color::WHITE)));
        assert_ne!(Color::series(0, Color::WHITE), Color::series(0, Color::BLACK));
    }
}
