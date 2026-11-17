//! Colour management: picture colours as the file means them. Most pictures are sRGB, say so or
//! say nothing. Some are wider: phone photos often Display P3, cameras and print shops Adobe
//! RGB. Shown as stored, they look dull; this converts them to the screen's sRGB.
//!
//! A file states its colours with an ICC profile, or in a PNG also with `gAMA` (the transfer
//! curve) and `cHRM` (primaries and white point). Either gives three things: a curve per
//! channel from value to linear light, a matrix mixing those into the screen's primaries, and
//! the screen's curve back. Results beyond the screen's range are clipped.
//!
//! Read: matrix/TRC profiles (screens, cameras, the wide spaces above) and grey ones, with
//! curves as a gamma, a table or a parametric formula. Not read: LUT profiles (printers',
//! inks), the screen's own profile (taken to be sRGB), and PNG's `cICP`. Unread profiles leave
//! colours as stored.
//!
//! All of it is the feature `color-profiles`, since what pictures a program gets cannot tell it
//! it is needed. Without it, every picture shows as stored.
//!
//! The arithmetic follows LittleCMS (the profiles' own colorants, Bradford adaptation), so
//! colours match it within a level or so: it computes at another precision and rounds its own
//! way.

/// The largest profile accepted: no screen's or camera's comes near.
pub(crate) const LONGEST: usize = 1 << 22;

/// The profile connection space white (D50), as X, Y, Z.
const WHITE: [f32; 3] = [0.9642, 1.0, 0.8249];

/// The sRGB primaries and white point: x and y of each.
const SCREEN: [f32; 8] = [0.64, 0.33, 0.30, 0.60, 0.15, 0.06, 0.3127, 0.3290];

/// A 3×3 matrix, row by row.
type Table = [[f32; 3]; 3];

/// One matrix doing both: `inner` first, then `outer`.
fn both(outer: &Table, inner: &Table) -> Table {
    std::array::from_fn(|row| std::array::from_fn(|column| (0..3).map(|k| outer[row][k] * inner[k][column]).sum()))
}

/// A matrix applied to a vector.
fn through(table: &Table, value: [f32; 3]) -> [f32; 3] {
    table.map(|row| row[0] * value[0] + row[1] * value[1] + row[2] * value[2])
}

/// The inverse of a matrix. Nothing if singular.
fn undone(t: &Table) -> Option<Table> {
    let minor = |r: usize, c: usize| t[(r + 1) % 3][(c + 1) % 3] * t[(r + 2) % 3][(c + 2) % 3] - t[(r + 1) % 3][(c + 2) % 3] * t[(r + 2) % 3][(c + 1) % 3];
    let whole = t[0][0] * minor(0, 0) + t[0][1] * minor(0, 1) + t[0][2] * minor(0, 2);
    whole.is_normal().then(|| std::array::from_fn(|row| std::array::from_fn(|column| minor(column, row) / whole)))
}

/// The sRGB transfer curve, from linear light 0..1 to value 0..1.
fn screen_value(light: f32) -> f32 {
    if light <= 0.003_130_8 { light * 12.92 } else { 1.055 * light.powf(1.0 / 2.4) - 0.055 }
}

/// The inverse: the linear light of an sRGB value.
fn screen_light(value: f32) -> f32 {
    if value <= 0.040_45 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
}

/// The matrix from linear RGB with primaries and white at `lie` (x and y of red, green, blue,
/// white) to XYZ, its white adapted to D50 by Bradford. Nothing for collinear primaries or an
/// impossible white.
fn table_of(lie: [f32; 8]) -> Option<Table> {
    let xyz = |x: f32, y: f32| [x / y, 1.0, (1.0 - x - y) / y];
    let [red, green, blue, white] = [0, 2, 4, 6].map(|at| xyz(lie[at], lie[at + 1]));
    let colours = [0, 1, 2].map(|row| [red[row], green[row], blue[row]]);
    // Each primary's share in the white, so all three make it.
    let shares = through(&undone(&colours)?, white);
    let table = colours.map(|row| [row[0] * shares[0], row[1] * shares[1], row[2] * shares[2]]);
    // Bradford's matrix into cone space, where each is scaled from the source white to D50; the
    // inverse follows.
    const CELLS: Table = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let (from, to) = (through(&CELLS, white), through(&CELLS, WHITE));
    let scaled = [[to[0] / from[0], 0.0, 0.0], [0.0, to[1] / from[1], 0.0], [0.0, 0.0, to[2] / from[2]]];
    let moved = both(&undone(&CELLS)?, &both(&scaled, &CELLS));
    let table = both(&moved, &table);
    table.iter().flatten().all(|number| number.is_finite()).then_some(table)
}

/// How a picture's colours convert to show as its file means them: per channel, the linear
/// light of each of its 256 values; the matrix into the screen's linear RGB (none for grey,
/// which stays grey); and the screen's value for a light in 4,096 steps (each value times 256,
/// plus one step to interpolate to).
pub(crate) struct Change {
    light: [Lights; 3],
    mix: Option<Table>,
    values: Vec<u16>,
}

impl Change {
    /// The conversion a colour profile asks for. Nothing if its colours are already the
    /// screen's (within what shows), if it is of a kind not read, or always without the feature
    /// `color-profiles`.
    pub(crate) fn of_profile(profile: &[u8]) -> Option<Change> {
        if !cfg!(feature = "color-profiles") {
            return None;
        }
        let read = Profile(profile);
        // The profile's colour space and PCS.
        if profile.get(20..24)? != b"XYZ " {
            return None;
        }
        let curve = |name: &[u8; 4]| read.tag(name)?.curve();
        let colour = |name: &[u8; 4]| read.tag(name)?.colour();
        match profile.get(16..20)? {
            b"GRAY" => Change::of([curve(b"kTRC")?; 3], None),
            b"RGB " => {
                let [red, green, blue] = [colour(b"rXYZ")?, colour(b"gXYZ")?, colour(b"bXYZ")?];
                let table = [0, 1, 2].map(|row| [red[row], green[row], blue[row]]);
                Change::of([curve(b"rTRC")?, curve(b"gTRC")?, curve(b"bTRC")?], Some(table))
            }
            _ => None,
        }
    }

    /// The conversion a PNG asks for with `gamma` (a value is the light it stands for to this
    /// power) and `lie` (x and y of its primaries and white). What it does not say is taken
    /// from sRGB, and a gamma just under 0.5 is taken as sRGB's curve, as its writers mean.
    /// Nothing if that comes to sRGB or is nonsense, or always without the feature
    /// `color-profiles`.
    pub(crate) fn of_light(gamma: Option<f32>, lie: Option<[f32; 8]>) -> Option<Change> {
        if !cfg!(feature = "color-profiles") {
            return None;
        }
        let lights = match gamma.filter(|gamma| (gamma * 2.2 - 1.0).abs() >= 0.01) {
            Some(gamma) => lights(|value| value.powf(1.0 / gamma)),
            None => lights(screen_light),
        };
        Change::of([lights; 3], Some(table_of(lie.unwrap_or(SCREEN))?))
    }

    /// The conversion for these curves, mixed by `table` into XYZ (none: grey). Nothing if that
    /// comes to sRGB: within half a level of its curve and a matrix within what could show of
    /// its own.
    fn of(light: [Lights; 3], table: Option<Table>) -> Option<Change> {
        let screen = table_of(SCREEN)?;
        let near = light.iter().flatten().enumerate().all(|(at, light)| (screen_value(*light) - (at % 256) as f32 / 255.0).abs() < 0.5 / 255.0);
        if near && table.as_ref().is_none_or(|table| close(table, &screen)) {
            return None;
        }
        let mix = match table {
            Some(table) => Some(both(&undone(&screen)?, &table)),
            None => None,
        };
        let values = (0..=4096).map(|step| (screen_value(step as f32 / 4096.0) * 255.0 * 256.0).round() as u16).collect();
        Some(Change { light, mix, values })
    }

    /// Converts pixels (`0xAARRGGBB`, alpha kept) as this says.
    pub(crate) fn apply(&self, pixels: &mut [u32]) {
        let [red, green, blue] = &self.light;
        for pixel in pixels.iter_mut() {
            let light = [red[(*pixel >> 16 & 255) as usize], green[(*pixel >> 8 & 255) as usize], blue[(*pixel & 255) as usize]];
            let light = self.mix.as_ref().map_or(light, |mix| through(mix, light));
            let [r, g, b] = light.map(|light| self.value(light));
            *pixel = *pixel & 0xff00_0000 | r << 16 | g << 8 | b;
        }
    }

    /// The screen's value 0..255 for a light (clamped to 0..1), interpolated between the two
    /// table steps around it.
    #[inline(always)]
    fn value(&self, light: f32) -> u32 {
        let at = (light.clamp(0.0, 1.0) * 65535.0 + 0.5) as u32;
        let (step, far) = ((at >> 4) as usize, at & 15);
        (self.values[step] as u32 * (16 - far) + self.values[step + 1] as u32 * far + 2048) >> 12
    }
}

/// Whether two matrices agree within what could show.
fn close(one: &Table, other: &Table) -> bool {
    one.iter().flatten().zip(other.iter().flatten()).all(|(one, other)| (one - other).abs() < 0.002)
}

/// The linear light of each of the 256 values, by a curve from value 0..1 to light 0..1
/// (clamped).
type Lights = [f32; 256];

/// The lights of the values by a curve.
fn lights(curve: impl Fn(f32) -> f32) -> Lights {
    std::array::from_fn(|value| curve(value as f32 / 255.0).clamp(0.0, 1.0))
}

/// A profile, read in place.
struct Profile<'a>(&'a [u8]);

impl<'a> Profile<'a> {
    /// A big-endian u32 at a place in the profile.
    fn long(&self, at: usize) -> Option<u32> {
        self.0.get(at..at.checked_add(4)?).map(|four| u32::from_be_bytes([four[0], four[1], four[2], four[3]]))
    }

    /// An s15Fixed16 number.
    fn fixed(&self, at: usize) -> Option<f32> {
        self.long(at).map(|long| long as i32 as f32 / 65536.0)
    }

    /// A tag's data by its signature. The tag table follows the header: a count, then per tag
    /// its signature, offset and size.
    fn tag(&self, name: &[u8; 4]) -> Option<Profile<'a>> {
        let count = self.long(128)? as usize;
        let entry = (0..count.min(1000)).map(|at| 132 + at * 12).find(|at| self.0.get(*at..*at + 4) == Some(&name[..]))?;
        let (start, long) = (self.long(entry + 4)? as usize, self.long(entry + 8)? as usize);
        self.0.get(start..start.checked_add(long)?).map(Profile)
    }

    /// A colour in an `XYZ ` tag: X, Y, Z.
    fn colour(&self) -> Option<[f32; 3]> {
        self.0.starts_with(b"XYZ ").then_some(())?;
        Some([self.fixed(8)?, self.fixed(12)?, self.fixed(16)?])
    }

    /// A curve tag: `curv`, evenly spaced points of two bytes each (no points: identity; one
    /// point: a gamma in u8Fixed8); or `para`, one of five formulas (see `formula`). Nothing
    /// for anything else.
    fn curve(&self) -> Option<Lights> {
        match (self.0.get(..4)?, self.long(8)?) {
            (b"curv", 0) => Some(lights(|value| value)),
            (b"curv", 1) => self.short(12).map(|power| lights(|value| value.powf(power / 256.0))),
            (b"curv", steps) => self.steps(steps as usize),
            (b"para", _) => self.para(),
            _ => None,
        }
    }

    /// A big-endian u16 at a place in the profile, as 0..1.
    fn short(&self, at: usize) -> Option<f32> {
        self.0.get(at..at + 2).map(|two| u16::from_be_bytes([two[0], two[1]]) as f32)
    }

    /// The curve of a `curv` tag of this many points.
    fn steps(&self, count: usize) -> Option<Lights> {
        let steps: Vec<f32> = (0..count.min(1 << 16)).map(|at| self.short(12 + at * 2).map(|step| step / 65535.0)).collect::<Option<_>>()?;
        Some(lights(|value| between(&steps, value)))
    }

    /// The curve of a `para` tag: which formula, and as many parameters as it has.
    fn para(&self) -> Option<Lights> {
        let kind = self.short(8)? as usize;
        let mut numbers = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        for (at, number) in numbers.iter_mut().enumerate().take(*[1, 3, 4, 5, 7].get(kind)?) {
            *number = self.fixed(12 + at * 4)?;
        }
        Some(lights(|value| formula(kind, numbers, value)))
    }
}

/// A value 0..1 among evenly spaced points, interpolated between its neighbours.
fn between(steps: &[f32], value: f32) -> f32 {
    let at = value * (steps.len() - 1) as f32;
    let (before, far) = (at.floor() as usize, at.fract());
    let after = (before + 1).min(steps.len() - 1);
    steps[before.min(steps.len() - 1)] * (1.0 - far) + steps[after] * far
}

/// The light of a value by one of the five ICC parametric curves, parameters g, a, b, c, d, e,
/// f (unused ones zero, except a, which is one). Each is `(a·value + b)^g` above a break and
/// linear below: type 0 with no break; types 1 and 2 broken where `a·value + b` is zero, with
/// nothing below and `c` added on both sides by type 2; types 3 and 4 broken at `d`, with
/// `c·value` below, and `e` above and `f` below added by type 4.
fn formula(kind: usize, [g, a, b, c, d, e, f]: [f32; 7], value: f32) -> f32 {
    let bend = [f32::MIN, -b / a, -b / a, d, d][kind];
    let (above, below) = ([0.0, 0.0, c, 0.0, e][kind], [0.0, 0.0, c, c * value, c * value + f][kind]);
    if value >= bend { (a * value + b).max(0.0).powf(g) + above } else { below }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    /// The primaries and white of Display P3 and of Adobe RGB.
    pub(crate) const P3: [f32; 8] = [0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290];
    pub(crate) const ADOBE: [f32; 8] = [0.640, 0.330, 0.210, 0.710, 0.150, 0.060, 0.3127, 0.3290];

    /// A screen profile as the ICC lays one out: primaries at `lie` (none: grey), each
    /// channel's curve `curve` (a `curv` or `para` tag as written).
    pub(crate) fn profile(lie: Option<[f32; 8]>, curve: &[u8]) -> Vec<u8> {
        let fixed = |number: f32| ((number * 65536.0).round() as i32).to_be_bytes();
        let colour = |xyz: [f32; 3]| [&b"XYZ \0\0\0\0"[..], &fixed(xyz[0]), &fixed(xyz[1]), &fixed(xyz[2])].concat();
        let mut tags: Vec<(&[u8; 4], Vec<u8>)> = vec![(b"wtpt", colour(WHITE))];
        match lie.and_then(table_of) {
            Some(table) => {
                tags.extend([b"rXYZ", b"gXYZ", b"bXYZ"].into_iter().enumerate().map(|(at, name)| (name, colour([table[0][at], table[1][at], table[2][at]]))));
                tags.extend([b"rTRC", b"gTRC", b"bTRC"].map(|name| (name, curve.to_vec())));
            }
            None => tags.push((b"kTRC", curve.to_vec())),
        }
        let (mut list, mut body) = ((tags.len() as u32).to_be_bytes().to_vec(), Vec::new());
        let start = 128 + 4 + 12 * tags.len();
        for (name, holds) in &tags {
            list.extend([&name[..], &((start + body.len()) as u32).to_be_bytes(), &(holds.len() as u32).to_be_bytes()].concat());
            body.extend(holds);
            body.resize(body.len().next_multiple_of(4), 0);
        }
        let mut head = vec![0u8; 128];
        head[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
        head[12..24].copy_from_slice(&[b"mntr".as_slice(), if lie.is_some() { b"RGB " } else { b"GRAY" }, b"XYZ "].concat());
        head[36..40].copy_from_slice(b"acsp");
        let all = [head, list, body].concat();
        [&(all.len() as u32).to_be_bytes()[..], &all[4..]].concat()
    }

    /// A gamma curve, as a `curv` tag writes it.
    pub(crate) fn power(power: f32) -> Vec<u8> {
        [&b"curv\0\0\0\0\0\0\0\x01"[..], &((power * 256.0).round() as u16).to_be_bytes()].concat()
    }

    /// The sRGB curve as a `para` tag writes it (type 3).
    pub(crate) fn screen_curve() -> Vec<u8> {
        let numbers = [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045];
        [&b"para\0\0\0\0\0\x03\0\0"[..], &numbers.iter().flat_map(|number| ((number * 65536.0f32).round() as i32).to_be_bytes()).collect::<Vec<u8>>()].concat()
    }

    #[test]
    fn colours_that_are_the_screens_change_nothing() {
        // sRGB, stated by a profile, by a PNG's chunks, or not at all.
        assert!(Change::of_profile(&profile(Some(SCREEN), &screen_curve())).is_none());
        assert!(Change::of_light(Some(0.45455), None).is_none() && Change::of_light(None, Some(SCREEN)).is_none() && Change::of_light(None, None).is_none());
        // Profiles not read: inks, other connection spaces, a `curv` too short; and bytes that
        // are no profile.
        let mut inks = profile(Some(SCREEN), &screen_curve());
        inks[16..20].copy_from_slice(b"CMYK");
        let mut other_terms = profile(Some(P3), &screen_curve());
        other_terms[20..24].copy_from_slice(b"Lab ");
        assert!([inks, other_terms, b"not a profile".to_vec(), Vec::new()].iter().all(|profile| Change::of_profile(profile).is_none()));
    }

    #[test]
    #[cfg(not(feature = "color-profiles"))]
    fn without_the_feature_no_picture_changes_its_colours() {
        // Wider colours, a grey of another gamma, a PNG's chunks: all shown as stored without
        // the feature.
        let profiles = [profile(Some(P3), &screen_curve()), profile(None, &power(1.8))];
        assert!(profiles.iter().all(|profile| Change::of_profile(profile).is_none()));
        assert!(Change::of_light(Some(1.0), None).is_none() && Change::of_light(None, Some(P3)).is_none());
    }

    #[test]
    #[cfg(feature = "color-profiles")]
    fn wider_colours_come_out_as_the_screen_shows_them() {
        let change = |profile: &[u8], pixel: u32| {
            let mut pixels = [pixel];
            Change::of_profile(profile).unwrap().apply(&mut pixels);
            pixels[0]
        };
        // Grey stays grey where the white is sRGB's; alpha stays.
        let p3 = profile(Some(P3), &screen_curve());
        assert_eq!([change(&p3, 0xff80_8080), change(&p3, 0x40ff_ffff), change(&p3, 0x0000_0000)], [0xff80_8080, 0x40ff_ffff, 0x0000_0000]);
        // Display P3's reddest red is beyond sRGB: clipped to sRGB's red. A duller P3 red is a
        // redder sRGB one.
        assert_eq!(change(&p3, 0xffff_0000), 0xffff_0000);
        assert!(change(&p3, 0xffc0_4040) >> 16 & 255 > 0xc8, "{:x}", change(&p3, 0xffc0_4040));
        // Grey with another gamma comes out lighter or darker, and stays grey.
        let grey = profile(None, &power(1.8));
        let lighter = change(&grey, 0xff40_4040);
        assert!(lighter & 255 > 0x40 && lighter & 0xffff == lighter >> 8 & 0xffff, "{lighter:x}");
    }

    #[test]
    fn curves_are_read_in_every_way_a_profile_writes_them() {
        let read = |curve: &[u8]| Profile(curve).curve().unwrap();
        let near = |lights: Lights, rule: &dyn Fn(f32) -> f32| lights.iter().enumerate().all(|(value, light)| (light - rule(value as f32 / 255.0).clamp(0.0, 1.0)).abs() < 1e-4);
        // Identity, a gamma, and evenly spaced points (here three, the middle a quarter).
        assert!(near(read(b"curv\0\0\0\0\0\0\0\0"), &|value| value));
        assert!(near(read(&power(2.2)), &|value| value.powf(563.0 / 256.0)));
        assert!(near(read(b"curv\0\0\0\0\0\0\0\x03\0\0\x40\0\xff\xff"), &|value| if value < 0.5 { value / 2.0 } else { 0.25 + (value - 0.5) * 1.5 }));
        // Each of the five formulas.
        let para = |kind: u8, numbers: &[f32]| {
            [&[b'p', b'a', b'r', b'a', 0, 0, 0, 0, 0, kind, 0, 0][..], &numbers.iter().flat_map(|number| ((number * 65536.0f32).round() as i32).to_be_bytes()).collect::<Vec<u8>>()].concat()
        };
        assert!(near(read(&para(0, &[2.0])), &|value| value * value));
        assert!(near(read(&para(1, &[2.0, 2.0, -0.5])), &|value| if value >= 0.25 { (2.0 * value - 0.5).powi(2) } else { 0.0 }));
        assert!(near(read(&para(2, &[1.0, 1.0, -0.5, 0.25])), &|value| if value >= 0.5 { value - 0.25 } else { 0.25 }));
        assert!(near(read(&screen_curve()), &screen_light));
        assert!(near(read(&para(4, &[1.0, 0.5, 0.0, 0.25, 0.5, 0.1, 0.05])), &|value| if value >= 0.5 { 0.5 * value + 0.1 } else { 0.25 * value + 0.05 }));
        // A formula that does not exist, and one cut short: no curve.
        assert!(Profile(&para(5, &[1.0; 7])).curve().is_none() && Profile(&para(4, &[1.0; 3])).curve().is_none());
    }

    #[test]
    fn a_damaged_profile_is_none_or_some_change() {
        // Cut short at every length, and each byte changed in turn: never a panic.
        let p3 = profile(Some(P3), &screen_curve());
        assert!((0..p3.len()).all(|cut| Change::of_profile(&p3[..cut]).is_none_or(|change| change.values.len() == 4097)));
        for at in 0..p3.len() {
            let damaged = |to: u8| [&p3[..at], &[to], &p3[at + 1..]].concat();
            for to in [p3[at] ^ (1 << (at % 8)), 0, 255] {
                if let Some(change) = Change::of_profile(&damaged(to)) {
                    change.apply(&mut [0xff12_3456, 0xffff_ffff, 0]);
                }
            }
        }
    }

    /// The test pictures not in sRGB: name, file, and LittleCMS's sRGB result (through vips).
    type Coloured = (&'static str, &'static [u8], &'static [u8]);
    pub(crate) const COLOURED: [Coloured; 8] = [
        // Display P3, as vips writes it into PNG, JPEG and WebP.
        ("p3.png", include_bytes!("../tests/pictures/p3.png"), include_bytes!("../tests/pictures/p3.png.srgb.png")),
        ("p3.jpg", include_bytes!("../tests/pictures/p3.jpg"), include_bytes!("../tests/pictures/p3.jpg.srgb.png")),
        ("p3.webp", include_bytes!("../tests/pictures/p3.webp"), include_bytes!("../tests/pictures/p3.webp.srgb.png")),
        // Adobe RGB (Ghostscript's profile) in a JPEG; ProPhoto's wider primaries and other
        // white in a PNG.
        ("adobe.jpg", include_bytes!("../tests/pictures/adobe.jpg"), include_bytes!("../tests/pictures/adobe.jpg.srgb.png")),
        ("prophoto.png", include_bytes!("../tests/pictures/prophoto.png"), include_bytes!("../tests/pictures/prophoto.png.srgb.png")),
        // Grey with another curve, in a JPEG.
        ("grey.jpg", include_bytes!("../tests/pictures/grey-profile.jpg"), include_bytes!("../tests/pictures/grey-profile.jpg.srgb.png")),
        // A PNG stating linear values (`gAMA` 1); and one with Adobe RGB primaries and gamma
        // 1.8 by `cHRM` and `gAMA`.
        ("light.png", include_bytes!("../tests/pictures/light.png"), include_bytes!("../tests/pictures/light.png.srgb.png")),
        ("chroma.png", include_bytes!("../tests/pictures/chroma.png"), include_bytes!("../tests/pictures/chroma.png.srgb.png")),
    ];

    /// The largest channel difference between two pictures, and how many pixels differ at all.
    #[cfg(feature = "color-profiles")]
    fn apart(one: &[u32], other: &[u32]) -> (u32, usize) {
        let worst = one.iter().zip(other).flat_map(|(one, other)| [16, 8, 0].map(|part| (one >> part & 255).abs_diff(other >> part & 255))).max().unwrap_or(0);
        (worst, one.iter().zip(other).filter(|(one, other)| (*one ^ *other) & 0xff_ffff != 0).count())
    }

    #[test]
    #[cfg(feature = "color-profiles")]
    fn colours_come_out_as_littlecms_makes_them() {
        // LittleCMS computes at another precision and rounds otherwise: a level apart at most,
        // in few pixels.
        for (name, file, wanted) in COLOURED {
            let read = match name.rsplit('.').next() {
                Some("jpg") => crate::jpeg::decode(file),
                Some("webp") => crate::webp::decode(file),
                _ => crate::png::decode(file),
            };
            let ((width, pixels), (wanted_width, wanted)) = (read.unwrap_or_else(|| panic!("{name} is read")), crate::png::decode(wanted).unwrap());
            assert_eq!((width, pixels.len()), (wanted_width, wanted.len()), "{name}: its size");
            let (worst, differ) = apart(&pixels, &wanted);
            assert!(worst <= 1 && differ * 10 < pixels.len(), "{name}: {differ} of {} pixels differ, by up to {worst}", pixels.len());
        }
    }

    /// Writes this module's test pictures afresh, and LittleCMS's result for each (through
    /// vips, with Ghostscript's profiles where vips has none).
    /// (`cargo test --lib profile -- --ignored`, with vips and Ghostscript.)
    #[test]
    #[ignore = "needs vips and Ghostscript's profiles, and writes the files the other tests read"]
    fn the_pictures_are_made_afresh() {
        let pictures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("pictures");
        let work = std::env::temp_dir().join(format!("taut-colours-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let run = |arguments: &[&str]| {
            let done = Command::new("vips").args(arguments).current_dir(&work).stdin(Stdio::null()).stderr(Stdio::null()).status();
            assert!(done.is_ok_and(|status| status.success()), "vips {arguments:?}");
        };
        let at = |name: &str| pictures.join(name).to_str().unwrap().to_string();
        let icc = |name: &str| format!("/usr/share/ghostscript/iccprofiles/{name}.icc");
        // A picture of every hue at many levels, and greys, in sRGB.
        let pixel = |at: usize| {
            let (x, y) = ((at % 64) as f32 / 63.0, (at / 64) as f32 / 47.0);
            let hue = |shift: f32| ((x * 6.0 + shift).rem_euclid(6.0) - 3.0).abs().clamp(1.0, 2.0) - 1.0;
            let level = |part: f32| ((part * (1.0 - y) + y * 0.5) * 255.0).round() as u32;
            0xff00_0000 | level(hue(0.0)) << 16 | level(hue(4.0)) << 8 | level(hue(2.0))
        };
        let source: Vec<u32> = (0..64 * 48).map(pixel).collect();
        std::fs::write(work.join("source.png"), crate::png::encode(64, &source).unwrap()).unwrap();
        run(&["icc_transform", "source.png", &at("p3.png"), "p3"]);
        run(&["icc_transform", "source.png", &format!("{}[Q=95]", at("p3.jpg")), "p3"]);
        run(&["icc_transform", "source.png", &format!("{}[lossless]", at("p3.webp")), "p3"]);
        run(&["icc_transform", "source.png", &format!("{}[Q=95]", at("adobe.jpg")), &icc("a98")]);
        run(&["icc_transform", "source.png", &at("prophoto.png"), &icc("rommrgb")]);
        run(&["colourspace", "source.png", "grey.png", "b-w"]);
        run(&["icc_transform", "grey.png", &format!("{}[Q=95]", at("grey-profile.jpg")), &icc("sgray")]);
        // The PNGs stating it by chunks: the picture as is, its chunks changed.
        let numbered = |chunks: &[(&[u8; 4], Vec<u8>)]| {
            let png = crate::png::encode(64, &source).unwrap();
            let rows = png.windows(4).position(|name| name == b"IDAT").unwrap() - 4;
            let said: Vec<u8> = chunks.iter().flat_map(|(name, body)| crate::png::chunk(name, body)).collect();
            [&png[..rows], &said, &png[rows..]].concat()
        };
        let number = |number: f32| ((number * 100_000.0).round() as u32).to_be_bytes();
        std::fs::write(at("light.png"), numbered(&[(b"gAMA", number(1.0).to_vec())])).unwrap();
        std::fs::write(at("chroma.png"), numbered(&[(b"gAMA", number(1.0 / 1.8).to_vec()), (b"cHRM", [6, 7, 0, 1, 2, 3, 4, 5].iter().flat_map(|at| number(ADOBE[*at])).collect())])).unwrap();
        std::fs::write(work.join("light.icc"), profile(Some(SCREEN), &power(1.0))).unwrap();
        std::fs::write(work.join("chroma.icc"), profile(Some(ADOBE), &power(1.8))).unwrap();
        // LittleCMS's sRGB result for each: by its own profile, or by one stating the same as
        // its chunks.
        for (name, ..) in COLOURED {
            let file = at(if name == "grey.jpg" { "grey-profile.jpg" } else { name });
            let given = match name {
                "light.png" => vec!["--input-profile", "light.icc"],
                "chroma.png" => vec!["--input-profile", "chroma.icc"],
                _ => vec!["--embedded"],
            };
            let out = format!("{}.srgb.png[keep=none]", file);
            run(&[&["icc_transform", &file, &out, "srgb"], &given[..]].concat());
        }
        std::fs::remove_dir_all(work).unwrap();
    }
}
