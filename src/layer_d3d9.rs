//! The UI over a program's own Direct3D 9 rendering (Direct3D 9Ex too): made once from its
//! device, then drawn each frame with one call, inside the program's scene.
//!
//! ```ignore
//! // SAFETY: the program's own device.
//! let mut layer = unsafe { taut::layer_d3d9::Layer::new(device) }?;
//! // each frame, between the program's BeginScene and EndScene, after its own drawing:
//! layer.draw(&mut ui)?;
//! // before IDirect3DDevice9::Reset (a 9Ex device needs none):
//! layer.before_reset();
//! ```
//!
//! The layer keeps the UI's pixels (a `Surface`), a texture of the same size in the device's
//! memory, and a copy in system memory. `draw` redraws only what changed, writes only that to the
//! copy and copies only that to the texture (`UpdateSurface`), then draws the texture as one quad
//! over the bound render target, at the viewport's top left, clipped to it, pixel for pixel, with
//! premultiplied alpha blending ("one, one minus source alpha"). Everything it sets is restored:
//! a state block of all state is taken before and applied after. A program that shows the UI
//! otherwise calls `update` and draws `texture()` itself.
//!
//! Nothing is loaded or compiled at run time: COM methods are called by their place in the
//! vtable, as d3d9.h orders them, and the quad is drawn by the fixed function pipeline. The
//! render target must not convert to sRGB: the UI's pixels are already in screen colours.
// Every function here is `#[inline]`: only a program that calls one compiles it. (Compiled in
// the library itself, even unused, this code moved how the rest is grouped and optimised.)
pub use crate::layer_d3d11::Error;
use crate::layer_d3d11::{Com, check, function, release};
use crate::surface::Surface;
use crate::ui::Ui;
use std::ffi::c_void;
use std::ptr::null_mut;

/// `D3DFMT_A8R8G8B8`: in memory blue, green, red, alpha, as the UI's pixels are.
const A8R8G8B8: u32 = 21;
/// `D3DFVF_XYZRHW | D3DFVF_TEX1`: corners placed in pixels, with one place in the texture.
const CORNERS: u32 = 0x104;

/// What `draw` sets, as d3d9types.h numbers them: render states (`SetRenderState`); and for
/// each texture stage, its states. Off: depth (7, 14), alpha test (15), culling (22: 1, none),
/// dithering (26), fog (28), specular (29), stencil (52), clip planes (152), sRGB writes (194),
/// separate alpha blending (206), wrapping (128). On: solid filling (8: 3), blending (27) of
/// "one, one minus source alpha, added" (19: 2, 20: 6, 171: 1), clipping (136) and the scissor
/// (174); lighting off (137), all four channels written (168).
const RENDER_STATES: [(u32, u32); 21] = [
    (7, 0),
    (14, 0),
    (15, 0),
    (22, 1),
    (26, 0),
    (28, 0),
    (29, 0),
    (52, 0),
    (152, 0),
    (194, 0),
    (206, 0),
    (128, 0),
    (8, 3),
    (27, 1),
    (19, 2),
    (20, 6),
    (171, 1),
    (136, 1),
    (174, 1),
    (137, 0),
    (168, 0xf),
];

/// Stage 0 gives the texture's colour and alpha as they are (1, 4: 2 "select the first";
/// 2, 5: 2 "the texture"), from its first places (11: 0), not transformed (24: 0); stage 1 is
/// off (1, 4: 1).
const STAGE_STATES: [(u32, u32, u32); 8] = [(0, 1, 2), (0, 2, 2), (0, 4, 2), (0, 5, 2), (0, 11, 0), (0, 24, 0), (1, 1, 1), (1, 4, 1)];

/// Sampler 0 reads the texture's pixels unfiltered (5, 6: 1 point; 7: 0 no mipmaps), clamped
/// at its edges (1, 2: 3), as they are (11: 0, not from sRGB).
const SAMPLER_STATES: [(u32, u32); 6] = [(1, 3), (2, 3), (5, 1), (6, 1), (7, 0), (11, 0)];

/// `D3DVIEWPORT9`.
#[repr(C)]
#[derive(Default)]
struct Viewport {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    near: f32,
    far: f32,
}

/// `D3DLOCKED_RECT`.
#[repr(C)]
struct Locked {
    pitch: i32,
    bits: *mut u8,
}

/// The UI in a program's Direct3D 9: its pixels, a texture of their size, a copy of them in
/// system memory, and what draws them over the program's picture.
pub struct Layer {
    device: Com,
    /// The texture (`IDirect3DTexture9`, in the device's memory); null until the UI has a size,
    /// and after `before_reset`.
    texture: Com,
    /// The copy (`IDirect3DSurface9` in system memory) that changes are copied to the texture
    /// from.
    copy: Com,
    surface: Surface,
}

impl std::fmt::Debug for Layer {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer").field("width", &self.surface.width).field("height", &self.surface.height).finish_non_exhaustive()
    }
}

impl Layer {
    /// The layer for the program's `device` (`IDirect3DDevice9` or `IDirect3DDevice9Ex`). The
    /// layer holds a reference to it while it lives.
    ///
    /// # Safety
    /// `device` is alive, and of that type.
    #[inline]
    pub unsafe fn new(device: *mut c_void) -> Result<Layer, Error> {
        // SAFETY: as the caller promises (vtable entry 1 of every COM object is AddRef).
        unsafe { function::<unsafe extern "system" fn(Com) -> u32>(device, 1)(device) };
        Ok(Layer { device, texture: null_mut(), copy: null_mut(), surface: Surface::default() })
    }

    /// Updates the texture after a run: redraws what changed, and copies only that. (The first
    /// time, after a resize, and after a reset, a new texture is made and all of it copied.)
    #[inline]
    pub fn update(&mut self, ui: &mut Ui) -> Result<(), Error> {
        let size = (self.surface.width, self.surface.height);
        let mut changed = self.surface.update(ui);
        if (self.surface.width, self.surface.height) != size || self.texture.is_null() {
            self.make_texture().inspect_err(|_| self.surface = Surface::default())?;
            changed = [[0, 0, self.surface.width, self.surface.height]].into_iter().filter(|_| !self.texture.is_null()).collect();
        }
        if changed.is_empty() {
            return Ok(());
        }
        let mut level = null_mut();
        // SAFETY: the texture is alive (vtable 18: GetSurfaceLevel).
        check(unsafe { function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(self.texture, 18)(self.texture, 0, &mut level) }, "finding the texture's surface")?;
        let copied = changed.iter().try_for_each(|place| self.copy_place(*place, level));
        // SAFETY: the surface was got above, and is held once.
        unsafe { release(level) };
        copied
    }

    /// Writes `place` (left, top, right, bottom) of the surface to the copy in system memory,
    /// and copies that to `level`, the texture's surface.
    #[inline]
    fn copy_place(&self, [left, top, right, bottom]: [usize; 4], level: Com) -> Result<(), Error> {
        let area = [left as i32, top as i32, right as i32, bottom as i32];
        let mut locked = Locked { pitch: 0, bits: null_mut() };
        let copy = self.copy;
        // SAFETY: the copy and the level are alive and the surface's size, and the area lies in
        // them; a locked area's rows start `pitch` bytes apart. (Surface vtable 13 locks, 14
        // unlocks; device 30: UpdateSurface.)
        unsafe {
            check(function::<unsafe extern "system" fn(Com, *mut Locked, *const [i32; 4], u32) -> i32>(copy, 13)(copy, &mut locked, &area, 0), "writing the copy")?;
            for (row, y) in (top..bottom).enumerate() {
                let from = &self.surface.pixels[y * self.surface.width + left..y * self.surface.width + right];
                std::ptr::copy_nonoverlapping(from.as_ptr().cast::<u8>(), locked.bits.add(row * locked.pitch as usize), from.len() * 4);
            }
            check(function::<unsafe extern "system" fn(Com) -> i32>(copy, 14)(copy), "writing the copy")?;
            let to = [left as i32, top as i32];
            check(function::<unsafe extern "system" fn(Com, Com, *const [i32; 4], Com, *const [i32; 2]) -> i32>(self.device, 30)(self.device, copy, &area, level, &to), "copying to the texture")
        }
    }

    /// A texture of the surface's size, and a copy to match in system memory (kept from before
    /// where it has the size); none while the UI has no size.
    #[inline]
    fn make_texture(&mut self) -> Result<(), Error> {
        let (width, height) = (self.surface.width as u32, self.surface.height as u32);
        // SAFETY: the texture is held here, and let go of.
        unsafe { release(std::mem::replace(&mut self.texture, null_mut())) };
        let mut described = [0u32; 8];
        if !self.copy.is_null() {
            // SAFETY: the copy is alive (vtable 12: GetDesc, a `D3DSURFACE_DESC` whose width
            // and height are its last two numbers).
            unsafe { function::<unsafe extern "system" fn(Com, *mut [u32; 8]) -> i32>(self.copy, 12)(self.copy, &mut described) };
        }
        if [described[6], described[7]] != [width, height] {
            // SAFETY: the old copy is held here, and let go of.
            unsafe { release(std::mem::replace(&mut self.copy, null_mut())) };
        }
        if self.surface.pixels.is_empty() {
            return Ok(());
        }
        type MakeTexture = unsafe extern "system" fn(Com, u32, u32, u32, u32, u32, u32, *mut Com, *mut isize) -> i32;
        type MakeSurface = unsafe extern "system" fn(Com, u32, u32, u32, u32, *mut Com, *mut isize) -> i32;
        let device = self.device;
        // SAFETY: the device is alive. (Vtable 23: CreateTexture, one level, no usage, in the
        // device's memory (pool 0); 36: CreateOffscreenPlainSurface in system memory (2).)
        unsafe {
            if self.copy.is_null() {
                check(function::<MakeSurface>(device, 36)(device, width, height, A8R8G8B8, 2, &mut self.copy, null_mut()), "making the copy")?;
            }
            check(function::<MakeTexture>(device, 23)(device, width, height, 1, 0, A8R8G8B8, 0, &mut self.texture, null_mut()), "making the texture")
        }
    }

    /// Updates the texture (`update`) and draws it over the bound render target: at the
    /// viewport's top left, pixel for pixel, clipped to the viewport. Call it inside the
    /// program's scene. Everything it sets is restored.
    #[inline]
    pub fn draw(&mut self, ui: &mut Ui) -> Result<(), Error> {
        self.update(ui)?;
        if self.texture.is_null() {
            return Ok(());
        }
        let (device, mut viewport, mut kept) = (self.device, Viewport::default(), null_mut());
        // SAFETY: the device is alive (vtable 48: GetViewport; 59: CreateStateBlock of all
        // state, 1).
        unsafe {
            function::<unsafe extern "system" fn(Com, *mut Viewport) -> i32>(device, 48)(device, &mut viewport);
            check(function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(device, 59)(device, 1, &mut kept), "keeping the device's state")?;
        }
        // The corners, in pixels, half a pixel up and to the left: Direct3D 9 puts a pixel's
        // middle at whole numbers.
        let (left, top) = (viewport.x as f32 - 0.5, viewport.y as f32 - 0.5);
        let (right, bottom) = (left + self.surface.width as f32, top + self.surface.height as f32);
        let corners: [[f32; 6]; 4] = [[left, top, 0.0, 1.0, 0.0, 0.0], [right, top, 0.0, 1.0, 1.0, 0.0], [left, bottom, 0.0, 1.0, 0.0, 1.0], [right, bottom, 0.0, 1.0, 1.0, 1.0]];
        let scissor = [viewport.x, viewport.y, viewport.x + viewport.width, viewport.y + viewport.height].map(|edge| edge as i32);
        // SAFETY: the device is alive, and each vtable index is that method: 107 and 92 set the
        // pixel and vertex shaders, 89 the vertex format, 65 the texture, 57 a render state, 67
        // a texture stage's state, 69 a sampler's, 75 the scissor rectangle; 83 draws from the
        // corners above (5: a strip of two triangles, 24 bytes a corner). The state block puts
        // everything back (5: Apply).
        unsafe {
            function::<unsafe extern "system" fn(Com, Com) -> i32>(device, 107)(device, null_mut());
            function::<unsafe extern "system" fn(Com, Com) -> i32>(device, 92)(device, null_mut());
            function::<unsafe extern "system" fn(Com, u32) -> i32>(device, 89)(device, CORNERS);
            function::<unsafe extern "system" fn(Com, u32, Com) -> i32>(device, 65)(device, 0, self.texture);
            RENDER_STATES.iter().for_each(|(state, to)| _ = function::<unsafe extern "system" fn(Com, u32, u32) -> i32>(device, 57)(device, *state, *to));
            STAGE_STATES.iter().for_each(|(stage, state, to)| _ = function::<unsafe extern "system" fn(Com, u32, u32, u32) -> i32>(device, 67)(device, *stage, *state, *to));
            SAMPLER_STATES.iter().for_each(|(state, to)| _ = function::<unsafe extern "system" fn(Com, u32, u32, u32) -> i32>(device, 69)(device, 0, *state, *to));
            function::<unsafe extern "system" fn(Com, *const [i32; 4]) -> i32>(device, 75)(device, &scissor);
            function::<unsafe extern "system" fn(Com, u32, u32, *const c_void, u32) -> i32>(device, 83)(device, 5, 2, corners.as_ptr().cast(), 24);
            function::<unsafe extern "system" fn(Com) -> i32>(kept, 5)(kept);
            release(kept);
        }
        Ok(())
    }

    /// Lets go of the texture, which lives in the device's memory: call it before the program
    /// calls `IDirect3DDevice9::Reset`, which fails while such things are held. The next
    /// `update` makes it again, whole. (Not needed with Direct3D 9Ex, which keeps them.)
    #[inline]
    pub fn before_reset(&mut self) {
        // SAFETY: the texture is held here, and let go of.
        unsafe { release(std::mem::replace(&mut self.texture, null_mut())) };
    }

    /// The texture (`IDirect3DTexture9`, `D3DFMT_A8R8G8B8`, premultiplied alpha), as `update`
    /// left it; null while the UI has no size, or after `before_reset`. Owned by the layer: a
    /// program keeping it calls `AddRef`.
    #[inline]
    pub fn texture(&self) -> *mut c_void {
        self.texture
    }

    /// The UI's pixels, which the texture holds.
    #[inline]
    pub fn surface(&self) -> &Surface {
        &self.surface
    }
}

impl Drop for Layer {
    #[inline]
    fn drop(&mut self) {
        // SAFETY: each is held once, by this layer.
        unsafe { [self.texture, self.copy, self.device].into_iter().for_each(|object| release(object)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The structures handed to Direct3D 9 are as large as d3d9types.h makes them on 64-bit
    /// Windows.
    #[test]
    fn the_structures_are_laid_out_as_direct3d_lays_them_out() {
        assert_eq!(size_of::<Viewport>(), 24);
        assert_eq!(size_of::<Locked>(), 16);
    }
}
