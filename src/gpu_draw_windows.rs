//! Windows' GPU drawing (the feature `gpu-draw`): a recorded paint (`gpu_draw::DrawList`) drawn
//! by Direct3D 11 into a window's texture, and GDI's text recorded as runs.
//!
//! - The shaders are compiled when the device is made, by `d3dcompiler_47.dll` (part of Windows
//!   10 and later): `vs_4_0` and `ps_4_0`, so any device of feature level 10 or more draws.
//! - The quads go up into a buffer read by vertex id and instance (no input layout); glyphs and
//!   runs into a kept cache texture, patches and pictures into a texture of this part's.
//! - GDI's runs (ClearType too) blend per channel: dual-source blending, the second colour
//!   being how much of each channel is covered. Everything else blends as premultiplied
//!   colour over what is there.
use crate::color::Color;
use crate::gpu_draw::{DrawList, Fast, SHADING, SIDE};
use crate::gpu_windows::{function_in, system_library};
use crate::layer_d3d11::{B8G8R8A8_UNORM, Box3, Com, TextureDescription, function, release};
use crate::text::Font;
use crate::windows::sys::{GdiFlush, HANDLE, IntersectClipRect, RestoreDC, SaveDC, SelectObject, SetBkMode, SetTextColor, SetViewportOrgEx, TextOutW};
use std::ffi::{CStr, c_char, c_void};
use std::hash::{Hash, Hasher};
use std::ptr::{null, null_mut};

#[link(name = "gdi32")]
unsafe extern "system" {
    fn GetTextExtentPoint32W(dc: HANDLE, text: *const u16, length: i32, size: *mut [i32; 2]) -> i32;
}

/// `D3DCompile`, as d3dcompiler.h declares it.
type Compile = unsafe extern "system" fn(*const c_void, usize, *const c_char, *const c_void, *const c_void, *const c_char, *const c_char, u32, u32, *mut Com, *mut Com) -> i32;
type Make<T> = unsafe extern "system" fn(Com, *const T, *mut Com) -> i32;
type MakeShader = unsafe extern "system" fn(Com, *const u32, usize, Com, *mut Com) -> i32;
type MakeView = unsafe extern "system" fn(Com, Com, *const [u32; 6], *mut Com) -> i32;
type Put = unsafe extern "system" fn(Com, Com, u32, *const Box3, *const c_void, u32, u32);
type SetShader = unsafe extern "system" fn(Com, Com, *const Com, u32);
type Views = unsafe extern "system" fn(Com, u32, u32, *const Com);

/// `D3D11_BUFFER_DESC`: size, usage, binding, CPU access, misc, stride.
type BufferDescription = [u32; 6];
/// `D3D11_BLEND_DESC`: alpha to coverage, independent blending, then eight targets of seven
/// numbers and a byte of channels each.
type BlendDescription = [u32; 66];
/// `D3D11_RASTERIZER_DESC`: ten numbers.
type RasterizerDescription = [u32; 10];

/// The quads' instances in HLSL, the pixel shader blending what `SHADING` gives.
pub(crate) const MAIN: &str = r#"
struct Between {
    float4 at : SV_Position;
    nointerpolation float4 shape : SHAPE;
    nointerpolation float4 more : MORE;
    nointerpolation float4 source : SOURCE;
    nointerpolation float4 color : COLOR;
    nointerpolation float kind : KIND;
};
Between corner(uint vertex : SV_VertexID, uint quad : SV_InstanceID) {
    Between o;
    uint at = quad * 6;
    float4 place = quads.Load(at);
    o.shape = quads.Load(at + 1);
    o.more = quads.Load(at + 2);
    o.source = quads.Load(at + 3);
    o.color = quads.Load(at + 4);
    float4 kind = quads.Load(at + 5);
    o.kind = kind.x;
    float2 p = float2((vertex & 1) != 0 ? place.z : place.x, (vertex & 2) != 0 ? place.w : place.y);
    o.at = float4(p.x / kind.z * 2.0 - 1.0, 1.0 - p.y / kind.w * 2.0, 0.0, 1.0);
    return o;
}
struct Drawn {
    float4 color : SV_Target0;
    float4 covers : SV_Target1;
};
Drawn pixel(Between i) {
    Drawn o;
    o.color = shade(i.at.xy, i.shape, i.more, i.source, i.color, i.kind, o.covers);
    return o;
}
"#;

/// HLSL's names for what `SHADING` uses, and the textures it reads.
pub(crate) const PRELUDE: &str = r#"
#define vec2 float2
#define vec3 float3
#define vec4 float4
#define fract frac
#define as_float asfloat
Buffer<float4> quads : register(t0);
Texture2D<float4> cache : register(t1);
Texture2D<float4> patches : register(t2);
float4 texel(int which, int x, int y) {
    if (which == 0) {
        return cache.Load(int3(x, y, 0));
    }
    return patches.Load(int3(x, y, 0));
}
"#;

/// What a device draws with: its shaders, blending and rasterizer states, the quads' buffer
/// (room for `room` quads) and its view, and the cache and patch textures and their views.
pub(crate) struct Pipeline {
    shaders: [Com; 2],
    blend: Com,
    rasterizer: Com,
    quads: Com,
    quads_view: Com,
    room: usize,
    textures: [Com; 2],
    views: [Com; 2],
}

// SAFETY: as its device: made on the device's thread, then used by the windows' alone.
unsafe impl Send for Pipeline {}

impl std::fmt::Debug for Pipeline {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pipeline").field("room", &self.room).finish_non_exhaustive()
    }
}

/// HLSL compiled for `target` (as `vs_4_0`), from function `entry`: its words; nothing where
/// d3dcompiler_47.dll is missing or the source doesn't compile.
pub(crate) fn compiled(source: &str, entry: &CStr, target: &CStr) -> Option<Vec<u32>> {
    let found = function_in(system_library("d3dcompiler_47.dll"), c"D3DCompile");
    let (mut code, mut said) = (null_mut(), null_mut());
    // SAFETY: `D3DCompile` as declared; the names end in zeros, the source is as long as said.
    // 1 << 13 is `D3DCOMPILE_IEEE_STRICTNESS`: floats as written. The blobs' GetBufferPointer
    // (3) and GetBufferSize (4); both blobs are released.
    unsafe {
        if found == 0 {
            return None;
        }
        let made = std::mem::transmute::<usize, Compile>(found)(source.as_ptr().cast(), source.len(), null(), null(), null(), entry.as_ptr(), target.as_ptr(), 1 << 13, 0, &mut code, &mut said);
        release(said);
        if made < 0 || code.is_null() {
            return None;
        }
        let (first, size) = (function::<unsafe extern "system" fn(Com) -> *const u32>(code, 3)(code), function::<unsafe extern "system" fn(Com) -> usize>(code, 4)(code));
        let words = std::slice::from_raw_parts(first, size / 4).to_vec();
        release(code);
        Some(words)
    }
}

/// A texture `SIDE` pixels square that shaders read, and its view; nulls where none is made.
fn texture(device: Com) -> [Com; 2] {
    let described = TextureDescription {
        width: SIDE as u32,
        height: SIDE as u32,
        mip_levels: 1,
        array_size: 1,
        format: B8G8R8A8_UNORM,
        sample_count: 1,
        sample_quality: 0,
        usage: 0,
        bind_flags: 0x8,
        cpu_access_flags: 0,
        misc_flags: 0,
    };
    let (mut texture, mut view) = (null_mut(), null_mut());
    type MakeTexture = unsafe extern "system" fn(Com, *const TextureDescription, *const c_void, *mut Com) -> i32;
    // SAFETY: the device's CreateTexture2D (5) and CreateShaderResourceView (7) with no
    // description: the whole texture.
    unsafe {
        if function::<MakeTexture>(device, 5)(device, &described, null(), &mut texture) >= 0 {
            function::<unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32>(device, 7)(device, texture, null(), &mut view);
        }
    }
    [texture, view]
}

impl Pipeline {
    /// What `device` (an `ID3D11Device`) draws with; nothing where any part can't be made.
    pub(crate) fn new(device: Com) -> Option<Pipeline> {
        let source = format!("{PRELUDE}{SHADING}{MAIN}");
        let (corner, pixel) = (compiled(&source, c"corner", c"vs_4_0")?, compiled(&source, c"pixel", c"ps_4_0")?);
        let mut made =
            Pipeline { shaders: [null_mut(); 2], blend: null_mut(), rasterizer: null_mut(), quads: null_mut(), quads_view: null_mut(), room: 0, textures: [null_mut(); 2], views: [null_mut(); 2] };
        // One, one minus the second colour (2, 17), added (1); alpha likewise (2, 19, 1); all
        // four channels.
        let mut blend = [0u32; 66];
        blend[2..10].copy_from_slice(&[1, 2, 17, 1, 2, 19, 1, 0xf]);
        // Solid (3), not culled (1), clipped to depth and to the scissor rectangle.
        let rasterizer: RasterizerDescription = [3, 1, 0, 0, 0, 0, 1, 1, 0, 0];
        // SAFETY: the device's CreateVertexShader (12) and CreatePixelShader (15) from compiled
        // words; CreateBlendState (20), CreateRasterizerState (22) from complete descriptions.
        unsafe {
            function::<MakeShader>(device, 12)(device, corner.as_ptr(), corner.len() * 4, null_mut(), &mut made.shaders[0]);
            function::<MakeShader>(device, 15)(device, pixel.as_ptr(), pixel.len() * 4, null_mut(), &mut made.shaders[1]);
            function::<Make<BlendDescription>>(device, 20)(device, &blend, &mut made.blend);
            function::<Make<RasterizerDescription>>(device, 22)(device, &rasterizer, &mut made.rasterizer);
        }
        let [cache, cache_view] = texture(device);
        let [patches, patches_view] = texture(device);
        (made.textures, made.views) = ([cache, patches], [cache_view, patches_view]);
        let all = [made.shaders[0], made.shaders[1], made.blend, made.rasterizer, cache_view, patches_view];
        all.iter().all(|object| !object.is_null()).then_some(made)
    }

    /// A view to draw into `texture` (the window's) with; null where none is made.
    pub(crate) fn target(device: Com, texture: Com) -> Com {
        let mut view = null_mut();
        // SAFETY: the device's CreateRenderTargetView (9), with no description: the whole
        // texture, made to be drawn into.
        unsafe { function::<unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32>(device, 9)(device, texture, null(), &mut view) };
        view
    }

    /// Makes the quads' buffer hold at least `count`, where it doesn't. Says whether it does.
    fn hold(&mut self, device: Com, count: usize) -> bool {
        if count <= self.room {
            return true;
        }
        let room = count.next_power_of_two().max(1024);
        let described: BufferDescription = [(room * size_of::<crate::gpu_draw::Quad>()) as u32, 0, 0x8, 0, 0, 0];
        // Four floats an element (`DXGI_FORMAT_R32G32B32A32_FLOAT`, 2), a buffer (1).
        let view: [u32; 6] = [2, 1, 0, (room * 6) as u32, 0, 0];
        // SAFETY: the old buffer and view are held here alone; the device's CreateBuffer (3) and
        // CreateShaderResourceView (7).
        unsafe {
            [std::mem::replace(&mut self.quads_view, null_mut()), std::mem::replace(&mut self.quads, null_mut())].into_iter().for_each(|object| release(object));
            self.room = 0;
            if function::<unsafe extern "system" fn(Com, *const BufferDescription, *const c_void, *mut Com) -> i32>(device, 3)(device, &described, null(), &mut self.quads) < 0 {
                return false;
            }
            if function::<MakeView>(device, 7)(device, self.quads, &view, &mut self.quads_view) < 0 {
                return false;
            }
        }
        self.room = room;
        true
    }

    /// Draws `list` into the texture `target` views, `size` pixels, inside `area` (left, top,
    /// right, bottom): its texels put in first. Says whether it could.
    pub(crate) fn draw(&mut self, device: Com, context: Com, target: Com, list: &DrawList, area: [i32; 4], size: [i32; 2]) -> bool {
        if list.quads.is_empty() {
            return !target.is_null();
        }
        if target.is_null() || !self.hold(device, list.quads.len()) {
            return false;
        }
        for upload in &list.uploads {
            let [left, top, width, height] = upload.at.map(|side| side as u32);
            let place = Box3 { left, top, front: 0, right: left + width, bottom: top + height, back: 1 };
            // SAFETY: the box lies in the texture (`Shelves` keeps it there); the staging holds
            // `height` rows of `width` from `from`. (The context's UpdateSubresource, 48.)
            unsafe { function::<Put>(context, 48)(context, self.textures[upload.texture as usize], 0, &place, list.staging[upload.from..].as_ptr().cast(), width * 4, 0) };
        }
        let bytes = (list.quads.len() * size_of::<crate::gpu_draw::Quad>()) as u32;
        let (viewport, scissor) = ([0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0], area);
        // SAFETY: the context's methods by place in its vtable, each with what it takes: 48
        // UpdateSubresource (a buffer's box is in bytes), 17 IASetInputLayout, 24
        // IASetPrimitiveTopology (a strip, 5), 11/60/64/23/9 each stage's shader, 25 and 8 the
        // vertex and pixel stages' views, 35 OMSetBlendState, 43 RSSetState, 44 RSSetViewports,
        // 45 RSSetScissorRects, 33 OMSetRenderTargets, 21 DrawInstanced.
        unsafe {
            let whole = Box3 { left: 0, top: 0, front: 0, right: bytes, bottom: 1, back: 1 };
            function::<Put>(context, 48)(context, self.quads, 0, &whole, list.quads.as_ptr().cast(), 0, 0);
            function::<unsafe extern "system" fn(Com, Com)>(context, 17)(context, null_mut());
            function::<unsafe extern "system" fn(Com, u32)>(context, 24)(context, 5);
            function::<SetShader>(context, 11)(context, self.shaders[0], null(), 0);
            [60, 64, 23].into_iter().for_each(|at| function::<SetShader>(context, at)(context, null_mut(), null(), 0));
            function::<SetShader>(context, 9)(context, self.shaders[1], null(), 0);
            function::<Views>(context, 25)(context, 0, 1, &self.quads_view);
            function::<Views>(context, 8)(context, 1, 2, self.views.as_ptr());
            function::<unsafe extern "system" fn(Com, Com, *const f32, u32)>(context, 35)(context, self.blend, null(), u32::MAX);
            function::<unsafe extern "system" fn(Com, Com)>(context, 43)(context, self.rasterizer);
            function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(context, 44)(context, 1, &viewport);
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(context, 45)(context, 1, &scissor);
            function::<unsafe extern "system" fn(Com, u32, *const Com, Com)>(context, 33)(context, 1, &target, null_mut());
            function::<unsafe extern "system" fn(Com, u32, u32, u32, u32)>(context, 21)(context, 4, list.quads.len() as u32, 0, 0);
            function::<unsafe extern "system" fn(Com, u32, *const Com, Com)>(context, 33)(context, 0, null(), null_mut());
        }
        true
    }
}

impl Drop for Pipeline {
    #[inline]
    fn drop(&mut self) {
        let [vertex, pixel] = self.shaders;
        let all = [vertex, pixel, self.blend, self.rasterizer, self.quads_view, self.quads, self.views[0], self.views[1], self.textures[0], self.textures[1]];
        // SAFETY: each is held once, by this pipeline.
        unsafe { all.into_iter().for_each(|object| release(object)) };
    }
}

/// An OpenGL function of the current WGL context by name: `wglGetProcAddress`'s, or for those
/// of OpenGL 1.1 opengl32.dll's own; null where neither has it.
pub(crate) fn wgl_function(name: &CStr) -> *const c_void {
    let library = system_library("opengl32.dll");
    let find = function_in(library, c"wglGetProcAddress");
    // SAFETY: `wglGetProcAddress` as wingdi.h declares it; the name ends in a zero. Some drivers
    // say 1, 2, 3 or -1 for none.
    let found = if find == 0 { 0 } else { unsafe { std::mem::transmute::<usize, unsafe extern "system" fn(*const c_char) -> usize>(find)(name.as_ptr()) } };
    let found = if matches!(found, 0..=3 | usize::MAX) { function_in(library, name) } else { found };
    found as *const c_void
}

/// The strip GDI draws a window's text into: its DC, and its pixels, `width` a row, `rows`
/// rows.
pub(crate) struct Strip {
    pub(crate) dc: HANDLE,
    pub(crate) pixels: *mut u32,
    pub(crate) width: i32,
    pub(crate) rows: i32,
}

/// A line of text in GDI's `handle` (`font` at `px` pixels), its top left at `at` in window
/// pixels, recorded as a run, drawn by GDI into the strip once and kept. On a solid colour
/// (`DrawList::run_on`) it is drawn on that colour: the pixels GDI gives the CPU. On anything
/// else (`DrawList::run`), black on white for dark ink, white on black for light, each channel's
/// distance from the background being how much it covers: GDI's contrast is then lost.
#[allow(clippy::too_many_arguments)] // (What GDI's own drawing takes, and the list.)
pub(crate) fn run(list: &mut DrawList, strip: &Strip, handle: HANDLE, font: Font, px: i32, units: &[u16], at: [i32; 2], ink: Color, clip: [i32; 4]) {
    let mut size = [0i32; 2];
    // SAFETY: the strip's DC is alive; the font is a live GDI font; the text is as long as said.
    unsafe {
        SelectObject(strip.dc, handle);
        GetTextExtentPoint32W(strip.dc, units.as_ptr(), units.len() as i32, &mut size);
    }
    // Room for what overhangs the advance: italics, ClearType's fringes.
    let pad = [size[1] / 4 + 2, 2];
    let (corner, whole) = ([at[0] - pad[0], at[1] - pad[1]], [size[0] + 2 * pad[0], size[1] + 2 * pad[1]]);
    let shown = [corner[0].max(clip[0]), corner[1].max(clip[1]), (corner[0] + whole[0]).min(clip[2]), (corner[1] + whole[1]).min(clip[3])];
    if shown[0] >= shown[2] || shown[1] >= shown[3] || units.is_empty() {
        return;
    }
    if strip.pixels.is_null() || strip.width <= 0 || strip.rows <= 0 {
        return list.failed = true;
    }
    // GDI's colour: red in the low byte; and GDI ignores alpha.
    let ink = Color(ink.0 | 0xff00_0000);
    let text = (ink.0 >> 16 & 0xff) | (ink.0 & 0xff00) | (ink.0 & 0xff) << 16;
    // On the colour under the middle of its letters, where all it draws lies on that.
    let middle = [at[0] + size[0] / 2, at[1] + size[1] / 2];
    if let Some(under) = list.solid_under([middle[0], middle[1], middle[0] + 1, middle[1] + 1]) {
        let mut hash = Fast(0x6f6e_7275);
        (font.face, px, units, under.0, text).hash(&mut hash);
        let how = [under.0, text, 0, 0];
        if list.run_on(hash.finish(), corner, whole, under, clip, |texels| drawn(strip, handle, units, pad, whole, how, texels)) {
            return;
        }
    }
    // Else as coverage. The background, the text's colour, what each pixel is xor-ed with.
    let dark = (ink.0 >> 16 & 255) * 299 + (ink.0 >> 8 & 255) * 587 + (ink.0 & 255) * 114 < 128_000;
    let how = if dark { [0x00ff_ffff, 0, 0x00ff_ffff, 0] } else { [0, 0x00ff_ffff, 0, 0] };
    let mut hash = Fast(0x7275_6e73);
    (font.face, px, units, dark).hash(&mut hash);
    list.run(hash.finish(), corner, whole, ink, clip, |texels| drawn(strip, handle, units, pad, whole, how, texels));
}

/// Has GDI draw `units` in `handle`, `pad` in from the top left, on a background `whole` big;
/// and reads the pixels back into `texels`. `how` is the background, the text's colour, and
/// what each pixel is xor-ed and or-ed with. A run larger than the strip is drawn a strip-sized
/// tile at a time.
fn drawn(strip: &Strip, handle: HANDLE, units: &[u16], pad: [i32; 2], whole: [i32; 2], how: [u32; 4], texels: &mut [u32]) {
    let ([wide, high], [background, text, flip, set]) = (whole.map(|side| side as usize), how);
    let [tile_wide, tile_high] = [strip.width as usize, strip.rows as usize];
    // SAFETY: the strip holds `rows` rows of `width` pixels; nothing else uses them now.
    let pixels = unsafe { std::slice::from_raw_parts_mut(strip.pixels, tile_wide * tile_high) };
    let across = wide.div_ceil(tile_wide);
    for tile in 0..across * high.div_ceil(tile_high) {
        let (left, top) = (tile % across * tile_wide, tile / across * tile_high);
        let (w, h) = (tile_wide.min(wide - left), tile_high.min(high - top));
        pixels.chunks_exact_mut(tile_wide).take(h).for_each(|row| row[..w].fill(background));
        // SAFETY: the strip's DC draws into those pixels; its state is restored after; the
        // text is as long as said.
        unsafe {
            let saved = SaveDC(strip.dc);
            SetViewportOrgEx(strip.dc, 0, 0, null_mut());
            IntersectClipRect(strip.dc, 0, 0, w as i32, h as i32);
            SelectObject(strip.dc, handle);
            SetBkMode(strip.dc, 1);
            SetTextColor(strip.dc, text);
            TextOutW(strip.dc, pad[0] - left as i32, pad[1] - top as i32, units.as_ptr(), units.len() as i32);
            RestoreDC(strip.dc, saved);
            GdiFlush();
        }
        let rows = pixels.chunks_exact(tile_wide).zip(texels[top * wide..].chunks_exact_mut(wide)).take(h);
        rows.for_each(|(row, texels)| texels[left..left + w].iter_mut().zip(row).for_each(|(texel, pixel)| *texel = ((pixel ^ flip) & 0x00ff_ffff) | set));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::gpu_draw::tests::{Scene, compare};

    /// `D3D11CreateDevice`, as d3d11.h declares it.
    type MakeDevice = unsafe extern "system" fn(Com, u32, isize, u32, *const u32, u32, u32, *mut Com, *mut u32, *mut Com) -> i32;

    /// A WARP device and its context: Direct3D 11 on the CPU, the same everywhere.
    fn warp() -> (Com, Com) {
        let (mut device, mut context, mut level) = (null_mut(), null_mut(), 0);
        let found = function_in(system_library("d3d11.dll"), c"D3D11CreateDevice");
        // SAFETY: as d3d11.h declares it; WARP (5), single-threaded with BGRA (0x21).
        let made = unsafe { std::mem::transmute::<usize, MakeDevice>(found)(null_mut(), 5, 0, 0x21, null(), 0, 7, &mut device, &mut level, &mut context) };
        assert!(made >= 0, "WARP: {made:#x}");
        (device, context)
    }

    /// Draws `list` on `device` into a new texture `size` pixels, first filled with `under`,
    /// and reads it back, row after row.
    fn drawn(device: Com, context: Com, pipeline: &mut Pipeline, list: &DrawList, size: [i32; 2], under: &[u32]) -> Vec<u32> {
        let [width, height] = size.map(|side| side as u32);
        let described = |usage, bind_flags, cpu_access_flags| TextureDescription {
            width,
            height,
            mip_levels: 1,
            array_size: 1,
            format: B8G8R8A8_UNORM,
            sample_count: 1,
            sample_quality: 0,
            usage,
            bind_flags,
            cpu_access_flags,
            misc_flags: 0,
        };
        let (mut texture, mut staging, mut mapped) = (null_mut(), null_mut(), crate::layer_d3d11::Pixels { first: null(), row_pitch: 0, slice_pitch: 0 });
        type MakeTexture = unsafe extern "system" fn(Com, *const TextureDescription, *const c_void, *mut Com) -> i32;
        unsafe {
            assert!(function::<MakeTexture>(device, 5)(device, &described(0, 0x20, 0), null(), &mut texture) >= 0);
            let all = Box3 { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
            function::<Put>(context, 48)(context, texture, 0, &all, under.as_ptr().cast(), width * 4, 0);
            let target = Pipeline::target(device, texture);
            assert!(pipeline.draw(device, context, target, list, [0, 0, size[0], size[1]], size));
            release(target);
            assert!(function::<MakeTexture>(device, 5)(device, &described(3, 0, 0x20000), null(), &mut staging) >= 0);
            function::<unsafe extern "system" fn(Com, Com, Com)>(context, 47)(context, staging, texture);
            assert!(function::<unsafe extern "system" fn(Com, Com, u32, u32, u32, *mut crate::layer_d3d11::Pixels) -> i32>(context, 14)(context, staging, 0, 1, 0, &mut mapped) >= 0);
            let rows = (0..height as usize).map(|row| std::slice::from_raw_parts(mapped.first.cast::<u8>().add(row * mapped.row_pitch as usize).cast::<u32>(), width as usize));
            let pixels = rows.flatten().copied().collect();
            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 15)(context, staging, 0);
            [staging, texture].into_iter().for_each(|object| release(object));
            pixels
        }
    }

    #[test]
    fn direct3d_11_draws_every_kind_of_thing_as_the_cpu_does() {
        let (device, context) = warp();
        let mut pipeline = Pipeline::new(device).expect("the shaders compile and the states are made");
        for scale in [1.0, 1.5, 2.0] {
            let (mut scene, mut list) = (Scene::new(scale), DrawList::default());
            scene.record(&mut list);
            assert!(!list.failed, "all of it on the GPU");
            let gpu = drawn(device, context, &mut pipeline, &list, scene.size, &scene.under);
            compare(&scene, &gpu, &format!("Direct3D 11 (WARP) at {scale}"));
        }
        unsafe { [context, device].into_iter().for_each(|object| release(object)) };
    }
}
