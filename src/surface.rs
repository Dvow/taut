//! The interface drawn into a block of pixels in memory, for whatever shows it: a renderer (as
//! a texture for Direct3D, OpenGL, Vulkan, Metal, a web canvas), a system with no window here
//! yet, or a test that compares pixels.
//!
//! ```ignore
//! let changed = ui.run(&mut describe);
//! surface.redraw(&mut ui, changed); // only what changed is drawn again
//! // then: copy the pixels inside `changed` to the texture, and draw the texture
//! ```
//!
//! A pixel is `0xAARRGGBB`, premultiplied (in memory: blue, green, red, alpha). Where nothing
//! is drawn on a page of [`Color::NONE`](crate::Color::NONE), pixels stay clear, so the picture
//! can lie over a game or anything else.
use crate::canvas::Canvas;
use crate::geom::Rect;
use crate::ui::Ui;

/// An interface drawn into a block of pixels with no window: each pixel `0xAARRGGBB`,
/// premultiplied. `Surface::default()` has no pixels; `update` draws it whole the first time.
#[doc(alias("offscreen", "render_target"))]
#[derive(Default)]
pub struct Surface {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Rows of pixels, top first.
    pub pixels: Vec<u32>,
}

impl std::fmt::Debug for Surface {
    /// Its size; not its pixels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface").field("width", &self.width).field("height", &self.height).finish_non_exhaustive()
    }
}

impl Surface {
    /// The whole interface as last laid out, drawn afresh `strip` rows at a time, as a window
    /// draws through one small block.
    pub fn draw(ui: &mut Ui, strip: usize) -> Surface {
        let (scale, strip) = (ui.scale(), strip.max(1));
        let (width, height) = ((ui.size().w * scale).round() as usize, (ui.size().h * scale).round() as usize);
        let mut pixels = vec![0u32; width * height];
        let mut block = vec![0u32; width * strip];
        for top in (0..height).step_by(strip) {
            let rows = strip.min(height - top);
            block.fill(0);
            ui.paint(&mut Canvas::new(&mut block[..width * rows], width, rows, (0, top as i32), scale));
            pixels[top * width..(top + rows) * width].copy_from_slice(&block[..width * rows]);
        }
        Surface { width, height, pixels }
    }

    /// Redraws the part of the interface in `area` and leaves the rest: what a window does with
    /// the rectangle a run gives back. Within it only the changed places are drawn, when there
    /// are several far apart. If the interface changed size, all of it is drawn.
    pub fn redraw(&mut self, ui: &mut Ui, area: Rect) {
        let scale = ui.scale();
        if (self.width, self.height) != ((ui.size().w * scale).round() as usize, (ui.size().h * scale).round() as usize) {
            *self = Surface::draw(ui, 64);
            return;
        }
        // The waiting places are what `area` stands for when it is the rectangle around them;
        // once all of them lie in what is drawn here, nothing waits.
        let places = if area == ui.around { ui.places.len().max(1) } else { 1 };
        for place in 0..places {
            let part = ui.places.get(place).copied().filter(|_| places > 1).unwrap_or(area);
            let mut canvas = Canvas::new(&mut self.pixels, self.width, self.height, (0, 0), scale);
            canvas.clip(part.snap_to_pixels(scale));
            if ui.theme.bg.a() < 255 {
                canvas.erase(); // a see-through page must be cleared first
            }
            ui.paint(&mut canvas);
        }
        if area.intersect(ui.around) == ui.around || ui.around.is_empty() {
            ui.changes();
        }
    }

    /// Brings the surface up to date after a run: redraws what changed since the last call and
    /// gives back where, in pixels (left, top, right, bottom), one rectangle per place: what a
    /// renderer copies to its texture. If the interface changed size, all of it is drawn and
    /// given back.
    pub fn update(&mut self, ui: &mut Ui) -> Vec<[usize; 4]> {
        let scale = ui.scale();
        // Copied as a slice, not collected: collecting would add a second user of the windows'
        // own way of taking changes, and change what every program carries.
        let places = ui.changes().as_slice().to_vec();
        if (self.width, self.height) != ((ui.size().w * scale).round() as usize, (ui.size().h * scale).round() as usize) {
            *self = Surface::draw(ui, 64);
            return [[0, 0, self.width, self.height]].into_iter().filter(|_| !self.pixels.is_empty()).collect();
        }
        let edge = |at: f32, most: usize| ((at * scale).round().max(0.0) as usize).min(most);
        let mut changed = Vec::new();
        for place in places {
            self.redraw(ui, place);
            let place = place.snap_to_pixels(scale);
            let [left, top, right, bottom] = [edge(place.x, self.width), edge(place.y, self.height), edge(place.right(), self.width), edge(place.bottom(), self.height)];
            changed.extend((left < right && top < bottom).then_some([left, top, right, bottom]));
        }
        changed
    }

    /// The pixels of `place` (left, top, right, bottom, as `update` gives it) as a texture upload
    /// takes them (`write_texture`, `glTexSubImage2D`, `UpdateSubresource`, `updateTexture2D`):
    /// where they go, how many, and their bytes, read straight from the surface. A place outside
    /// the surface is cut to it.
    #[inline]
    pub fn upload(&self, place: [usize; 4]) -> Upload<'_> {
        let [left, right] = [place[0], place[2]].map(|x| x.min(self.width));
        let [top, bottom] = [place[1], place[3]].map(|y| y.min(self.height));
        let (width, height) = (right.saturating_sub(left), bottom.saturating_sub(top));
        let pixels = match width * height {
            0 => &[][..],
            _ => &self.pixels[top * self.width + left..(bottom - 1) * self.width + right],
        };
        // SAFETY: a pixel is four bytes, and any byte is a value.
        let bytes = unsafe { std::slice::from_raw_parts(pixels.as_ptr().cast::<u8>(), pixels.len() * 4) };
        Upload { x: left as u32, y: top as u32, width: width as u32, height: height as u32, bytes, bytes_per_row: (self.width * 4) as u32 }
    }

    /// The picture as a PNG file; empty for a surface never drawn.
    pub fn to_png(&self) -> Vec<u8> {
        crate::png::encode(self.width, &self.pixels).unwrap_or_default()
    }
}

/// A place of a surface as a texture upload takes it: `width` by `height` pixels to `x`, `y`,
/// from `bytes` (B, G, R, A, premultiplied), rows `bytes_per_row` apart.
///
/// ```ignore
/// // wgpu
/// for place in surface.update(&mut ui) {
///     let up = surface.upload(place);
///     let at = wgpu::Origin3d { x: up.x, y: up.y, z: 0 };
///     let size = wgpu::Extent3d { width: up.width, height: up.height, depth_or_array_layers: 1 };
///     let layout = wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(up.bytes_per_row), rows_per_image: None };
///     queue.write_texture(wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: at, aspect: wgpu::TextureAspect::All }, up.bytes, layout, size);
/// }
/// // OpenGL: glPixelStorei(GL_UNPACK_ROW_LENGTH, up.bytes_per_row / 4), then
/// // glTexSubImage2D(GL_TEXTURE_2D, 0, x, y, width, height, GL_BGRA, GL_UNSIGNED_INT_8_8_8_8_REV, bytes)
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Upload<'a> {
    /// The left edge, in pixels.
    pub x: u32,
    /// The top edge.
    pub y: u32,
    /// How many pixels wide.
    pub width: u32,
    /// How many rows.
    pub height: u32,
    /// From the top left pixel to the bottom right one: each row `width * 4` bytes, then the
    /// rest of the surface's row up to the next.
    pub bytes: &'a [u8],
    /// From one row to the next, in bytes: the surface's width times 4.
    pub bytes_per_row: u32,
}

impl std::fmt::Debug for Upload<'_> {
    /// Its place and size; not its bytes.
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Upload { x, y, width, height, bytes_per_row, .. } = self;
        f.debug_struct("Upload").field("x", x).field("y", y).field("width", width).field("height", height).field("bytes", &self.bytes.len()).field("bytes_per_row", bytes_per_row).finish()
    }
}
