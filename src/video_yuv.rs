//! Videos' frames of YUV 4:2:0 made RGB (the feature `video`): on the CPU, and by the shaders
//! that draw a frame from the decoder's own buffer, with the same 14-bit fixed-point numbers, so
//! both give the same pixels. Each chroma sample covers 2 × 2 pixels, read as the nearest.

/// One plane of a frame: its bytes, and the steps between rows and between samples.
pub(crate) struct Plane<'a> {
    pub(crate) bytes: &'a [u8],
    pub(crate) row: usize,
    pub(crate) step: usize,
}

impl Plane<'_> {
    /// The sample at column `x` of row `y` (0 past the end).
    pub(crate) fn at(&self, x: usize, y: usize) -> u8 {
        self.bytes.get(y * self.row + x * self.step).copied().unwrap_or(0)
    }
}

/// How YUV becomes RGB: by BT.601, BT.709 or BT.2020, in limited or full range, in 14-bit
/// fixed point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Colours {
    black: i32,
    luma: i32,
    red_v: i32,
    green_u: i32,
    green_v: i32,
    blue_u: i32,
}

/// The weights of red and blue in luma (Kr, Kb) of each standard.
pub(crate) const BT601: (f32, f32) = (0.299, 0.114);
pub(crate) const BT709: (f32, f32) = (0.2126, 0.0722);
pub(crate) const BT2020: (f32, f32) = (0.2627, 0.0593);

impl Colours {
    /// Of the standard with luma weights `(kr, kb)`, in full range or limited.
    pub(crate) fn new((kr, kb): (f32, f32), full: bool) -> Colours {
        let (luma, chroma) = if full { (1.0, 1.0) } else { (255.0 / 219.0, 255.0 / 224.0) };
        let (fixed, kg) = (|value: f32| (value * 16384.0).round() as i32, 1.0 - kr - kb);
        Colours {
            black: if full { 0 } else { 16 },
            luma: fixed(luma),
            red_v: fixed(2.0 * (1.0 - kr) * chroma),
            green_u: fixed(2.0 * (1.0 - kb) * kb / kg * chroma),
            green_v: fixed(2.0 * (1.0 - kr) * kr / kg * chroma),
            blue_u: fixed(2.0 * (1.0 - kb) * chroma),
        }
    }

    /// One pixel, opaque `0xAARRGGBB`.
    pub(crate) fn rgb(&self, y: u8, u: u8, v: u8) -> u32 {
        let y = (i32::from(y) - self.black) * self.luma;
        let (u, v) = (i32::from(u) - 128, i32::from(v) - 128);
        let channel = |value: i32| ((value + 8192) >> 14).clamp(0, 255) as u32;
        0xFF00_0000 | channel(y + self.red_v * v) << 16 | channel(y - self.green_u * u - self.green_v * v) << 8 | channel(y + self.blue_u * u)
    }

    /// The pixels of `[left, top, right, bottom]` of a frame of planes `y`, `u` and `v`.
    pub(crate) fn pixels(&self, [y, u, v]: [&Plane; 3], [left, top, right, bottom]: [usize; 4]) -> Vec<u32> {
        let rows = (top..bottom).flat_map(|row| (left..right).map(move |column| (column, row)));
        rows.map(|(x, row)| self.rgb(y.at(x, row), u.at(x / 2, row / 2), v.at(x / 2, row / 2))).collect()
    }

    /// What the shaders take of it for a frame `size` pixels big: black, luma, red from V,
    /// green from U; green from V, blue from U, the width and height (`FROM_YUV`).
    #[cfg(feature = "gpu-draw")]
    pub(crate) fn for_shaders(&self, size: (u32, u32)) -> [f32; 8] {
        let Colours { black, luma, red_v, green_u, green_v, blue_u } = *self;
        [black, luma, red_v, green_u, green_v, blue_u, size.0 as i32, size.1 as i32].map(|number| number as f32)
    }
}

/// The GLSL of `Colours::rgb`: a pixel's Y, U and V bytes made RGB (0 to 1), opaque, by the
/// numbers `Colours::for_shaders` gives (`a` the first four, `b` the rest). (Android's OpenGL
/// reads frames made RGB by the driver; its Vulkan shader is compiled ahead.)
#[cfg(all(feature = "gpu-draw", any(test, not(target_os = "android"))))]
pub(crate) const FROM_YUV: &str = r#"
vec4 from_yuv(vec3 yuv, vec4 a, vec4 b) {
    int luma = (int(yuv.x) - int(a.x)) * int(a.y);
    int u = int(yuv.y) - 128;
    int v = int(yuv.z) - 128;
    ivec3 rgb = (ivec3(luma + int(a.z) * v, luma - int(a.w) * u - int(b.x) * v, luma + int(b.y) * u) + 8192) >> 14;
    return vec4(clamp(vec3(rgb), 0.0, 255.0) / 255.0, 1.0);
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_black_grey_and_white_come_back_as_they_went_in() {
        let colours = Colours::new(BT601, false);
        assert_eq!(colours.rgb(16, 128, 128), 0xFF00_0000);
        assert_eq!(colours.rgb(126, 128, 128), 0xFF80_8080);
        assert_eq!(colours.rgb(235, 128, 128), 0xFFFF_FFFF);
    }

    #[test]
    fn bt601_red_is_red() {
        // (BT.601 limited: red is Y 81, U 90, V 240.)
        let red = Colours::new(BT601, false).rgb(81, 90, 240);
        let [_, r, g, b] = red.to_be_bytes();
        assert!(r >= 253 && g <= 2 && b <= 2, "{red:#x}");
    }

    #[test]
    fn each_chroma_sample_covers_two_by_two_pixels() {
        let (luma, blue, red) = ([128u8; 16], [64u8, 192, 64, 192], [128u8; 4]);
        let (y, u, v) = (Plane { bytes: &luma, row: 4, step: 1 }, Plane { bytes: &blue, row: 2, step: 1 }, Plane { bytes: &red, row: 2, step: 1 });
        let pixels = Colours::new(BT709, true).pixels([&y, &u, &v], [1, 0, 4, 2]);
        assert_eq!(pixels.len(), 6);
        assert_eq!(pixels[0], pixels[3], "rows 0 and 1 share chroma");
        assert_ne!(pixels[0], pixels[1], "columns 1 and 2 do not");
        assert_eq!(pixels[1], pixels[2]);
    }

    #[cfg(feature = "gpu-draw")]
    #[test]
    fn the_shaders_are_given_the_same_numbers() {
        let colours = Colours::new(BT709, false);
        let [black, luma, red_v, green_u, green_v, blue_u, width, height] = colours.for_shaders((64, 48));
        assert_eq!((black, width, height), (16.0, 64.0, 48.0));
        assert_eq!([luma, red_v, green_u, green_v, blue_u], [colours.luma, colours.red_v, colours.green_u, colours.green_v, colours.blue_u].map(|n| n as f32));
    }
}
