//! SVG filters: primitives an element's rendering passes through before it is composited. Each
//! makes an image from its inputs (the element, its alpha only, what lies under it, or earlier
//! results): blur, offset, flood, composite, blend, colour matrix and component transfer,
//! morphology, tile, turbulence, image, lighting, convolution, or displacement. The last result
//! is what is drawn of the element.
//!
//! All in pixels. The element is first drawn on its own layer (see `svg.rs`); the filter region
//! (rounded out to whole pixels) is taken from it, each primitive makes an image of that size,
//! filling its own subregion (by default all of it, or where its inputs are), and the last
//! image goes back into the layer, transparent everywhere else. Positions and lengths
//! (deviations, radii, offsets) are in the element's units, mapped to pixels when drawn.
//!
//! Colours are processed in linear light where the file says so (`color-interpolation-filters`,
//! the default), in 8 bits each, as librsvg does: every image knows its colour space and is
//! converted to a primitive's on input. The last is converted back.
//!
//! A blur works as the spec allows: for a deviation of 2 pixels or more, three successive box
//! blurs approximating the Gaussian (running sums, so cost per pixel doesn't grow with size);
//! below that, with the Gaussian itself, each pixel weighting its neighbours by the curve's
//! area over them, out to three deviations. (librsvg takes for each neighbour the curve's area
//! one pixel further out, which makes its small blurs narrower than stated.) Morphology uses
//! van Herk / Gil-Werman, also constant cost per pixel at any radius.
//!
//! `feImage` draws its element or picture on a block of its own just before the filter runs, and
//! `FillPaint` and `StrokePaint` the element's paint (see `svg.rs`); the filter takes those blocks
//! off the top. `BackgroundImage` is everything drawn under the element so far on blocks of its
//! size, composited, as `enable-background: new` on the root would make it. Lighting follows the
//! spec's normals and light vectors per pixel; convolution and displacement follow it too. With
//! `kernelUnitLength`, lighting and convolution run on a grid of kernel units, as librsvg does
//! (see `Room::on_grid`).
use crate::color::Color;
use crate::svg::{Block, Table};

/// A filter as applied to one element.
pub(crate) struct Filter {
    /// From its units (positions and lengths) to the drawing's.
    pub(crate) table: Table,
    /// The filter region in those units: left, top, width, height. Nothing of the element draws
    /// outside it. (Its edges are computed in double precision from the values as read, as
    /// librsvg does: an edge of -0.1 of a box lies a hair before the pixel it seems to fall on,
    /// which is then inside.)
    pub(crate) region: [f64; 4],
    pub(crate) steps: Vec<Step>,
    /// How many blocks on top of the element's hold its `feImage` images, the first lowest.
    pub(crate) images: usize,
}

impl Filter {
    /// The region's bounding box in drawing units: left, top, right, bottom.
    pub(crate) fn bounds(&self) -> [f32; 4] {
        let [x, y, w, h] = self.region;
        reach(&exact(&self.table), [x, y, x + w, y + h]).map(|side| side as f32)
    }
}

/// One filter primitive: its operation, inputs, subregion where set (left, top, width, height,
/// in units: unset parts are as large as its inputs, or the region), and colour space.
pub(crate) struct Step {
    pub(crate) kind: Kind,
    pub(crate) inputs: [Input; 2],
    pub(crate) place: [Option<f64>; 4],
    pub(crate) space: Space,
}

/// A primitive's input.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Input {
    /// `SourceGraphic`;
    Source,
    /// `SourceAlpha`, in black;
    Alpha,
    /// an earlier result (which);
    Made(usize),
    /// `BackgroundImage`: what was drawn under the element;
    Background,
    /// `BackgroundAlpha`: its alpha, in black;
    BackgroundAlpha,
    /// or one of the filter's images (an element's fill or stroke paint, see `svg.rs`).
    Image(usize),
}

/// A primitive's colour space.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Space {
    /// linearRGB: blends as light mixes.
    Linear,
    /// sRGB: as sent to the screen.
    Screen,
    /// Whatever space its input is in.
    Taken,
}

/// A primitive's operation.
pub(crate) enum Kind {
    /// `feGaussianBlur`: standard deviation across and down, in units.
    Blur([f32; 2]),
    /// `feOffset`: across and down.
    Offset([f32; 2]),
    /// `feFlood`.
    Flood(Color),
    /// `feMerge`: its inputs, each over the ones before.
    Merge(Vec<Input>),
    /// `feComposite`: the first onto the second by an operator.
    Composite(Operator),
    /// `feBlend`: the first over the second, colours blended.
    Blend(Mode),
    /// `feColorMatrix`: red, green, blue and alpha each recomputed from all four: four rows of
    /// five, the last added.
    Matrix([f32; 20]),
    /// `feComponentTransfer`: red, green, blue and alpha each by a function of itself.
    Transfer(Box<[Function; 4]>),
    /// `feDropShadow`: offset, blur deviation, colour.
    Shadow([f32; 2], [f32; 2], Color),
    /// `feMorphology`: dilate (`true`) or erode, by radii across and down.
    Morphology(bool, [f32; 2]),
    /// `feTile`: its input's subregion, tiled edge to edge.
    Tile,
    /// `feTurbulence` (see `Noise`).
    Noise(Noise),
    /// `feImage`: which of the filter's images.
    Image(usize),
    /// `feDiffuseLighting` and `feSpecularLighting` (see `Lighting`).
    Light(Box<Lighting>),
    /// `feConvolveMatrix` (see `Convolution`).
    Convolve(Box<Convolution>),
    /// `feDisplacementMap`: the scale in units (across and down), and which channels of the
    /// second input move pixels across and down (0 red, 1 green, 2 blue, 3 alpha).
    Displace([f32; 2], [usize; 2]),
}

/// A lighting step: specular (else diffuse), surface scale, its constant (`diffuseConstant` or
/// `specularConstant`), specular exponent, light colour, the light, and its kernel unit where
/// given (`kernelUnitLength`, see `Room::on_grid`).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Lighting {
    pub(crate) specular: bool,
    pub(crate) surface: f32,
    pub(crate) constant: f32,
    pub(crate) exponent: f32,
    pub(crate) color: Color,
    pub(crate) source: Source,
    pub(crate) unit: Option<[f32; 2]>,
}

/// A light, its positions in units (depth as the filter units say it).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Source {
    /// `feDistantLight`: azimuth and elevation, in degrees.
    Distant([f32; 2]),
    /// `fePointLight`: where it is.
    Point([f32; 3]),
    /// `feSpotLight`: where it is, where it points, its specular exponent, and its limiting
    /// cone's half angle in degrees, if any.
    Spot([f32; 3], [f32; 3], f32, Option<f32>),
}

/// A convolution step: kernel columns and rows, the kernel (row by row), divisor, bias, the
/// target pixel (column, row), how pixels past the edges are taken, whether alpha is kept, and
/// its kernel unit where given (`kernelUnitLength`, see `Room::on_grid`).
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Convolution {
    pub(crate) order: [usize; 2],
    pub(crate) kernel: Vec<f32>,
    pub(crate) divisor: f32,
    pub(crate) bias: f32,
    pub(crate) target: [usize; 2],
    pub(crate) edge: Edge,
    pub(crate) keep_alpha: bool,
    pub(crate) unit: Option<[f32; 2]>,
}

/// What a convolution takes for a pixel past the edge of its input: the nearest edge pixel,
/// the pixel from the other side, or nothing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Edge {
    Duplicate,
    Wrap,
    None,
}

/// How `feComposite` puts the first input onto the second.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Operator {
    /// over it;
    Over,
    /// as much as lies where the second is opaque;
    In,
    /// where the second is not;
    Out,
    /// over the second, only where that is;
    Atop,
    /// each where the other is not;
    Xor,
    /// added;
    Lighter,
    /// or each channel `k1 a b + k2 a + k3 b + k4`, `a` the first's and `b` the second's.
    Arithmetic([f32; 4]),
}

/// `feBlend` modes (Compositing and Blending's, in its order).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Mode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

impl Mode {
    /// The mode a file names.
    pub(crate) fn named(name: &str) -> Option<Mode> {
        let mode = match name {
            "normal" => Mode::Normal,
            "multiply" => Mode::Multiply,
            "screen" => Mode::Screen,
            "overlay" => Mode::Overlay,
            "darken" => Mode::Darken,
            "lighten" => Mode::Lighten,
            "color-dodge" => Mode::ColorDodge,
            "color-burn" => Mode::ColorBurn,
            "hard-light" => Mode::HardLight,
            "soft-light" => Mode::SoftLight,
            "difference" => Mode::Difference,
            "exclusion" => Mode::Exclusion,
            "hue" => Mode::Hue,
            "saturation" => Mode::Saturation,
            "color" => Mode::Color,
            "luminosity" => Mode::Luminosity,
            _ => return None,
        };
        Some(mode)
    }
}

/// A component transfer function of one channel, as a fraction 0 to 1.
#[derive(Clone, PartialEq, Debug)]
pub(crate) enum Function {
    /// Identity.
    Same,
    /// Linear interpolation through these values, spread evenly over 0 to 1;
    Table(Vec<f32>),
    /// steps of these values, equally wide;
    Discrete(Vec<f32>),
    /// a line: slope, intercept;
    Linear(f32, f32),
    /// or `amplitude c^exponent + offset`.
    Gamma(f32, f32, f32),
}

impl Function {
    /// Its value at `c`, 0 to 1.
    fn of(&self, c: f32) -> f32 {
        match self {
            Function::Same => c,
            Function::Table(values) if values.len() > 1 => {
                let steps = values.len() - 1;
                let k = ((c * steps as f32) as usize).min(steps - 1);
                values[k] + (c * steps as f32 - k as f32) * (values[k + 1] - values[k])
            }
            Function::Discrete(values) if !values.is_empty() => values[((c * values.len() as f32) as usize).min(values.len() - 1)],
            Function::Table(values) | Function::Discrete(values) => values.first().copied().unwrap_or(c),
            Function::Linear(slope, intercept) => slope * c + intercept,
            Function::Gamma(amplitude, exponent, offset) => amplitude * c.powf(*exponent) + offset,
        }
    }
}

/// Perlin noise as the spec defines it: base frequency (per unit, across and down), octave
/// count, seed, `stitchTiles`, and whether fractal noise (sum of octaves), else turbulence (sum
/// of absolute values).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Noise {
    pub(crate) base: [f32; 2],
    pub(crate) octaves: u32,
    pub(crate) seed: f32,
    pub(crate) stitch: bool,
    pub(crate) fractal: bool,
}

/// A primitive's cost, in multiples of a one-colour fill over its region, including copying its
/// inputs and converting them to linear light: measured (a one-colour fill costs about 6 ns per
/// pixel), plus a margin. A blur of any size costs 18; turbulence 11 per octave. (Every filter
/// costs `FIXED` more.)
pub(crate) fn weight(kind: &Kind) -> f32 {
    match kind {
        Kind::Flood(_) => 2.0,
        Kind::Tile => 3.0,
        Kind::Offset(_) | Kind::Transfer(_) => 5.0,
        Kind::Blend(Mode::Normal) => 6.0,
        Kind::Composite(_) => 8.0,
        Kind::Morphology(..) => 9.0,
        Kind::Matrix(_) => 12.0,
        Kind::Blend(_) => 14.0,
        Kind::Blur(_) => 18.0,
        Kind::Shadow(..) => 20.0,
        Kind::Merge(inputs) => 1.0 + 5.0 * inputs.len() as f32,
        Kind::Noise(noise) => 7.0 + 11.0 * noise.octaves as f32,
        Kind::Image(_) => 4.0,
        Kind::Displace(..) => 14.0,
        Kind::Light(lighting) => 40.0 + if lighting.unit.is_some() { GRID } else { 0.0 },
        Kind::Convolve(convolution) => 6.0 + 2.0 * convolution.kernel.len() as f32 + if convolution.unit.is_some() { GRID } else { 0.0 },
    }
}

/// What a step costs beyond its operation (as `weight` counts) for taking in what lies under
/// the element: compositing up to `BEHIND` blocks.
pub(crate) fn background_weight(step: &Step) -> f32 {
    let behind = step.inputs.iter().any(|input| matches!(input, Input::Background | Input::BackgroundAlpha));
    if behind { 3.0 * BEHIND as f32 } else { 0.0 }
}

/// How many blocks under an element its background is composited from, nearest first: the
/// blocks of groups, clips and masks it lies in.
const BEHIND: usize = 8;

/// A filter's cost besides its primitives, as `weight` counts: taking its region from the layer
/// and back, and converting to linear light and back.
pub(crate) const FIXED: f32 = 4.0;

/// What resampling a step's input onto a grid of kernel units and its result back costs, as
/// `weight` counts (see `Room::on_grid`).
const GRID: f32 = 8.0;

/// The most turbulence octaves: more change nothing 8 bits can show.
pub(crate) const OCTAVES: u32 = 12;

/// The colour matrix that saturates (above 1) or desaturates (below), as
/// `feColorMatrix type="saturate"` defines it.
pub(crate) fn saturated(s: f32) -> [f32; 20] {
    #[rustfmt::skip]
    let matrix = [
        0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0.0, 0.0,
        0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0.0, 0.0,
        0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    matrix
}

/// The colour matrix that rotates hue by `degrees`.
pub(crate) fn hue_turned(degrees: f32) -> [f32; 20] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    #[rustfmt::skip]
    let matrix = [
        0.213 + cos * 0.787 - sin * 0.213, 0.715 - cos * 0.715 - sin * 0.715, 0.072 - cos * 0.072 + sin * 0.928, 0.0, 0.0,
        0.213 - cos * 0.213 + sin * 0.143, 0.715 + cos * 0.285 + sin * 0.140, 0.072 - cos * 0.072 - sin * 0.283, 0.0, 0.0,
        0.213 - cos * 0.213 - sin * 0.787, 0.715 - cos * 0.715 + sin * 0.715, 0.072 + cos * 0.928 + sin * 0.072, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    matrix
}

/// The colour matrix turning luminance into alpha, in black.
#[rustfmt::skip]
pub(crate) const LUMINANCE: [f32; 20] = [
    0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 0.0, 0.0,
    0.2125, 0.7154, 0.0721, 0.0, 0.0,
];

/// The identity colour matrix.
#[rustfmt::skip]
pub(crate) const SAME: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.0, 1.0, 0.0,
];

/// The colour matrix of `grayscale(amount)` and `sepia(amount)` (`sepia` true), as the spec
/// defines them.
pub(crate) fn toned(amount: f32, sepia: bool) -> [f32; 20] {
    let rest = 1.0 - amount.clamp(0.0, 1.0);
    let [r, g, b, keep] = match sepia {
        true => [[0.393, 0.769, 0.189], [0.349, 0.686, 0.168], [0.272, 0.534, 0.131], [0.607, 0.314, 0.869]],
        false => [[0.2126, 0.7152, 0.0722], [0.2126, 0.7152, 0.0722], [0.2126, 0.7152, 0.0722], [0.7874, 0.2848, 0.9278]],
    };
    #[rustfmt::skip]
    let matrix = [
        r[0] + keep[0] * rest, r[1] - r[1] * rest, r[2] - r[2] * rest, 0.0, 0.0,
        g[0] - g[0] * rest, g[1] + keep[1] * rest, g[2] - g[2] * rest, 0.0, 0.0,
        b[0] - b[0] * rest, b[1] - b[1] * rest, b[2] + keep[2] * rest, 0.0, 0.0,
        0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    matrix
}

/// An image a primitive made, the size of the filter's part of the layer: premultiplied canvas
/// pixels, whether in linear light, and the subregion it filled (left, top, right, bottom,
/// pixels from its corner), outside which it is transparent.
#[derive(Clone)]
struct Picture {
    pixels: Vec<u32>,
    linear: bool,
    part: [usize; 4],
}

/// Where a filter works: its part of the layer (left and top there, width and height in
/// pixels), and where that part's corner lies in the whole picture; the region within that part,
/// in fractional pixels (left, top, right, bottom); and the transform from filter units to its
/// pixels, and back.
struct Room {
    left: usize,
    top: usize,
    wide: usize,
    high: usize,
    corner: [f64; 2],
    region: [f64; 4],
    table: Exact,
    back: Exact,
}

/// What a filter takes in besides the element: its `feImage` images and what lies under it,
/// each as large as its room.
struct Taken {
    images: Vec<Picture>,
    background: Option<Picture>,
}

/// Filters the top block of a drawing being drawn: its pixels (`wide` per row, its first row
/// being row `top` of the image, which its `fit` maps drawing units onto). The filter's images
/// lie on top of it, and are dropped. Outside the region, or with no primitives, nothing is
/// left.
pub(crate) fn apply(filter: &Filter, blocks: &mut Vec<Block>) {
    // (The reader begins a block for each image, so all are there; the element's stays.)
    let mut images = Vec::with_capacity(filter.images);
    while images.len() < filter.images && blocks.len() > 1 {
        images.extend(blocks.pop());
    }
    images.reverse();
    let Some((block, under)) = blocks.split_last_mut() else { return };
    let (wide, high) = (block.wide, block.pixels.len() / block.wide.max(1));
    let Some(room) = Room::of(filter, wide, high, block.top, &block.fit) else {
        return block.pixels.fill(0);
    };
    let light = Light::new();
    let source = room.taken(&block.pixels, wide);
    let sized = |image: &Block| if image.pixels.len() == block.pixels.len() { room.taken(&image.pixels, wide) } else { room.clear() };
    let behind = filter.steps.iter().any(|step| background_weight(step) > 0.0);
    let taken = Taken { images: images.iter().map(sized).collect(), background: behind.then(|| room.behind(under, block.pixels.len(), wide)) };
    let mut made: Vec<Picture> = Vec::with_capacity(filter.steps.len());
    for step in &filter.steps {
        let picture = room.step(step, &source, (&made, &taken), &light);
        made.push(picture);
    }
    block.pixels.fill(0);
    if let Some(last) = made.pop() {
        room.put(&light.taken(last, false), &mut block.pixels, wide);
    }
}

impl Room {
    /// Where a filter works on a `wide` by `high` layer (see `apply`). `None` if the region
    /// lies wholly outside, is empty, or its transform can't be inverted.
    fn of(filter: &Filter, wide: usize, high: usize, top: i32, fit: &Table) -> Option<Room> {
        let table = both(&[1.0, 0.0, 0.0, 1.0, 0.0, -f64::from(top)], &both(&exact(fit), &exact(&filter.table)));
        let [x, y, w, h] = filter.region;
        let [l, t, r, b] = reach(&table, [x, y, x + w, y + h]);
        let [l, t, r, b] = [l.max(0.0), t.max(0.0), r.min(wide as f64), b.min(high as f64)];
        if !(l < r && t < b && w > 0.0 && h > 0.0) {
            return None;
        }
        let ([left, upper], [right, lower]) = ([l.floor() as usize, t.floor() as usize], [(r.ceil() as usize).min(wide), (b.ceil() as usize).min(high)]);
        let table = both(&[1.0, 0.0, 0.0, 1.0, -(left as f64), -(upper as f64)], &table);
        let region = [l - left as f64, t - upper as f64, r - left as f64, b - upper as f64];
        let corner = [left as f64, f64::from(top) + upper as f64];
        Some(Room { left, top: upper, wide: right - left, high: lower - upper, corner, region, back: undone(&table)?, table })
    }

    /// The filter's part of a layer, as an image in sRGB.
    fn taken(&self, pixels: &[u32], wide: usize) -> Picture {
        let mut taken = Vec::with_capacity(self.wide * self.high);
        for row in self.top..self.top + self.high {
            taken.extend_from_slice(&pixels[row * wide + self.left..row * wide + self.left + self.wide]);
        }
        Picture { pixels: taken, linear: false, part: [0, 0, self.wide, self.high] }
    }

    /// Puts an image back into the layer it came from.
    fn put(&self, picture: &Picture, pixels: &mut [u32], wide: usize) {
        for (row, line) in picture.pixels.chunks_exact(self.wide).enumerate() {
            let at = (self.top + row) * wide + self.left;
            pixels[at..at + self.wide].copy_from_slice(line);
        }
    }

    /// A transparent image.
    fn clear(&self) -> Picture {
        Picture { pixels: vec![0; self.wide * self.high], linear: false, part: [0, 0, self.wide, self.high] }
    }

    /// What lies under the filtered block within the room: the blocks under it of its size
    /// (`long` pixels, `wide` per row; up to `BEHIND`, nearest first), composited from the
    /// lowest, in sRGB.
    fn behind(&self, under: &[Block], long: usize, wide: usize) -> Picture {
        let same = under.iter().rev().take_while(|block| block.pixels.len() == long).take(BEHIND).count();
        let mut picture = self.clear();
        for block in &under[under.len() - same..] {
            let part = self.taken(&block.pixels, wide);
            picture.pixels.iter_mut().zip(&part.pixels).for_each(|(under, top)| *under = over(*top, *under));
        }
        picture
    }

    /// Runs one primitive on the element's rendering (`source`), earlier results, and what the
    /// filter takes in besides.
    fn step(&self, step: &Step, source: &Picture, (made, taken): (&[Picture], &Taken), light: &Light) -> Picture {
        let (part, exact) = self.part(step, made);
        let wanted = match step.space {
            Space::Linear => Some(true),
            Space::Screen => Some(false),
            Space::Taken => None,
        };
        let take = |n: usize| self.input(step.inputs[n], source, (made, taken), wanted, light);
        let (wide, t) = (self.wide, self.table.map(|n| n as f32));
        let scale = [t[0].hypot(t[2]), t[1].hypot(t[3])];
        let picture = match &step.kind {
            Kind::Blur(deviation) => blurred(take(0), part, wide, [deviation[0] * scale[0], deviation[1] * scale[1]]),
            Kind::Offset([dx, dy]) => moved(take(0), (part, exact), wide, [t[0] * dx + t[2] * dy, t[1] * dx + t[3] * dy]),
            Kind::Flood(color) => flooded(*color, part, wide, self.high),
            Kind::Merge(inputs) => merged(inputs.iter().map(|input| self.input(*input, source, (made, taken), wanted, light)), part, wide, self.high, wanted),
            Kind::Composite(operator) => composited(take(0), &take(1), *operator, part, wide),
            Kind::Blend(mode) => blended(take(0), &take(1), *mode, part, wide),
            Kind::Matrix(matrix) => matrixed(take(0), matrix, part, wide),
            Kind::Transfer(functions) => transferred(take(0), functions, part, wide),
            Kind::Shadow(by, deviation, color) => {
                let ([dx, dy], spread) = (*by, [deviation[0] * scale[0], deviation[1] * scale[1]]);
                shadowed(take(0), (part, exact), wide, [t[0] * dx + t[2] * dy, t[1] * dx + t[3] * dy], spread, *color)
            }
            Kind::Morphology(thicker, radius) => thickened(take(0), part, wide, *thicker, [radius[0] * scale[0], radius[1] * scale[1]]),
            Kind::Tile => tiled(take(0), part, wide),
            Kind::Noise(noise) => self.noise(noise, part, wanted.unwrap_or(false)),
            Kind::Image(image) => light.taken(taken.images.get(*image).cloned().unwrap_or_else(|| self.clear()), wanted.unwrap_or(false)),
            Kind::Light(lighting) => self.on_grid(lighting.unit, take(0), part, |input, (wide, part), place| self.lit(lighting, &input, (wide, part), place, (wanted.unwrap_or(false), light))),
            Kind::Convolve(convolution) => self.on_grid(convolution.unit, take(0), part, |input, (wide, part), _| convolved(input, convolution, part, wide)),
            // The image moved is taken as it is; the map in the step's colour space.
            Kind::Displace(by, channels) => {
                let moved = self.input(step.inputs[0], source, (made, taken), None, light);
                displaced(moved, &take(1), part, wide, [by[0] * scale[0], by[1] * scale[1]], *channels)
            }
        };
        // Nothing is made outside its subregion: a larger input is cut down.
        clear_outside(picture, part, wide)
    }

    /// A primitive's subregion in pixels. Each side it sets is in units; unset sides are as
    /// large as its inputs, if those are only earlier results, else the region. Within the
    /// region, rounded out to whole pixels; and exact, in fractional pixels (left, top, right,
    /// bottom).
    fn part(&self, step: &Step, made: &[Picture]) -> ([usize; 4], [f64; 4]) {
        let merged = match &step.kind {
            Kind::Merge(inputs) => &inputs[..],
            _ => &[],
        };
        let used = match step.kind {
            Kind::Flood(_) | Kind::Noise(_) | Kind::Tile | Kind::Image(_) => &[][..],
            Kind::Composite(_) | Kind::Blend(_) | Kind::Displace(..) => &step.inputs[..],
            Kind::Merge(_) => merged,
            _ => &step.inputs[..1],
        };
        let parts: Option<Vec<[usize; 4]>> = used.iter().map(|input| if let Input::Made(n) = input { made.get(*n).map(|made| made.part) } else { None }).collect();
        let pixels = parts.filter(|parts| !parts.is_empty()).map(|parts| parts.iter().fold([usize::MAX, usize::MAX, 0, 0], |a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]));
        let pixels = pixels.map_or(self.region, |[l, t, r, b]| [l as f64, t as f64, r as f64, b as f64]);
        let [mut l, mut t, mut r, mut b] = reach(&self.back, pixels);
        if let Some(x) = step.place[0] {
            (l, r) = (x, x + r - l);
        }
        if let Some(y) = step.place[1] {
            (t, b) = (y, y + b - t);
        }
        r = step.place[2].map_or(r, |w| l + w);
        b = step.place[3].map_or(b, |h| t + h);
        let [l, t, r, b] = reach(&self.table, [l, t, r, b]);
        let [l, t, r, b] = [l.max(self.region[0]), t.max(self.region[1]), r.min(self.region[2]), b.min(self.region[3])];
        if !(l < r && t < b) {
            return ([0; 4], [0.0; 4]);
        }
        ([l.floor() as usize, t.floor() as usize, (r.ceil() as usize).min(self.wide), (b.ceil() as usize).min(self.high)], [l, t, r, b])
    }

    /// A primitive's input, in its colour space (`wanted`; `None`: any).
    fn input(&self, input: Input, source: &Picture, (made, taken): (&[Picture], &Taken), wanted: Option<bool>, light: &Light) -> Picture {
        let alpha = |picture: &Picture| Picture { pixels: picture.pixels.iter().map(|pixel| pixel & 0xff00_0000).collect(), linear: wanted.unwrap_or(false), part: picture.part };
        let behind = || taken.background.clone().unwrap_or_else(|| self.clear());
        let picture = match input {
            Input::Source => source.clone(),
            Input::Alpha => alpha(source),
            Input::Made(n) => made.get(n).cloned().unwrap_or_else(|| self.clear()),
            Input::Background => behind(),
            Input::BackgroundAlpha => alpha(&behind()),
            Input::Image(image) => taken.images.get(image).cloned().unwrap_or_else(|| self.clear()),
        };
        match wanted {
            Some(linear) => light.taken(picture, linear),
            None => picture,
        }
    }

    /// Turbulence over a subregion, in linear light or not: each pixel's at its corner's
    /// position in units, as librsvg takes it.
    fn noise(&self, noise: &Noise, part: [usize; 4], linear: bool) -> Picture {
        let lattice = Lattice::new(noise.seed);
        let [l, t, r, b] = reach(&self.back, part.map(|side| side as f64));
        let mut pixels = vec![0; self.wide * self.high];
        for y in part[1]..part[3] {
            for x in part[0]..part[2] {
                let at = through(&self.back, [x as f64, y as f64]);
                pixels[y * self.wide + x] = lattice.pixel(noise, at, [l, t, r - l, b - t]);
            }
        }
        Picture { pixels, linear, part }
    }

    /// A lighting step over a subregion of an input `wide` pixels per row, the input's alpha as
    /// the surface's height: in linear light or not. Its pixels lie in the room as `place` says
    /// (scale across and down, then shift: see `on_grid`). Diffuse light is opaque; specular
    /// light as solid as its brightest channel.
    fn lit(&self, lighting: &Lighting, input: &Picture, (wide, part): (usize, [usize; 4]), place: [f32; 4], (linear, light): (bool, &Light)) -> Picture {
        let [_, red, green, blue] = four(lighting.color.0);
        let color = [red, green, blue].map(|c| if linear { light.linear[c as usize] as f32 } else { c as f32 } / 255.0);
        let source = self.placed(&lighting.source);
        let mut pixels = vec![0; input.pixels.len()];
        for y in part[1]..part[3] {
            for x in part[0]..part[2] {
                pixels[y * wide + x] = lit_pixel(lighting, &source, color, (&input.pixels, wide, part), ([x, y], place));
            }
        }
        Picture { pixels, linear, part }
    }

    /// Runs a step on its input as if on a grid of `unit` (in filter units) per pixel, as
    /// `kernelUnitLength` asks and librsvg does it: the input resampled onto the grid (cells
    /// laid from the picture's corner), the step run there (given the grid's picture, its width
    /// and part, and its pixels' place in the room: see `lit`), and the result resampled back
    /// within `part`. A cell finer than a pixel is taken as one pixel. Without a unit, the step
    /// runs on the input as it is.
    fn on_grid(&self, unit: Option<[f32; 2]>, input: Picture, part: [usize; 4], step: impl Fn(Picture, (usize, [usize; 4]), [f32; 4]) -> Picture) -> Picture {
        let t = self.table;
        let size = unit.map(|[dx, dy]| [(t[0] * f64::from(dx) + t[2] * f64::from(dy)).abs(), (t[1] * f64::from(dx) + t[3] * f64::from(dy)).abs()]);
        let Some(size) = size.filter(|size| size.iter().all(|side| side.is_finite())).map(|size| size.map(|side| side.clamp(1.0, self.wide.max(self.high).max(1) as f64))) else {
            return step(input, (self.wide, part), [1.0, 1.0, 0.0, 0.0]);
        };
        let ([wide, high], phase) = ([self.wide, self.high], [self.corner[0].rem_euclid(size[0]), self.corner[1].rem_euclid(size[1])]);
        let cells: [usize; 2] = std::array::from_fn(|axis| ((([wide, high][axis] as f64) + phase[axis]) / size[axis]).ceil() as usize);
        // Each cell covers so many pixels; each pixel, a share of a cell.
        let onto: [Taps; 2] = std::array::from_fn(|axis| taps(cells[axis], [wide, high][axis], |cell| (cell as f64 * size[axis] - phase[axis], (cell + 1) as f64 * size[axis] - phase[axis])));
        let back: [Taps; 2] = std::array::from_fn(|axis| taps([wide, high][axis], cells[axis], |at| ((at as f64 + phase[axis]) / size[axis], (at as f64 + 1.0 + phase[axis]) / size[axis])));
        let low = |at: usize, axis: usize| ((at as f64 + phase[axis]) / size[axis]).floor() as usize;
        let high_of = |at: usize, axis: usize| (((at as f64 + phase[axis]) / size[axis]).ceil() as usize).min(cells[axis]);
        let grid_part = [low(part[0], 0), low(part[1], 1), high_of(part[2], 0), high_of(part[3], 1)];
        let grid = Picture { pixels: resampled(&input.pixels, wide, &onto), linear: input.linear, part: grid_part };
        let place = [size[0] as f32, size[1] as f32, -phase[0] as f32, -phase[1] as f32];
        let done = step(grid, (cells[0], grid_part), place);
        clear_outside(Picture { pixels: resampled(&done.pixels, cells[0], &back), linear: done.linear, part }, part, wide)
    }

    /// A light with its positions in the room's pixels. (Depths are scaled as lengths are, by
    /// the transform's mean scale.)
    fn placed(&self, source: &Source) -> Source {
        let scale = (self.table[0] * self.table[3] - self.table[1] * self.table[2]).abs().sqrt() as f32;
        let place = |[x, y, z]: [f32; 3]| {
            let [x, y] = through(&self.table, [x.into(), y.into()]);
            [x as f32, y as f32, z * scale]
        };
        match *source {
            Source::Distant(angles) => Source::Distant(angles),
            Source::Point(at) => Source::Point(place(at)),
            Source::Spot(at, to, exponent, cone) => Source::Spot(place(at), place(to), exponent, cone),
        }
    }
}

/// One pixel of a lighting step (see `Room::lit`): the light from `source` (in pixels), of
/// `color`, on the surface the input's alpha makes (`wide` per row, the subregion `part`), at
/// pixel `[x, y]`, which lies in the room as `place` says.
fn lit_pixel(lighting: &Lighting, source: &Source, color: [f32; 3], (pixels, wide, part): (&[u32], usize, [usize; 4]), ([x, y], place): ([usize; 2], [f32; 4])) -> u32 {
    let height = |x: usize, y: usize| (pixels[y * wide + x] >> 24) as f32 / 255.0;
    let [nx, ny] = [true, false].map(|across| -lighting.surface * slope(&height, [x, y], part, across));
    let normal = unit([nx, ny, 1.0]);
    let surface = [x as f32 * place[0] + place[2], y as f32 * place[1] + place[3], lighting.surface * height(x, y)];
    // The direction to the light, and how much of its colour reaches here.
    let (to_light, share) = match *source {
        Source::Distant([azimuth, elevation]) => {
            let (a, e) = (azimuth.to_radians(), elevation.to_radians());
            ([a.cos() * e.cos(), a.sin() * e.cos(), e.sin()], 1.0)
        }
        Source::Point(at) => (unit([at[0] - surface[0], at[1] - surface[1], at[2] - surface[2]]), 1.0),
        Source::Spot(at, to, exponent, cone) => {
            let to_light = unit([at[0] - surface[0], at[1] - surface[1], at[2] - surface[2]]);
            let aim = unit([to[0] - at[0], to[1] - at[1], to[2] - at[2]]);
            let facing = -(to_light[0] * aim[0] + to_light[1] * aim[1] + to_light[2] * aim[2]);
            let inside = facing > 0.0 && cone.is_none_or(|cone| facing >= cone.to_radians().cos());
            (to_light, if inside { facing.powf(exponent) } else { 0.0 })
        }
    };
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let level = |part: f32| (part.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    if !lighting.specular {
        let lit = lighting.constant * dot(normal, to_light).max(0.0) * share;
        let [r, g, b] = color.map(|c| level(lit * c));
        return pixel([255, r, g, b]);
    }
    let halfway = unit([to_light[0], to_light[1], to_light[2] + 1.0]);
    let lit = lighting.constant * dot(normal, halfway).max(0.0).powf(lighting.exponent) * share;
    let [r, g, b] = color.map(|c| level(lit * c));
    pixel([r.max(g).max(b), r, g, b])
}

/// A vector made one long (left as it is if of no length).
fn unit([x, y, z]: [f32; 3]) -> [f32; 3] {
    let long = (x * x + y * y + z * z).sqrt();
    if long > 0.0 { [x / long, y / long, z / long] } else { [x, y, z] }
}

/// How a surface's height changes at a pixel, across or down, as the spec's Sobel kernels
/// compute it: from the pixels on both sides where there are, else from the pixel itself and
/// the one there is; the rows (or columns) beside weighted half the middle one's. (Nothing at
/// all in a subregion one pixel across.)
fn slope(height: &impl Fn(usize, usize) -> f32, [x, y]: [usize; 2], [l, t, r, b]: [usize; 4], across: bool) -> f32 {
    // Along the slope, and beside it: the coordinate, and its subregion's start and end.
    let ((along, start, end), (beside, first, last)) = if across { ((x, l, r), (y, t, b)) } else { ((y, t, b), (x, l, r)) };
    let (before, after) = (along > start, along + 1 < end);
    if !(before || after) {
        return 0.0;
    }
    let [from, to] = [if before { along - 1 } else { along }, if after { along + 1 } else { along }];
    let at = |along: usize, beside: usize| if across { height(along, beside) } else { height(beside, along) };
    let (mut sum, mut weights) = (0.0, 0.0);
    for (side, weight) in [(beside.wrapping_sub(1), 1.0), (beside, 2.0), (beside + 1, 1.0)] {
        if (first..last).contains(&side) {
            sum += weight * (at(to, side) - at(from, side));
            weights += weight;
        }
    }
    // A difference over one pixel, not two, counts twice.
    sum * if before && after { 1.0 } else { 2.0 } / weights
}

/// An image convolved with a kernel within its subregion (see `Convolution`): on premultiplied
/// colours, or on plain ones with alpha kept.
fn convolved(picture: Picture, convolution: &Convolution, part: [usize; 4], wide: usize) -> Picture {
    let source = clear_outside(picture, part, wide);
    let mut pixels = vec![0; source.pixels.len()];
    for y in part[1]..part[3] {
        for x in part[0]..part[2] {
            pixels[y * wide + x] = convolved_pixel(&source.pixels, (wide, part), [x, y], convolution);
        }
    }
    Picture { pixels, ..source }
}

/// One pixel of a convolution (see `convolved`).
fn convolved_pixel(pixels: &[u32], (wide, [l, t, r, b]): (usize, [usize; 4]), [x, y]: [usize; 2], convolution: &Convolution) -> u32 {
    let ([columns, rows], [tx, ty]) = (convolution.order, convolution.target);
    // A pixel at a place that may lie past the subregion's edges, as the edge mode takes it.
    let taken = |at: i64, low: usize, high: usize| match convolution.edge {
        Edge::Duplicate => Some(at.clamp(low as i64, high as i64 - 1) as usize),
        Edge::Wrap => Some(low + (at - low as i64).rem_euclid((high - low) as i64) as usize),
        Edge::None => (low as i64..high as i64).contains(&at).then_some(at as usize),
    };
    let plain = |pixel: u32| {
        let [a, r, g, b] = four(pixel).map(|part| part as f32 / 255.0);
        match convolution.keep_alpha {
            true if a > 0.0 => [a, r / a, g / a, b / a],
            true => [a, 0.0, 0.0, 0.0],
            false => [a, r, g, b],
        }
    };
    let mut sum = [0.0f32; 4];
    for i in 0..rows {
        for j in 0..columns {
            let (Some(sx), Some(sy)) = (taken(x as i64 - tx as i64 + j as i64, l, r), taken(y as i64 - ty as i64 + i as i64, t, b)) else { continue };
            let weight = convolution.kernel[(rows - 1 - i) * columns + columns - 1 - j];
            let parts = plain(pixels[sy * wide + sx]);
            sum.iter_mut().zip(parts).for_each(|(sum, part)| *sum += part * weight);
        }
    }
    let level = |part: f32| (part * 255.0 + 0.5) as u32;
    // (Values beyond measure can sum to no number: nothing then.)
    let [a, r, g, b] = sum.map(|sum| sum / convolution.divisor).map(|part| if part.is_nan() { 0.0 } else { part });
    if convolution.keep_alpha {
        let [r, g, b] = [r, g, b].map(|c| (c + convolution.bias).clamp(0.0, 1.0));
        let a = (pixels[y * wide + x] >> 24) as f32 / 255.0;
        return pixel([a, r * a, g * a, b * a].map(level));
    }
    // The bias adds to colours as much as they are solid, keeping them premultiplied.
    let a = (a + convolution.bias).clamp(0.0, 1.0);
    pixel([a, (r + convolution.bias * a).clamp(0.0, a), (g + convolution.bias * a).clamp(0.0, a), (b + convolution.bias * a).clamp(0.0, a)].map(level))
}

/// An image whose pixels are taken from elsewhere, as a map's channels say: each from `scale`
/// times its two channels (across, down; each less a half) away, between pixels blended from
/// the four nearest; nothing from outside the subregion. The map's colours are taken plain, not
/// premultiplied.
fn displaced(moved: Picture, map: &Picture, part: [usize; 4], wide: usize, scale: [f32; 2], channels: [usize; 2]) -> Picture {
    let mut pixels = vec![0; moved.pixels.len()];
    let [l, t, r, b] = part;
    let at = |x: f32, y: f32| match x >= l as f32 && x < r as f32 && y >= t as f32 && y < b as f32 {
        true => moved.pixels[y as usize * wide + x as usize],
        false => 0,
    };
    for y in t..b {
        for x in l..r {
            let [a, red, green, blue] = four(map.pixels[y * wide + x]);
            let plain = |c: u32| if a == 0 { 0.0 } else { (c * 255 + a / 2) as f32 / a as f32 / 255.0 };
            let values = [plain(red), plain(green), plain(blue), a as f32 / 255.0];
            let [sx, sy] = [x as f32 + scale[0] * (values[channels[0]] - 0.5), y as f32 + scale[1] * (values[channels[1]] - 0.5)];
            let ([x0, y0], [fx, fy]) = ([sx.floor(), sy.floor()], [sx - sx.floor(), sy - sy.floor()]);
            let weights = [(1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy];
            pixels[y * wide + x] = mixed([at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0)], weights);
        }
    }
    Picture { pixels, ..moved }
}

/// For each pixel of a resampled line, the pixels of the line it takes and their weights.
type Taps = Vec<Vec<(usize, f32)>>;

/// How each of `count` new pixels takes those of a line `long` pixels (see `resampled`), the
/// nth covering `span(n)` of it (from, to, in its pixels): covering a pixel or more, the mean of
/// those it covers (by how much of each), else the two nearest pixel middles blended (as cairo
/// resamples). Nothing from outside the line.
fn taps(count: usize, long: usize, span: impl Fn(usize) -> (f64, f64)) -> Taps {
    (0..count)
        .map(|n| {
            let (from, to) = span(n);
            if to - from >= 1.0 { covered(from, to, long) } else { nearest((from + to) / 2.0 - 0.5, long) }
        })
        .collect()
}

/// The pixels of a line `long` pixels that `from` to `to` covers, each weighed by how much of
/// the span it is.
fn covered(from: f64, to: f64, long: usize) -> Vec<(usize, f32)> {
    let (first, last) = (from.floor().max(0.0) as usize, to.ceil().clamp(0.0, long as f64) as usize);
    (first..last).map(|at| (at, ((to.min(at as f64 + 1.0) - from.max(at as f64)) / (to - from)) as f32)).filter(|(_, weight)| *weight > 0.0).collect()
}

/// The two pixels of a line `long` pixels around place `at` (pixel middles at whole numbers),
/// each weighed by how near.
fn nearest(at: f64, long: usize) -> Vec<(usize, f32)> {
    let low = at.floor();
    let share = (at - low) as f32;
    let inside = |(at, weight): &(f64, f32)| *at >= 0.0 && *at < long as f64 && *weight > 0.0;
    [(low, 1.0 - share), (low + 1.0, share)].into_iter().filter(inside).map(|(at, weight)| (at as usize, weight)).collect()
}

/// A picture `wide` pixels per row resampled: each new column and row from the old ones as
/// `taps` say (across, then down), channel by channel on premultiplied colours.
fn resampled(pixels: &[u32], wide: usize, [across, down]: &[Taps; 2]) -> Vec<u32> {
    let high = pixels.len() / wide.max(1);
    let mix = |taken: &mut dyn Iterator<Item = ([f32; 4], f32)>| taken.fold([0.0f32; 4], |sum, (parts, weight)| std::array::from_fn(|n| sum[n] + parts[n] * weight));
    let mut rows: Vec<[f32; 4]> = Vec::with_capacity(across.len() * high);
    for row in pixels.chunks_exact(wide.max(1)) {
        rows.extend(across.iter().map(|taps| mix(&mut taps.iter().map(|(at, weight)| (four(row[*at]).map(|part| part as f32), *weight)))));
    }
    let mut out = Vec::with_capacity(across.len() * down.len());
    for taps in down {
        for x in 0..across.len() {
            let [a, r, g, b] = mix(&mut taps.iter().map(|(y, weight)| (rows[y * across.len() + x], *weight))).map(|part| (part + 0.5).clamp(0.0, 255.0) as u32);
            out.push(pixel([a, r.min(a), g.min(a), b.min(a)]));
        }
    }
    out
}

/// An affine transform (see `svg::Table`) in double precision.
type Exact = [f64; 6];

/// A transform in double precision.
fn exact(table: &Table) -> Exact {
    table.map(f64::from)
}

/// One transform equal to `inner`, then `outer`.
fn both(outer: &Exact, inner: &Exact) -> Exact {
    let ([a, b, c, d, e, f], [p, q, r, s, t, u]) = (*outer, *inner);
    [a * p + c * q, b * p + d * q, a * r + c * s, b * r + d * s, a * t + c * u + e, b * t + d * u + f]
}

/// A point through a transform.
fn through(table: &Exact, [x, y]: [f64; 2]) -> [f64; 2] {
    [table[0] * x + table[2] * y + table[4], table[1] * x + table[3] * y + table[5]]
}

/// The inverse transform; `None` if it collapses everything onto a line.
fn undone(table: &Exact) -> Option<Exact> {
    let [a, b, c, d, e, f] = *table;
    let area = a * d - b * c;
    area.is_normal().then(|| [d / area, -b / area, -c / area, a / area, (c * f - d * e) / area, (b * e - a * f) / area])
}

/// The bounding box of a box through a transform (left, top, right, bottom).
fn reach(table: &Exact, [l, t, r, b]: [f64; 4]) -> [f64; 4] {
    let corners = [[l, t], [r, t], [l, b], [r, b]].map(|corner| through(table, corner));
    let low = corners.iter().fold([f64::MAX; 2], |low, p| [low[0].min(p[0]), low[1].min(p[1])]);
    let high = corners.iter().fold([f64::MIN; 2], |high, p| [high[0].max(p[0]), high[1].max(p[1])]);
    [low[0], low[1], high[0], high[1]]
}

/// An image with everything outside a subregion cleared.
fn clear_outside(mut picture: Picture, [l, t, r, b]: [usize; 4], wide: usize) -> Picture {
    for (row, line) in picture.pixels.chunks_exact_mut(wide.max(1)).enumerate() {
        match (t..b).contains(&row) && l < r {
            true => {
                line[..l].fill(0);
                line[r..].fill(0);
            }
            false => line.fill(0),
        }
    }
    picture.part = [l, t, r.max(l), b.max(t)];
    picture
}

/// Tables converting a colour (not premultiplied) from sRGB to linear light and back, to the
/// nearest of 256 levels, as librsvg's do.
struct Light {
    linear: [u8; 256],
    screen: [u8; 256],
    /// For each alpha, 2^32 divided by it, rounded up: un-premultiplying is then a multiply and
    /// a shift, which for these values equals the division.
    plain: [u64; 256],
}

impl Light {
    fn new() -> Light {
        let into = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let back = |c: f32| if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        let level = |c: f32| (c * 255.0 + 0.5) as u8;
        let plain = std::array::from_fn(|a| if a == 0 { 0 } else { (1u64 << 32).div_ceil(a as u64) });
        Light { linear: std::array::from_fn(|n| level(into(n as f32 / 255.0))), screen: std::array::from_fn(|n| level(back(n as f32 / 255.0))), plain }
    }

    /// An image in the asked colour space (linear, or sRGB).
    fn taken(&self, mut picture: Picture, linear: bool) -> Picture {
        if picture.linear == linear {
            return picture;
        }
        let table = if linear { &self.linear } else { &self.screen };
        for pixel in &mut picture.pixels {
            *pixel = changed(*pixel, table, &self.plain);
        }
        picture.linear = linear;
        picture
    }
}

/// A canvas pixel with its colour through a table (on the un-premultiplied colour, rounded as
/// librsvg does).
fn changed(pixel: u32, table: &[u8; 256], plain: &[u64; 256]) -> u32 {
    let a = pixel >> 24;
    if a == 0 {
        return 0;
    }
    let by = plain[a as usize];
    let part = |shift: u32| {
        let plain = ((((pixel >> shift & 255) * 255 + a / 2) as u64 * by) >> 32) as u32;
        (table[plain.min(255) as usize] as u32 * a + 127) / 255
    };
    a << 24 | part(16) << 16 | part(8) << 8 | part(0)
}

/// A pixel's channels: alpha, red, green, blue.
fn four(pixel: u32) -> [u32; 4] {
    [pixel >> 24, pixel >> 16 & 255, pixel >> 8 & 255, pixel & 255]
}

/// A pixel from its channels.
fn pixel([a, r, g, b]: [u32; 4]) -> u32 {
    a << 24 | r << 16 | g << 8 | b
}

/// `a` times `b`, each out of 255, rounded as pixman does.
fn times(a: u32, b: u32) -> u32 {
    let t = a * b + 128;
    ((t >> 8) + t) >> 8
}

/// A sum of products of levels (out of 255²), as a level (rounded as `times`).
fn shared(sum: u32) -> u32 {
    let t = sum + 128;
    (((t >> 8) + t) >> 8).min(255)
}

/// One pixel over another.
fn over(top: u32, under: u32) -> u32 {
    let (rest, [ua, ur, ug, ub], [ta, tr, tg, tb]) = (255 - (top >> 24), four(under), four(top));
    pixel([ta + times(ua, rest), tr + times(ur, rest), tg + times(ug, rest), tb + times(ub, rest)].map(|part| part.min(255)))
}

/// An image blurred within its subregion by deviations in pixels, across and down.
fn blurred(picture: Picture, part: [usize; 4], wide: usize, [across, down]: [f32; 2]) -> Picture {
    let mut picture = clear_outside(picture, part, wide);
    // A deviation of zero or less is no blur.
    if !(across > 0.0 || down > 0.0) {
        return picture;
    }
    let [l, t, r, b] = part;
    if l >= r || t >= b {
        return picture;
    }
    match across {
        _ if across >= 2.0 => boxes(&mut picture.pixels, wide, part, across, true),
        _ if across > 0.0 => bell(&mut picture.pixels, wide, part, across, true),
        _ => {}
    }
    match down {
        _ if down >= 2.0 => boxes(&mut picture.pixels, wide, part, down, false),
        _ if down > 0.0 => bell(&mut picture.pixels, wide, part, down, false),
        _ => {}
    }
    picture
}

/// Blurs a subregion one way (across, else down) by three box blurs approximating a Gaussian of
/// the deviation: three of size `d` if that is odd; else two of size `d`, the first shifted
/// back and the second forward, and one of size `d + 1`.
fn boxes(pixels: &mut [u32], wide: usize, part: [usize; 4], deviation: f32, across: bool) {
    let d = (deviation * 3.0 * std::f32::consts::TAU.sqrt() / 4.0 + 0.5).floor() as usize;
    let sizes = match d % 2 {
        _ if d == 0 => [(1, 0); 3],
        1 => [(d, d / 2); 3],
        _ => [(d, d / 2), (d, d / 2 - 1), (d + 1, d / 2)],
    };
    for (size, before) in sizes {
        match across {
            true => box_across(pixels, wide, part, size, before),
            false => box_down(pixels, wide, part, size, before),
        }
    }
}

/// One horizontal box blur: each pixel of each subregion row becomes the mean of `size` pixels
/// of its row, starting `before` before it (outside the subregion is transparent).
fn box_across(pixels: &mut [u32], wide: usize, [l, t, r, b]: [usize; 4], size: usize, before: usize) {
    let (mut row, share) = (vec![0u32; r - l], 1.0 / size as f32);
    for y in t..b {
        let line = &mut pixels[y * wide + l..y * wide + r];
        row.copy_from_slice(line);
        box_line(&row, line, size, before, share);
    }
}

/// One box blur along a line of pixels: see `box_across`.
fn box_line(row: &[u32], line: &mut [u32], size: usize, before: usize, share: f32) {
    let after = size - before;
    let mut sum = Sum::default();
    row.iter().take(after).for_each(|pixel| sum.add(*pixel));
    line[0] = sum.mean(share);
    for x in 1..row.len() {
        if x > before {
            sum.take(row[x - before - 1]);
        }
        if x + after - 1 < row.len() {
            sum.add(row[x + after - 1]);
        }
        line[x] = sum.mean(share);
    }
}

/// One vertical box blur: as `box_across`, a whole row at a time.
fn box_down(pixels: &mut [u32], wide: usize, [l, t, r, b]: [usize; 4], size: usize, before: usize) {
    let (after, share, high) = (size - before, 1.0 / size as f32, b - t);
    let taken: Vec<u32> = (t..b).flat_map(|y| pixels[y * wide + l..y * wide + r].iter().copied()).collect();
    let (across, mut sums) = (r - l, vec![Sum::default(); r - l]);
    let row = |y: usize| &taken[y * across..(y + 1) * across];
    (0..after.min(high)).for_each(|y| sums.iter_mut().zip(row(y)).for_each(|(sum, pixel)| sum.add(*pixel)));
    for y in 0..high {
        if y > before {
            sums.iter_mut().zip(row(y - before - 1)).for_each(|(sum, pixel)| sum.take(*pixel));
        }
        if y > 0 && y + after - 1 < high {
            sums.iter_mut().zip(row(y + after - 1)).for_each(|(sum, pixel)| sum.add(*pixel));
        }
        let line = &mut pixels[(t + y) * wide + l..(t + y) * wide + r];
        line.iter_mut().zip(&sums).for_each(|(pixel, sum)| *pixel = sum.mean(share));
    }
}

/// A running sum of pixels: alpha, red, green, blue. (Written out: a loop over four stays a
/// loop in a size-optimized build.)
#[derive(Clone, Copy, Default)]
struct Sum {
    a: u32,
    r: u32,
    g: u32,
    b: u32,
}

impl Sum {
    /// Adds a pixel.
    #[inline(always)]
    fn add(&mut self, pixel: u32) {
        (self.a, self.r, self.g, self.b) = (self.a + (pixel >> 24), self.r + (pixel >> 16 & 255), self.g + (pixel >> 8 & 255), self.b + (pixel & 255));
    }

    /// Removes an added pixel.
    #[inline(always)]
    fn take(&mut self, pixel: u32) {
        (self.a, self.r, self.g, self.b) = (self.a - (pixel >> 24), self.r - (pixel >> 16 & 255), self.g - (pixel >> 8 & 255), self.b - (pixel & 255));
    }

    /// The sum times `share`, rounded to the nearest level.
    #[inline(always)]
    fn mean(&self, share: f32) -> u32 {
        let level = |sum: u32| (sum as f32 * share + 0.5) as u32;
        level(self.a) << 24 | level(self.r) << 16 | level(self.g) << 8 | level(self.b)
    }
}

/// Adds a pixel times `weight` to a channel sum (alpha, red, green, blue).
#[inline(always)]
fn weigh(sum: &mut [f32; 4], pixel: u32, weight: f32) {
    sum[0] += (pixel >> 24) as f32 * weight;
    sum[1] += (pixel >> 16 & 255) as f32 * weight;
    sum[2] += (pixel >> 8 & 255) as f32 * weight;
    sum[3] += (pixel & 255) as f32 * weight;
}

/// Blurs a subregion one way (across, else down) with the Gaussian itself: each pixel takes of
/// each neighbour the curve's area over it.
fn bell(pixels: &mut [u32], wide: usize, [l, t, r, b]: [usize; 4], deviation: f32, across: bool) {
    let weights = curve(deviation);
    let reach = weights.len() / 2;
    let taken: Vec<u32> = (t..b).flat_map(|y| pixels[y * wide + l..y * wide + r].iter().copied()).collect();
    let (columns, rows) = (r - l, b - t);
    let (steps, count, stride) = if across { (1, columns, columns) } else { (columns, rows, 1) };
    let lines = if across { rows } else { columns };
    for line in 0..lines {
        let first = line * stride;
        for at in 0..count {
            let mut sum = [0.0f32; 4];
            (at.saturating_sub(reach)..(at + reach + 1).min(count)).for_each(|n| weigh(&mut sum, taken[first + n * steps], weights[n + reach - at]));
            let (x, y) = if across { (at, line) } else { (line, at) };
            let level = |part: f32| (part.clamp(0.0, 255.0) + 0.5) as u32;
            pixels[(t + y) * wide + l + x] = level(sum[0]) << 24 | level(sum[1]) << 16 | level(sum[2]) << 8 | level(sum[3]);
        }
    }
}

/// The Gaussian's area over each pixel for a deviation (in pixels), out to three deviations
/// each way, the centre pixel in the middle.
fn curve(deviation: f32) -> Vec<f32> {
    let reach = (deviation * 3.0).ceil().max(1.0) as i32;
    let below = |x: f32| 0.5 * (1.0 + erf(x / (deviation * std::f32::consts::SQRT_2)));
    let weights: Vec<f32> = (-reach..=reach).map(|n| below(n as f32 + 0.5) - below(n as f32 - 0.5)).collect();
    let sum: f32 = weights.iter().sum();
    weights.into_iter().map(|weight| weight / sum).collect()
}

/// The error function, within 2×10⁻⁷ (Abramowitz and Stegun 7.1.26).
fn erf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_6 + t * (-0.284_496_74 + t * (1.421_413_7 + t * (-1.453_152_1 + t * 1.061_405_4))));
    (1.0 - poly * (-x * x).exp()).copysign(x)
}

/// An image offset by pixels, across and down: for fractional offsets, each pixel bilinear from
/// the four it now covers. The result stays within its subregion (`exact` as given) and is
/// clipped there: an edge pixel of either keeps as much as lies inside (as librsvg's clip
/// does).
fn moved(picture: Picture, (part, exact): ([usize; 4], [f64; 4]), wide: usize, [dx, dy]: [f32; 2]) -> Picture {
    let source = clear_outside(picture, part, wide);
    let high = source.pixels.len() / wide.max(1);
    let (whole, rest) = ([dx.floor(), dy.floor()], [dx - dx.floor(), dy - dy.floor()]);
    if !(whole[0].abs() < 1e7 && whole[1].abs() < 1e7) {
        return Picture { pixels: vec![0; source.pixels.len()], ..source };
    }
    let [wx, wy] = [whole[0] as i64, whole[1] as i64];
    let at = |x: i64, y: i64| match x >= 0 && y >= 0 && (x as usize) < wide && (y as usize) < high {
        true => source.pixels[y as usize * wide + x as usize],
        false => 0,
    };
    let weights = [(1.0 - rest[0]) * (1.0 - rest[1]), rest[0] * (1.0 - rest[1]), (1.0 - rest[0]) * rest[1], rest[0] * rest[1]];
    let [x, y] = [f64::from(dx), f64::from(dy)];
    let kept = [exact[0].max(exact[0] + x), exact[1].max(exact[1] + y), exact[2].min(exact[2] + x), exact[3].min(exact[3] + y)];
    let covered = |at: usize, low: f64, high: f64| (high.min(at as f64 + 1.0) - low.max(at as f64)).clamp(0.0, 1.0);
    let mut pixels = vec![0; source.pixels.len()];
    let whole_move = rest == [0.0, 0.0];
    for y in part[1]..part[3] {
        let down = covered(y, kept[1], kept[3]);
        for x in part[0]..part[2] {
            let (x0, y0) = (x as i64 - wx - 1, y as i64 - wy - 1);
            let moved = if whole_move { at(x0 + 1, y0 + 1) } else { mixed([at(x0 + 1, y0 + 1), at(x0, y0 + 1), at(x0 + 1, y0), at(x0, y0)], weights) };
            let share = down * covered(x, kept[0], kept[2]);
            pixels[y * wide + x] = if share >= 1.0 { moved } else { as_much(moved, (share * 255.0 + 0.5) as u32) };
        }
    }
    Picture { pixels, ..source }
}

/// A pixel at `part`/255 of its alpha (colour with it).
fn as_much(was: u32, part: u32) -> u32 {
    pixel(four(was).map(|c| times(c, part)))
}

/// Four pixels weighted.
fn mixed(near: [u32; 4], weights: [f32; 4]) -> u32 {
    let part = |shift: u32| (near.iter().zip(weights).map(|(pixel, weight)| (pixel >> shift & 255) as f32 * weight).sum::<f32>() + 0.5).min(255.0) as u32;
    part(24) << 24 | part(16) << 16 | part(8) << 8 | part(0)
}

/// A subregion flooded with one colour (not premultiplied); the image in sRGB.
fn flooded(color: Color, [l, t, r, b]: [usize; 4], wide: usize, high: usize) -> Picture {
    let [a, red, green, blue] = four(color.0);
    let premultiplied = pixel([a, times(red, a), times(green, a), times(blue, a)]);
    let mut pixels = vec![0; wide * high];
    for y in t..b {
        pixels[y * wide + l..y * wide + r].fill(premultiplied);
    }
    Picture { pixels, linear: false, part: [l, t, r, b] }
}

/// Images composited in order, the first at the bottom.
fn merged(pictures: impl Iterator<Item = Picture>, part: [usize; 4], wide: usize, high: usize, wanted: Option<bool>) -> Picture {
    let mut merged = Picture { pixels: vec![0; wide * high], linear: wanted.unwrap_or(false), part };
    for picture in pictures {
        merged.linear = wanted.unwrap_or(picture.linear);
        merged.pixels.iter_mut().zip(&picture.pixels).for_each(|(under, top)| *under = over(*top, *under));
    }
    merged
}

/// The first image onto the second by an operator.
fn composited(first: Picture, second: &Picture, operator: Operator, part: [usize; 4], wide: usize) -> Picture {
    let mut picture = clear_outside(first, part, wide);
    let second = &second.pixels;
    for (top, under) in picture.pixels.iter_mut().zip(second) {
        let ([ta, ..], [ua, ..]) = (four(*top), four(*under));
        *top = match operator {
            Operator::Over => over(*top, *under),
            Operator::In => pixel(four(*top).map(|part| times(part, ua))),
            Operator::Out => pixel(four(*top).map(|part| times(part, 255 - ua))),
            Operator::Atop => pixel([0, 1, 2, 3].map(|n| shared(four(*top)[n] * ua + four(*under)[n] * (255 - ta)))),
            Operator::Xor => pixel([0, 1, 2, 3].map(|n| shared(four(*top)[n] * (255 - ua) + four(*under)[n] * (255 - ta)))),
            Operator::Lighter => pixel([0, 1, 2, 3].map(|n| (four(*top)[n] + four(*under)[n]).min(255))),
            Operator::Arithmetic(k) => arithmetic(*top, *under, k),
        };
    }
    clear_outside(picture, part, wide)
}

/// Two pixels as `k1 a b + k2 a + k3 b + k4` per channel (colour clamped to alpha).
fn arithmetic(first: u32, second: u32, [k1, k2, k3, k4]: [f32; 4]) -> u32 {
    let ([a, ..], [b, ..]) = (four(first), four(second));
    let of = |a: u32, b: u32| {
        let (a, b) = (a as f32 / 255.0, b as f32 / 255.0);
        (k1 * a * b + k2 * a + k3 * b + k4).clamp(0.0, 1.0)
    };
    let solid = of(a, b);
    if solid <= 0.0 {
        return 0;
    }
    let ([_, r1, g1, b1], [_, r2, g2, b2]) = (four(first), four(second));
    let level = |part: f32| (part.min(solid) * 255.0 + 0.5) as u32;
    pixel([level(solid), level(of(r1, r2)), level(of(g1, g2)), level(of(b1, b2))])
}

/// The first image over the second, blended by a mode, as Compositing and Blending defines it.
fn blended(first: Picture, second: &Picture, mode: Mode, part: [usize; 4], wide: usize) -> Picture {
    let mut picture = clear_outside(first, part, wide);
    for (top, under) in picture.pixels.iter_mut().zip(&second.pixels) {
        *top = match mode {
            Mode::Normal => over(*top, *under),
            _ => mix(*top, *under, mode),
        };
    }
    clear_outside(picture, part, wide)
}

/// One pixel over another, blended by a non-normal `mode`.
fn mix(top: u32, under: u32, mode: Mode) -> u32 {
    let parts = |pixel: u32| four(pixel).map(|part| part as f32 / 255.0);
    let ([sa, sr, sg, sb], [da, dr, dg, db]) = (parts(top), parts(under));
    // Un-premultiplied colours.
    let plain = |c: f32, a: f32| if a > 0.0 { (c / a).min(1.0) } else { 0.0 };
    let (source, backdrop) = ([plain(sr, sa), plain(sg, sa), plain(sb, sa)], [plain(dr, da), plain(dg, da), plain(db, da)]);
    let mixed = match mode {
        Mode::Hue | Mode::Saturation | Mode::Color | Mode::Luminosity => apart(source, backdrop, mode),
        _ => [0, 1, 2].map(|n| each(source[n], backdrop[n], mode)),
    };
    let solid = sa + da - sa * da;
    let level = |part: f32| (part.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let color = |n: usize, s: f32, d: f32| level((1.0 - sa) * d + (1.0 - da) * s + sa * da * mixed[n]);
    pixel([level(solid), color(0, sr, dr), color(1, sg, dg), color(2, sb, db)])
}

/// One channel of a separable mode: the top's and the backdrop's.
fn each(s: f32, d: f32, mode: Mode) -> f32 {
    match mode {
        Mode::Multiply => s * d,
        Mode::Screen => s + d - s * d,
        Mode::Overlay => hard_light(d, s),
        Mode::Darken => s.min(d),
        Mode::Lighten => s.max(d),
        Mode::ColorDodge if d == 0.0 => 0.0,
        Mode::ColorDodge if s >= 1.0 => 1.0,
        Mode::ColorDodge => (d / (1.0 - s)).min(1.0),
        Mode::ColorBurn if d >= 1.0 => 1.0,
        Mode::ColorBurn if s <= 0.0 => 0.0,
        Mode::ColorBurn => 1.0 - ((1.0 - d) / s).min(1.0),
        Mode::HardLight => hard_light(s, d),
        Mode::SoftLight if s <= 0.5 => d - (1.0 - 2.0 * s) * d * (1.0 - d),
        Mode::SoftLight => {
            let lifted = if d <= 0.25 { ((16.0 * d - 12.0) * d + 4.0) * d } else { d.sqrt() };
            d + (2.0 * s - 1.0) * (lifted - d)
        }
        Mode::Difference => (s - d).abs(),
        Mode::Exclusion => s + d - 2.0 * s * d,
        _ => s,
    }
}

/// Hard light: multiply where the top is dark, screen where it is light.
fn hard_light(s: f32, d: f32) -> f32 {
    match s <= 0.5 {
        true => d * 2.0 * s,
        false => {
            let s = 2.0 * s - 1.0;
            s + d - s * d
        }
    }
}

/// The colour of a non-separable mode: the hue, saturation or luminosity of one with the rest
/// of the other, as the spec's `SetLum` and `SetSat`.
fn apart(source: [f32; 3], backdrop: [f32; 3], mode: Mode) -> [f32; 3] {
    let lum = |c: [f32; 3]| 0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2];
    let sat = |c: [f32; 3]| c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2]);
    let with_sat = |c: [f32; 3], s: f32| {
        let (low, high) = (c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
        c.map(|part| if high > low { (part - low) * s / (high - low) } else { 0.0 })
    };
    let with_lum = |c: [f32; 3], l: f32| {
        let d = l - lum(c);
        let c = c.map(|part| part + d);
        let (l, low, high) = (lum(c), c[0].min(c[1]).min(c[2]), c[0].max(c[1]).max(c[2]));
        let c = if low < 0.0 { c.map(|part| l + (part - l) * l / (l - low)) } else { c };
        if high > 1.0 { c.map(|part| l + (part - l) * (1.0 - l) / (high - l)) } else { c }
    };
    match mode {
        Mode::Hue => with_lum(with_sat(source, sat(backdrop)), lum(backdrop)),
        Mode::Saturation => with_lum(with_sat(backdrop, sat(source)), lum(backdrop)),
        Mode::Color => with_lum(source, lum(backdrop)),
        _ => with_lum(backdrop, lum(source)),
    }
}

/// An image with each channel recomputed from all four (not premultiplied) by a 4×5 matrix.
/// Where alpha is zero, the colour is taken as transparent black.
fn matrixed(picture: Picture, matrix: &[f32; 20], part: [usize; 4], wide: usize) -> Picture {
    let mut picture = clear_outside(picture, part, wide);
    let [l, t, r, b] = part;
    for y in t..b {
        for pixel in &mut picture.pixels[y * wide + l..y * wide + r] {
            *pixel = matrix_pixel(*pixel, matrix);
        }
    }
    picture
}

/// One pixel through a colour matrix (see `matrixed`), rounded as librsvg does.
fn matrix_pixel(was: u32, m: &[f32; 20]) -> u32 {
    let [a, r, g, b] = four(was).map(|part| part as f32 / 255.0);
    let plain = if a > 0.0 { [r / a, g / a, b / a, a, 1.0] } else { [0.0, 0.0, 0.0, 0.0, 1.0] };
    let row = |n: usize| (0..5).map(|k| m[n * 5 + k] * plain[k]).sum::<f32>();
    let solid = row(3).clamp(0.0, 1.0);
    let level = |part: f32| (part.clamp(0.0, 1.0) * solid * 255.0 + 0.5) as u32;
    pixel([level(1.0), level(row(0)), level(row(1)), level(row(2))])
}

/// An image with each channel through a function (red, green, blue, alpha).
fn transferred(picture: Picture, functions: &[Function; 4], part: [usize; 4], wide: usize) -> Picture {
    let mut picture = clear_outside(picture, part, wide);
    // Each function is tabulated once for the 256 levels.
    let tables: Vec<[u8; 256]> = functions.iter().map(|function| std::array::from_fn(|n| (function.of(n as f32 / 255.0).clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let [l, t, r, b] = part;
    for y in t..b {
        for pixel in &mut picture.pixels[y * wide + l..y * wide + r] {
            *pixel = transfer_pixel(*pixel, &tables);
        }
    }
    picture
}

/// One pixel through four channel tables (see `transferred`).
fn transfer_pixel(was: u32, tables: &[[u8; 256]]) -> u32 {
    let [a, r, g, b] = four(was);
    let plain = |c: u32| (c * 255 + a / 2).checked_div(a).map_or(0, |plain| plain.min(255));
    let solid = tables[3][a as usize] as u32;
    let color = |n: usize, c: u32| (tables[n][plain(c) as usize] as u32 * solid + 127) / 255;
    pixel([solid, color(0, r), color(1, g), color(2, b)])
}

/// An image with a drop shadow: its alpha in black, blurred, offset, coloured.
fn shadowed(picture: Picture, (part, exact): ([usize; 4], [f64; 4]), wide: usize, by: [f32; 2], deviation: [f32; 2], color: Color) -> Picture {
    let alpha = Picture { pixels: picture.pixels.iter().map(|pixel| pixel & 0xff00_0000).collect(), ..picture.clone() };
    let shadow = moved(blurred(alpha, part, wide, deviation), (part, exact), wide, by);
    let [a, red, green, blue] = four(color.0);
    let colored = |cover: u32| {
        let solid = times(cover >> 24, a);
        pixel([solid, times(red, solid), times(green, solid), times(blue, solid)])
    };
    let mut pixels: Vec<u32> = shadow.pixels.iter().map(|cover| colored(*cover)).collect();
    pixels.iter_mut().zip(&picture.pixels).for_each(|(under, top)| *under = over(*top, *under));
    clear_outside(Picture { pixels, ..picture }, part, wide)
}

/// An image dilated (each channel the maximum within the radius) or eroded (the minimum, zero
/// near its edges), by radii in pixels across and down. Radii of zero or less both ways leave
/// it as is.
fn thickened(picture: Picture, part: [usize; 4], wide: usize, thicker: bool, radius: [f32; 2]) -> Picture {
    let mut picture = clear_outside(picture, part, wide);
    if !(radius[0] > 0.0 || radius[1] > 0.0) {
        return picture;
    }
    let [l, t, r, b] = part;
    // A fractional radius reaches into the pixel it ends in, as librsvg takes it.
    let [across, down] = radius.map(|radius| if radius > 0.0 { radius.min(1e7).ceil() as usize } else { 0 });
    let (mut line, mut out, mut room) = (Vec::new(), Vec::new(), [Vec::new(), Vec::new()]);
    for y in t..b {
        line.clear();
        line.extend_from_slice(&picture.pixels[y * wide + l..y * wide + r]);
        extremes(&line, &mut picture.pixels[y * wide + l..y * wide + r], across, thicker, &mut room);
    }
    for x in l..r {
        line.clear();
        line.extend((t..b).map(|y| picture.pixels[y * wide + x]));
        out.clone_from(&line);
        extremes(&line, &mut out, down, thicker, &mut room);
        out.iter().enumerate().for_each(|(n, pixel)| picture.pixels[(t + n) * wide + x] = *pixel);
    }
    picture
}

/// The per-channel maximum of two pixels.
#[inline(always)]
fn most_of(a: u32, b: u32) -> u32 {
    (a >> 24).max(b >> 24) << 24 | (a >> 16 & 255).max(b >> 16 & 255) << 16 | (a >> 8 & 255).max(b >> 8 & 255) << 8 | (a & 255).max(b & 255)
}

/// The per-channel minimum of two pixels.
#[inline(always)]
fn least_of(a: u32, b: u32) -> u32 {
    (a >> 24).min(b >> 24) << 24 | (a >> 16 & 255).min(b >> 16 & 255) << 16 | (a >> 8 & 255).min(b >> 8 & 255) << 8 | (a & 255).min(b & 255)
}

/// Each pixel of a line the maximum (or minimum) per channel within `radius`, by van Herk /
/// Gil-Werman: the line in blocks of the window's length, with running extremes from each
/// block's start and from its end, of which a window combines the two it overlaps. A window
/// reaching past the line has minimum zero. (`room` holds the two runs.)
fn extremes(line: &[u32], out: &mut [u32], radius: usize, most: bool, [from_start, to_end]: &mut [Vec<u32>; 2]) {
    let (n, size) = (line.len(), radius.saturating_mul(2).saturating_add(1));
    let pick = |a: u32, b: u32| if most { most_of(a, b) } else { least_of(a, b) };
    if n == 0 {
        return;
    }
    from_start.clear();
    from_start.extend_from_slice(line);
    to_end.clone_from(from_start);
    // Each pixel's position in its block, counted as the runs go: no division per pixel.
    let mut place = 0;
    for at in 1..n {
        place = if place + 1 == size { 0 } else { place + 1 };
        if place != 0 {
            from_start[at] = pick(from_start[at - 1], line[at]);
        }
    }
    let mut place = (n - 1) % size;
    for at in (0..n.saturating_sub(1)).rev() {
        if place != 0 {
            to_end[at] = pick(to_end[at + 1], line[at]);
        }
        place = if place == 0 { size - 1 } else { place - 1 };
    }
    // The window ends' positions in their blocks, advanced as the window moves.
    let mut ends = [0, radius.min(n - 1) % size];
    for (at, out) in out.iter_mut().enumerate() {
        let (low, high) = (at.saturating_sub(radius), at.saturating_add(radius).min(n - 1));
        *out = match (ends[1] >= ends[0], ends[0] == 0) {
            _ if !most && (at < radius || at + radius >= n) => 0,
            (true, true) => from_start[high],
            (true, false) => to_end[low],
            (false, _) => pick(to_end[low], from_start[high]),
        };
        let next = |end: usize| if end + 1 == size { 0 } else { end + 1 };
        ends = [if at >= radius { next(ends[0]) } else { ends[0] }, if at + radius + 1 < n { next(ends[1]) } else { ends[1] }];
    }
}

/// An image's subregion tiled edge to edge over a subregion of the room.
fn tiled(picture: Picture, part: [usize; 4], wide: usize) -> Picture {
    let [tl, tt, tr, tb] = picture.part;
    let mut pixels = vec![0; picture.pixels.len()];
    if tl >= tr || tt >= tb {
        return Picture { pixels, part, ..picture };
    }
    let (tile_wide, tile_high) = (tr - tl, tb - tt);
    for y in part[1]..part[3] {
        let from = tt + (y as i64 - tt as i64).rem_euclid(tile_high as i64) as usize;
        for x in part[0]..part[2] {
            pixels[y * wide + x] = picture.pixels[from * wide + tl + (x as i64 - tl as i64).rem_euclid(tile_wide as i64) as usize];
        }
    }
    Picture { pixels, part, ..picture }
}

/// Turbulence's source: the spec's lattice, made from a seed.
struct Lattice {
    selector: [usize; 514],
    gradient: [[[f64; 2]; 514]; 4],
}

/// The lattice size, and an offset keeping positions positive.
const SIZE: usize = 256;
const AWAY: f64 = 4096.0;

impl Lattice {
    /// The lattice for a seed (truncated to an integer, as the spec says).
    fn new(seed: f32) -> Lattice {
        let mut lattice = Lattice { selector: [0; 514], gradient: [[[0.0; 2]; 514]; 4] };
        let mut seed = start(seed.trunc() as i64);
        for k in 0..4 {
            for i in 0..SIZE {
                lattice.selector[i] = i;
                lattice.gradient[k][i] = lattice.point(&mut seed);
            }
        }
        for i in (1..SIZE).rev() {
            seed = random(seed);
            lattice.selector.swap(i, seed as usize % SIZE);
        }
        for i in 0..SIZE + 2 {
            lattice.selector[SIZE + i] = lattice.selector[i];
            (0..4).for_each(|k| lattice.gradient[k][SIZE + i] = lattice.gradient[k][i]);
        }
        lattice
    }

    /// A unit-length lattice gradient from the next two random numbers.
    fn point(&self, seed: &mut i64) -> [f64; 2] {
        let mut next = || {
            *seed = random(*seed);
            ((*seed % (2 * SIZE as i64)) - SIZE as i64) as f64 / SIZE as f64
        };
        let (x, y) = (next(), next());
        let long = (x * x + y * y).sqrt();
        [x / long, y / long]
    }

    /// One channel's noise at a position, as the spec's `noise2`, with tile stitching where set
    /// (width, height, and where each wraps).
    fn noise(&self, channel: usize, [x, y]: [f64; 2], stitch: Option<[i64; 4]>) -> f64 {
        let (tx, ty) = (x + AWAY, y + AWAY);
        let ([mut bx0, mut by0], [rx0, ry0]) = ([tx as i64, ty as i64], [tx - (tx as i64) as f64, ty - (ty as i64) as f64]);
        let [mut bx1, mut by1] = [bx0.saturating_add(1), by0.saturating_add(1)];
        if let Some([wide, high, wrap_x, wrap_y]) = stitch {
            [bx0, bx1] = [bx0, bx1].map(|b| if b >= wrap_x { b - wide } else { b });
            [by0, by1] = [by0, by1].map(|b| if b >= wrap_y { b - high } else { b });
        }
        let [bx0, bx1, by0, by1] = [bx0, bx1, by0, by1].map(|b| (b & 255) as usize);
        let (i, j) = (self.selector[bx0], self.selector[bx1]);
        let corners = [self.selector[i + by0], self.selector[j + by0], self.selector[i + by1], self.selector[j + by1]];
        let s_curve = |t: f64| t * t * (3.0 - 2.0 * t);
        let (sx, sy, rx1, ry1) = (s_curve(rx0), s_curve(ry0), rx0 - 1.0, ry0 - 1.0);
        let dot = |corner: usize, x: f64, y: f64| x * self.gradient[channel][corner][0] + y * self.gradient[channel][corner][1];
        let lerp = |t: f64, a: f64, b: f64| a + t * (b - a);
        let a = lerp(sx, dot(corners[0], rx0, ry0), dot(corners[1], rx1, ry0));
        let b = lerp(sx, dot(corners[2], rx0, ry1), dot(corners[3], rx1, ry1));
        lerp(sy, a, b)
    }

    /// The turbulence pixel at a position (units), in a tile at `tile` (left, top, width,
    /// height, units): its channels un-premultiplied, then premultiplied.
    fn pixel(&self, noise: &Noise, at: [f64; 2], tile: [f64; 4]) -> u32 {
        let [mut fx, mut fy] = noise.base.map(f64::from);
        let mut stitch = None;
        if noise.stitch {
            fx = stitched(fx, tile[2]);
            fy = stitched(fy, tile[3]);
            let (wide, high) = ((tile[2] * fx + 0.5) as i64, (tile[3] * fy + 0.5) as i64);
            stitch = Some([wide, high, ((tile[0] * fx + AWAY) as i64).saturating_add(wide), ((tile[1] * fy + AWAY) as i64).saturating_add(high)]);
        }
        let mut sums = [0.0f64; 4];
        let (mut place, mut ratio) = ([at[0] * fx, at[1] * fy], 1.0);
        for _ in 0..noise.octaves.min(OCTAVES) {
            for (channel, sum) in sums.iter_mut().enumerate() {
                let value = self.noise(channel, place, stitch);
                *sum += if noise.fractal { value } else { value.abs() } / ratio;
            }
            (place, ratio) = ([place[0] * 2.0, place[1] * 2.0], ratio * 2.0);
            // A tile too large for the arithmetic is clamped: beyond it, nothing is stitched.
            let wrapped = |wrap: i64| wrap.saturating_mul(2).saturating_sub(AWAY as i64);
            stitch = stitch.map(|[wide, high, wrap_x, wrap_y]| [wide.saturating_mul(2), high.saturating_mul(2), wrapped(wrap_x), wrapped(wrap_y)]);
        }
        let level = |sum: f64| ((if noise.fractal { (sum * 255.0 + 255.0) / 2.0 } else { sum * 255.0 }) + 0.5).clamp(0.0, 255.0) as u32;
        let [r, g, b, a] = sums.map(level);
        pixel([a, times(r, a), times(g, a), times(b, a)])
    }
}

/// A frequency adjusted to fit a whole number of times into a tile side, the nearer one.
fn stitched(frequency: f64, side: f64) -> f64 {
    if frequency == 0.0 {
        return frequency;
    }
    let (low, high) = ((side * frequency).floor() / side, (side * frequency).ceil() / side);
    if frequency / low < high / frequency { low } else { high }
}

/// The spec's random seed setup.
fn start(seed: i64) -> i64 {
    let seed = if seed <= 0 { -(seed % (RANDOM_M - 1)) + 1 } else { seed };
    seed.min(RANDOM_M - 1)
}

/// The next random number (Park and Miller's).
fn random(seed: i64) -> i64 {
    let next = 16807 * (seed % 127_773) - 2836 * (seed / 127_773);
    if next <= 0 { next + RANDOM_M } else { next }
}

/// The random number modulus: 2^31 - 1.
const RANDOM_M: i64 = 2_147_483_647;

#[cfg(test)]
mod tests {
    use super::*;

    /// Pseudo-random numbers from a seed.
    fn chance(seed: &mut u64) -> u32 {
        *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (*seed >> 32) as u32
    }

    #[test]
    fn thicker_and_thinner_are_the_most_and_least_of_each_window() {
        // Compared with the naive way, each pixel scanning its whole window: lines of every
        // length, radii from zero to longer than the line.
        let mut seed = 7;
        for (n, radius) in (1..40).flat_map(|n| [0, 1, 2, 3, 7, 50].map(|radius| (n, radius))) {
            let line: Vec<u32> = (0..n).map(|_| chance(&mut seed)).collect();
            for most in [true, false] {
                let mut out = vec![0; n];
                extremes(&line, &mut out, radius, most, &mut [Vec::new(), Vec::new()]);
                let plain = (0..n).map(|at| match most {
                    true => line[at.saturating_sub(radius)..(at + radius + 1).min(n)].iter().fold(0, |a, b| most_of(a, *b)),
                    false if at < radius || at + radius >= n => 0,
                    false => line[at - radius..=at + radius].iter().fold(u32::MAX, |a, b| least_of(a, *b)),
                });
                assert_eq!(out, plain.collect::<Vec<u32>>(), "{n} long, radius {radius}, most {most}");
            }
        }
    }

    #[test]
    fn stitched_noise_meets_itself_at_the_edges_of_its_tile() {
        // A 40 by 30 unit tile, with a frequency fitting it a whole number of times once
        // adjusted: the noise along its right edge equals its left's, its bottom's its top's.
        // (Unstitched, it doesn't.)
        let lattice = Lattice::new(3.0);
        let noise = |stitch: bool| Noise { base: [0.052, 0.07], octaves: 3, seed: 3.0, stitch, fractal: true };
        let tile = [0.0, 0.0, 40.0, 30.0];
        for along in [0.5, 7.25, 13.0, 29.5] {
            let [left, right, top, bottom] = [[0.0, along], [40.0, along], [along, 0.0], [along, 30.0]].map(|at| lattice.pixel(&noise(true), at, tile));
            assert!(left == right && top == bottom, "{along}");
            assert_ne!(lattice.pixel(&noise(false), [0.0, along], tile), lattice.pixel(&noise(false), [40.0, along], tile));
        }
    }

    #[test]
    fn the_bell_curve_is_the_integral_of_itself() {
        // The error function within a few 10⁻⁷ of the integral computed the slow way; and each
        // pixel's weight is the curve's area over it.
        for x in [-3.0f32, -1.2, -0.3, 0.0, 0.1, 0.7, 1.9, 4.0] {
            let steps = 100_000;
            let integral = (0..steps).map(|n| (n as f64 + 0.5) * x as f64 / steps as f64).map(|t| (-t * t).exp()).sum::<f64>() * x as f64 / steps as f64;
            assert!((erf(x) as f64 - integral * 2.0 / std::f64::consts::PI.sqrt()).abs() < 3e-6, "{x}");
        }
        let weights = curve(1.0);
        assert!(weights.len() == 7 && (weights.iter().sum::<f32>() - 1.0).abs() < 1e-6 && (weights[3] - 0.3830).abs() < 3e-4 && (weights[4] - 0.2418).abs() < 3e-4);
    }

    #[test]
    fn a_blur_keeps_all_it_spreads() {
        // A blur of a few pixels in the middle of a large area: as much coverage after as
        // before, to each box's rounding, either way.
        let wide = 64;
        let mut pixels = vec![0u32; wide * wide];
        (28..36).for_each(|y| pixels[y * wide + 28..y * wide + 36].fill(0xff40_80c0));
        for deviation in [0.6, 1.5, 2.0, 3.7, 5.0] {
            let picture = Picture { pixels: pixels.clone(), linear: false, part: [0, 0, wide, wide] };
            let blurred = blurred(picture, [0, 0, wide, wide], wide, [deviation, deviation]);
            let solid = |pixels: &[u32]| pixels.iter().map(|pixel| (pixel >> 24) as f32).sum::<f32>();
            let (before, after) = (solid(&pixels), solid(&blurred.pixels));
            assert!((before - after).abs() / before < 0.01, "{deviation}: {before} {after}");
        }
    }

    #[test]
    fn colours_are_unmultiplied_as_a_division_would() {
        // A multiply and a shift, as an exact division by alpha, rounded to nearest.
        let light = Light::new();
        let same: [u8; 256] = std::array::from_fn(|n| n as u8);
        for a in 1..256u32 {
            for c in 0..=a {
                let divided = (c * 255 + a / 2) / a;
                let pixel = a << 24 | c << 16;
                assert_eq!(changed(pixel, &same, &light.plain) >> 16 & 255, (divided * a + 127) / 255, "{c} of {a}");
            }
        }
    }
}
