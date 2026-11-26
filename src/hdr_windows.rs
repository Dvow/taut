//! HDR on Windows (the feature `hdr`, see `hdr`): the screen a window is on, for every GPU
//! path (`Hdr`), and windows lit on an HDR screen presented in scRGB by Direct3D 11 and 12
//! (Vulkan's and OpenGL's passes are `hdr_vulkan`'s and `hdr_opengl`'s).
//!
//! - The screen: DXGI's output under the window (`IDXGIOutput6::GetDesc1`: whether HDR is on,
//!   its peak), and its SDR white from Windows' display settings (`DisplayConfigGetDeviceInfo`).
//!   Read again once the window is on another screen, or Windows says screens or settings
//!   changed.
//! - While something is lit there, the swap chain is `R16G16B16A16_FLOAT` in
//!   `DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709` (`ResizeBuffers`, `SetColorSpace1`); else as
//!   ever, `B8G8R8A8_UNORM`, the same pixels.
//! - In scRGB the window's texture reaches the back buffer through a pixel shader, not a copy:
//!   the SDR colour made linear, offset, times each colour's gain (capped at the peak), offset
//!   back, times SDR white. The gains are a texture (`R16G16B16A16_FLOAT`) the plane's changed
//!   part goes up to.
//! - The shaders are compiled when first needed by `d3dcompiler_47.dll`: `vs_4_0` and `ps_4_0`
//!   for Direct3D 11; `vs_5_1`, `ps_5_1` and a root signature for 12.
use crate::gpu_windows::Shown;
use crate::hdr::{Gains, Screen};
use crate::layer_d3d11::{B8G8R8A8_UNORM, Box3, Com, TextureDescription, function, release};
use crate::ui::Ui;
use crate::windows::sys::{HANDLE, InvalidateRect, MonitorFromWindow, RECT};
use crate::windows::{function_in, named, system_library};
use std::ffi::{CStr, c_char, c_void};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDisplayConfigBufferSizes(flags: u32, paths: *mut u32, modes: *mut u32) -> i32;
    fn QueryDisplayConfig(flags: u32, paths: *mut u32, path_info: *mut [u32; 18], modes: *mut u32, mode_info: *mut [u32; 16], topology: *mut u32) -> i32;
    fn DisplayConfigGetDeviceInfo(packet: *mut u32) -> i32;
}

/// `D3DCompile`, as d3dcompiler.h declares it.
type Compile = unsafe extern "system" fn(*const c_void, usize, *const c_char, *const c_void, *const c_void, *const c_char, *const c_char, u32, u32, *mut Com, *mut Com) -> i32;
type Ask = unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32;

// Formats and colour spaces, as dxgiformat.h and dxgicommon.h have them.
pub(crate) const FLOATS: u32 = 10;
const LINEAR_709: u32 = 1;
const SRGB_709: u32 = 0;
const PQ_2020: u32 = 12;
/// The interfaces asked for: `IDXGIFactory1`, `IDXGIOutput6`, `IDXGISwapChain3`.
const FACTORY: [u32; 4] = named(0x770A_AE78, 0xF26F, 0x4DBA, [0xA8, 0x29, 0x25, 0x3C, 0x83, 0xD1, 0xB3, 0x87]);
const OUTPUT6: [u32; 4] = named(0x0683_46E8, 0xAAEC, 0x4B84, [0xAD, 0xD7, 0x13, 0x7F, 0x51, 0x3F, 0x77, 0xA1]);
const CHAIN3: [u32; 4] = named(0x94D9_9BDB, 0xF1F8, 0x4AB0, [0xB2, 0x36, 0x7D, 0xA0, 0x17, 0x0E, 0xDA, 0xB1]);

/// The pass from the window's texture to an scRGB back buffer. `white` is SDR white in scRGB
/// (nits over 80), `most` the most a gain may be (peak over SDR white). A pixel's gains are
/// red, green and blue, and its gain map's offset.
const SHADERS: &str = r#"
Texture2D<float4> picture : register(t0);
Texture2D<float4> gains : register(t1);
cbuffer Light : register(b0) { float white; float most; float2 unused; };
float4 corner(uint at : SV_VertexID) : SV_Position {
    float2 p = float2((at << 1) & 2, at & 2);
    return float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1);
}
float4 pixel(float4 at : SV_Position) : SV_Target {
    int3 place = int3(at.xy, 0);
    float3 c = picture.Load(place).rgb;
    float3 bent = pow((c + 0.055) / 1.055, 2.4);
    float3 shown = float3(c.r <= 0.04045 ? c.r / 12.92 : bent.r, c.g <= 0.04045 ? c.g / 12.92 : bent.g, c.b <= 0.04045 ? c.b / 12.92 : bent.b);
    // (Gains are never 0: 0 lies outside them, where none is.)
    float4 lit = gains.Load(place);
    float3 gain = min(lit.rgb > 0 ? lit.rgb : 1, most);
    return float4(max((shown + lit.a) * gain - lit.a, 0) * white, 1);
}
"#;
/// Direct3D 12's root signature: the two textures (t0, t1) in a table, the light (b0) as four
/// constants.
const SIGNATURE: &str = "#define ROOT \"DescriptorTable(SRV(t0, numDescriptors = 2)), RootConstants(num32BitConstants = 4, b0)\"\n";

/// How often Windows said screens or settings changed: each window's screen is read again.
static SCREENS: AtomicU32 = AtomicU32::new(0);

/// Windows said screens or their settings changed: every window's screen is read again.
pub(crate) fn screens_changed() {
    SCREENS.fetch_add(1, Ordering::Relaxed);
}

/// `DXGI_OUTPUT_DESC1`.
#[repr(C)]
struct OutputDescription {
    name: [u16; 32],
    place: RECT,
    attached: i32,
    rotation: u32,
    monitor: HANDLE,
    bits: u32,
    colors: u32,
    primaries: [f32; 8],
    least: f32,
    most: f32,
    most_whole: f32,
}

/// The screen showing `monitor`: whether HDR is on there, its SDR white and peak. An SDR screen
/// where DXGI does not say.
pub(crate) fn screen_of(monitor: HANDLE) -> Screen {
    let Some(output) = output_of(monitor).filter(|output| output.colors == PQ_2020) else { return Screen::SDR };
    let white = white_of(&output.name).unwrap_or(80.0);
    // (A screen that names no peak is taken as 1,000 nits.)
    Screen { hdr: true, white, peak: if output.most > white { output.most } else { white.max(1000.0) } }
}

/// What DXGI says of the output showing `monitor`; nothing where it has none.
fn output_of(monitor: HANDLE) -> Option<OutputDescription> {
    let (made, mut factory) = (function_in(system_library("dxgi.dll"), c"CreateDXGIFactory1"), null_mut());
    // SAFETY: `CreateDXGIFactory1` as dxgi.h declares it; the factory is released.
    unsafe {
        if made == 0 || std::mem::transmute::<usize, unsafe extern "system" fn(*const [u32; 4], *mut Com) -> i32>(made)(&FACTORY, &mut factory) < 0 {
            return None;
        }
        let found = (0..16).find_map(|at| on_adapter(factory, at, monitor));
        release(factory);
        found
    }
}

/// What DXGI says of the output showing `monitor` on the factory's adapter `at`, if it is there.
fn on_adapter(factory: Com, at: u32, monitor: HANDLE) -> Option<OutputDescription> {
    let mut adapter = null_mut();
    // SAFETY: the factory's EnumAdapters1 (12); the adapter's EnumOutputs (7), each output asked
    // for `IDXGIOutput6` (0) and its GetDesc1 (27). Everything got is released.
    unsafe {
        if function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(factory, 12)(factory, at, &mut adapter) < 0 {
            return None;
        }
        let mut found = None;
        for at in 0..16 {
            let (mut output, mut six) = (null_mut(), null_mut());
            if function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(adapter, 7)(adapter, at, &mut output) < 0 {
                break;
            }
            function::<Ask>(output, 0)(output, &OUTPUT6, &mut six);
            let described = (!six.is_null()).then(|| described(six)).flatten();
            [six, output].into_iter().for_each(|object| release(object));
            found = found.or(described.filter(|described| described.monitor == monitor));
        }
        release(adapter);
        found
    }
}

/// An `IDXGIOutput6`'s description (GetDesc1, 27).
///
/// # Safety
/// `output` is a live `IDXGIOutput6`.
unsafe fn described(output: Com) -> Option<OutputDescription> {
    let mut said = std::mem::MaybeUninit::<OutputDescription>::zeroed();
    // SAFETY: as the caller promises; the description is all numbers, whole once filled in.
    unsafe { (function::<unsafe extern "system" fn(Com, *mut OutputDescription) -> i32>(output, 27)(output, said.as_mut_ptr()) >= 0).then(|| said.assume_init()) }
}

/// Whether any screen is HDR now.
pub(crate) fn an_hdr_screen() -> bool {
    unsafe extern "system" fn each(monitor: HANDLE, _: HANDLE, _: *mut RECT, found: isize) -> i32 {
        // SAFETY: `found` is the caller's flag, alive through the enumeration.
        unsafe { *(found as *mut bool) |= screen_of(monitor).hdr };
        1
    }
    #[cfg(test)]
    if tests::AN_HDR_SCREEN.load(Ordering::Relaxed) {
        return true;
    }
    let mut found = false;
    // SAFETY: the callback writes the flag, which outlives the call.
    unsafe { crate::windows::sys::EnumDisplayMonitors(0, null(), each, (&raw mut found) as isize) };
    found
}

/// The SDR white Windows' display settings give the screen of GDI name `name` (`\\.\DISPLAY1`),
/// in nits.
fn white_of(name: &[u16; 32]) -> Option<f32> {
    let (mut paths, mut modes) = (0u32, 0u32);
    // SAFETY: `GetDisplayConfigBufferSizes` and `QueryDisplayConfig` for the active paths (2),
    // into room for as many as the first said; each packet is as wingdi.h lays it out:
    // `DISPLAYCONFIG_DEVICE_INFO_HEADER` (type, size, adapter, id), then its answer.
    unsafe {
        if GetDisplayConfigBufferSizes(2, &mut paths, &mut modes) != 0 {
            return None;
        }
        let (mut path_info, mut mode_info) = (vec![[0u32; 18]; paths as usize], vec![[0u32; 16]; modes as usize]);
        if QueryDisplayConfig(2, &mut paths, path_info.as_mut_ptr(), &mut modes, mode_info.as_mut_ptr(), null_mut()) != 0 {
            return None;
        }
        path_info.truncate(paths as usize);
        // The path whose source has that name (`GET_SOURCE_NAME`, 1); its target's SDR white
        // (`GET_SDR_WHITE_LEVEL`, 11), in thousandths of 80 nits.
        let source = |path: &[u32; 18]| {
            let mut packet = [0u32; 21];
            packet[..5].copy_from_slice(&[1, 84, path[0], path[1], path[2]]);
            DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) == 0 && packet[5..].iter().flat_map(|two| [*two as u16, (*two >> 16) as u16]).eq(name.iter().copied())
        };
        let path = path_info.iter().find(|path| source(path))?;
        let mut packet = [11, 24, path[5], path[6], path[7], 0];
        (DisplayConfigGetDeviceInfo(packet.as_mut_ptr()) == 0 && packet[5] > 0).then(|| packet[5] as f32 / 1000.0 * 80.0)
    }
}

/// A window's HDR: its gains, its screen and which screen that was read for (with the count of
/// screen changes then), and whether it is presented in HDR.
#[derive(Debug, Default)]
pub(crate) struct Hdr {
    pub(crate) gains: Gains,
    pub(crate) screen: Screen,
    read: Option<(HANDLE, u32)>,
    pub(crate) on: bool,
}

impl Hdr {
    /// Reads window `hwnd`'s screen again where it is on another, or screens changed.
    fn follow(&mut self, hwnd: HANDLE) {
        // SAFETY: a plain query (`MONITOR_DEFAULTTONEAREST`, 2).
        let now = (unsafe { MonitorFromWindow(hwnd, 2) }, SCREENS.load(Ordering::Relaxed));
        if self.read == Some(now) {
            return;
        }
        self.read = Some(now);
        #[cfg(test)]
        if let Some(faked) = tests::faked(now.0) {
            self.screen = faked;
            return;
        }
        self.screen = screen_of(now.0);
    }

    /// Whether the window is to be presented in scRGB: lit, on an HDR screen.
    pub(crate) fn wanted(&self) -> bool {
        self.screen.hdr && self.gains.any()
    }

    /// Puts swap chain `chain` (`size` pixels) in scRGB where wanted and the path `can`, and back
    /// where not. Says whether that worked.
    pub(crate) fn switch(&mut self, chain: Com, size: [i32; 2], can: bool) -> bool {
        let want = self.wanted() && can;
        if want == self.on {
            return true;
        }
        let (mut three, [format, space]) = (null_mut(), if want { [FLOATS, LINEAR_709] } else { [B8G8R8A8_UNORM, SRGB_709] });
        // SAFETY: the chain's ResizeBuffers (13) while nothing holds its buffers (each present
        // releases its back buffer); as `IDXGISwapChain3` (0), its SetColorSpace1 (38).
        let done = unsafe {
            if function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, u32) -> i32>(chain, 13)(chain, 0, size[0].max(1) as u32, size[1].max(1) as u32, format, 0) < 0 {
                return false;
            }
            function::<Ask>(chain, 0)(chain, &CHAIN3, &mut three);
            let set = !three.is_null() && function::<unsafe extern "system" fn(Com, u32) -> i32>(three, 38)(three, space) >= 0;
            release(three);
            set
        };
        self.on = want;
        done
    }

    /// The four constants the pass takes: SDR white in scRGB, and the most a gain may be.
    pub(crate) fn constants(&self) -> [f32; 4] {
        [self.screen.white / 80.0, self.screen.headroom(), 0.0, 0.0]
    }
}

/// Takes in what lights a window `hwnd`, `size` pixels, shown by `shown`, after a run: its
/// screen read again where needed, its gains drawn again where they changed, and that part (or
/// all, where it goes in or out of scRGB, or its screen changed while it is lit) to be painted
/// and presented. Says whether the window's headroom changed: its app is to run again.
pub(crate) fn light(hwnd: HANDLE, shown: Option<&mut Shown>, size: [i32; 2], ui: &mut Ui) -> bool {
    let Some(hdr) = shown.and_then(Shown::hdr) else { return ui.set_screen(Screen::SDR) };
    let screen = hdr.screen;
    hdr.follow(hwnd);
    let news = ui.set_screen(hdr.screen);
    let (glows, was) = (if hdr.screen.hdr { ui.glows() } else { Vec::new() }, hdr.wanted());
    let part = hdr.gains.take(glows, size, &ui.light.maps).map(|[left, top, right, bottom]| RECT { left, top, right, bottom });
    // (Lit, its light and HDR metadata are its screen's: shown again whole when that changes.)
    let whole = was != hdr.wanted() || (was && hdr.screen != screen);
    if whole || part.is_some() {
        // SAFETY: the window is alive; the rectangle outlives the call.
        unsafe { InvalidateRect(hwnd, part.as_ref().filter(|_| !whole).map_or(null(), |part| part), 0) };
    }
    news
}

/// HLSL compiled for `target` (as `vs_4_0`), from function `entry`: its words; nothing where
/// d3dcompiler_47.dll is missing or the source doesn't compile.
fn compiled(source: &str, entry: &CStr, target: &CStr) -> Option<Vec<u32>> {
    let found = function_in(system_library("d3dcompiler_47.dll"), c"D3DCompile");
    let (mut code, mut said) = (null_mut(), null_mut());
    // SAFETY: `D3DCompile` as declared; the names end in zeros, the source is as long as said.
    // The blobs' GetBufferPointer (3) and GetBufferSize (4); both blobs are released.
    unsafe {
        if found == 0 {
            return None;
        }
        let made = std::mem::transmute::<usize, Compile>(found)(source.as_ptr().cast(), source.len(), null(), null(), null(), entry.as_ptr(), target.as_ptr(), 0, 0, &mut code, &mut said);
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

/// Direct3D 11's pass to scRGB: its shaders, the constants' buffer, and the gains' texture and
/// view, `size` pixels.
#[derive(Debug)]
pub(crate) struct Pass11 {
    shaders: [Com; 2],
    constants: Com,
    gains: [Com; 2],
    size: [i32; 2],
}

type MakeTexture = unsafe extern "system" fn(Com, *const TextureDescription, *const c_void, *mut Com) -> i32;
type MakeView = unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32;
type Put = unsafe extern "system" fn(Com, Com, u32, *const Box3, *const c_void, u32, u32);

impl Pass11 {
    /// The pass on `device` (`ID3D11Device`); nothing where it can't be made.
    pub(crate) fn new(device: Com) -> Option<Pass11> {
        let (corner, pixel) = (compiled(SHADERS, c"corner", c"vs_4_0")?, compiled(SHADERS, c"pixel", c"ps_4_0")?);
        let mut made = Pass11 { shaders: [null_mut(); 2], constants: null_mut(), gains: [null_mut(); 2], size: [0; 2] };
        // `D3D11_BUFFER_DESC`: 16 bytes, the default usage, a constant buffer (4).
        let buffer: [u32; 6] = [16, 0, 4, 0, 0, 0];
        type MakeShader = unsafe extern "system" fn(Com, *const u32, usize, Com, *mut Com) -> i32;
        // SAFETY: the device's CreateVertexShader (12), CreatePixelShader (15) and CreateBuffer
        // (3), from whole code and descriptions.
        unsafe {
            function::<MakeShader>(device, 12)(device, corner.as_ptr(), corner.len() * 4, null_mut(), &mut made.shaders[0]);
            function::<MakeShader>(device, 15)(device, pixel.as_ptr(), pixel.len() * 4, null_mut(), &mut made.shaders[1]);
            function::<unsafe extern "system" fn(Com, *const [u32; 6], *const c_void, *mut Com) -> i32>(device, 3)(device, &buffer, null(), &mut made.constants);
        }
        (![made.shaders[0], made.shaders[1], made.constants].iter().any(|made| made.is_null())).then_some(made)
    }

    /// Puts the gains that changed into their texture: all of them into a new one where the
    /// window has another size. Says whether there is one.
    fn put_up(&mut self, device: Com, context: Com, gains: &mut Gains) -> bool {
        let size = gains.size;
        if size != self.size {
            // SAFETY: held here alone.
            unsafe { self.gains.iter_mut().for_each(|object| release(std::mem::replace(object, null_mut()))) };
            let described = TextureDescription {
                width: size[0] as u32,
                height: size[1] as u32,
                mip_levels: 1,
                array_size: 1,
                format: FLOATS,
                sample_count: 1,
                sample_quality: 0,
                usage: 0,
                bind_flags: 8,
                cpu_access_flags: 0,
                misc_flags: 0,
            };
            // SAFETY: the device's CreateTexture2D (5) and CreateShaderResourceView (7), whole.
            unsafe { function::<MakeTexture>(device, 5)(device, &described, null(), &mut self.gains[0]) };
            if !self.gains[0].is_null() {
                // SAFETY: as above, of the texture just made.
                unsafe { function::<MakeView>(device, 7)(device, self.gains[0], null(), &mut self.gains[1]) };
            }
            (self.size, gains.changed) = (size, Some([0, 0, size[0], size[1]]));
        }
        if let Some([left, top, right, bottom]) = gains.changed.take().filter(|_| !self.gains[0].is_null()) {
            let place = Box3 { left: left as u32, top: top as u32, front: 0, right: right as u32, bottom: bottom as u32, back: 1 };
            let first = gains.plane[top as usize * size[0] as usize + left as usize..].as_ptr();
            // SAFETY: the box lies in the texture; the rows read lie in the plane, which is as
            // large. (The context's UpdateSubresource, 48.)
            unsafe { function::<Put>(context, 48)(context, self.gains[0], 0, &place, first.cast(), size[0] as u32 * 8, 0) };
        }
        !self.gains[1].is_null()
    }

    /// Draws `picture` (the window's texture) into `back` (an scRGB back buffer), both `size`
    /// pixels, lit by `hdr`'s gains. Says whether it could.
    pub(crate) fn draw(&mut self, [device, context]: [Com; 2], hdr: &mut Hdr, picture: Com, back: Com, size: [i32; 2]) -> bool {
        if !self.put_up(device, context, &mut hdr.gains) {
            return false;
        }
        let ([mut target, mut view], constants) = ([null_mut(); 2], hdr.constants());
        let viewport = [0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0];
        type SetShader = unsafe extern "system" fn(Com, Com, *const Com, u32);
        type Views = unsafe extern "system" fn(Com, u32, u32, *const Com);
        // SAFETY: the device's CreateRenderTargetView (9) and CreateShaderResourceView (7) of
        // whole textures; the context's methods by place in its vtable, each with what it takes:
        // 48 UpdateSubresource, 33 OMSetRenderTargets, 35 OMSetBlendState, 43 RSSetState, 44
        // RSSetViewports, 17 IASetInputLayout, 24 IASetPrimitiveTopology (a list, 4), 11
        // VSSetShader, 9 PSSetShader, 8 PSSetShaderResources, 16 PSSetConstantBuffers, 13 Draw.
        // The views are released, the textures unbound.
        unsafe {
            function::<MakeView>(device, 9)(device, back, null(), &mut target);
            function::<MakeView>(device, 7)(device, picture, null(), &mut view);
            if target.is_null() || view.is_null() {
                [target, view].into_iter().for_each(|object| release(object));
                return false;
            }
            function::<Put>(context, 48)(context, self.constants, 0, null(), constants.as_ptr().cast(), 0, 0);
            function::<unsafe extern "system" fn(Com, u32, *const Com, Com)>(context, 33)(context, 1, &target, null_mut());
            function::<unsafe extern "system" fn(Com, Com, *const f32, u32)>(context, 35)(context, null_mut(), null(), u32::MAX);
            function::<unsafe extern "system" fn(Com, Com)>(context, 43)(context, null_mut());
            function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(context, 44)(context, 1, &viewport);
            function::<unsafe extern "system" fn(Com, Com)>(context, 17)(context, null_mut());
            function::<unsafe extern "system" fn(Com, u32)>(context, 24)(context, 4);
            function::<SetShader>(context, 11)(context, self.shaders[0], null(), 0);
            function::<SetShader>(context, 9)(context, self.shaders[1], null(), 0);
            function::<Views>(context, 8)(context, 0, 2, [view, self.gains[1]].as_ptr());
            function::<Views>(context, 16)(context, 0, 1, &self.constants);
            function::<unsafe extern "system" fn(Com, u32, u32)>(context, 13)(context, 3, 0);
            function::<Views>(context, 8)(context, 0, 2, [null_mut(); 2].as_ptr());
            function::<unsafe extern "system" fn(Com, u32, *const Com, Com)>(context, 33)(context, 0, null(), null_mut());
            [target, view].into_iter().for_each(|object| release(object));
        }
        true
    }
}

impl Drop for Pass11 {
    fn drop(&mut self) {
        // SAFETY: each is held once, by this pass.
        unsafe { self.shaders.iter().chain(&self.gains).chain([&self.constants]).for_each(|object| release(*object)) };
    }
}

/// `D3D12_SHADER_BYTECODE`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Bytecode {
    code: *const u32,
    size: usize,
}

/// `D3D12_RENDER_TARGET_BLEND_DESC`.
#[repr(C)]
#[derive(Clone, Copy)]
struct TargetBlend {
    enable: i32,
    logic_enable: i32,
    numbers: [u32; 7],
    write_mask: u8,
}

/// `D3D12_GRAPHICS_PIPELINE_STATE_DESC`, as d3d12.h lays it out: the root signature; vertex,
/// pixel, domain, hull and geometry shaders; stream output (as four pointers); alpha to coverage
/// and independent blending; eight targets' blending; the sample mask; the rasterizer (eleven
/// numbers); depth and stencil (thirteen); the input layout; strip cut, topology, target count
/// and formats, depth format, samples, node mask, cached state, flags.
#[repr(C)]
struct PipelineDescription {
    root: Com,
    shaders: [Bytecode; 5],
    stream_output: [usize; 4],
    blends: [i32; 2],
    targets: [TargetBlend; 8],
    sample_mask: u32,
    rasterizer: [u32; 11],
    depth: [u32; 13],
    input_layout: [usize; 2],
    strip_cut: u32,
    topology: u32,
    target_count: u32,
    formats: [u32; 8],
    depth_format: u32,
    samples: [u32; 2],
    node_mask: u32,
    cached: [usize; 2],
    flags: u32,
}

/// Direct3D 12's pass to scRGB: its root signature and pipeline state, a shader-seen heap of the
/// two textures' views and a heap of the target's; the gains' texture (`size` pixels, ready for
/// shaders once `ready`) and the upload buffer they go up through, with the part waiting there.
#[derive(Debug)]
pub(crate) struct Pass12 {
    root: Com,
    pipeline: Com,
    views: Com,
    targets: Com,
    step: usize,
    gains: Com,
    upload: Option<(Com, *mut u8)>,
    size: [i32; 2],
    ready: bool,
    waiting: Option<[i32; 4]>,
}

// SAFETY: as its device: made and used on the windows' thread alone.
unsafe impl Send for Pass12 {}

/// The interfaces Direct3D 12 makes here.
const ROOT: [u32; 4] = named(0xC54A_6B66, 0x72DF, 0x4EE8, [0x8B, 0xE5, 0xA9, 0x46, 0xA1, 0x42, 0x92, 0x14]);
const PIPELINE: [u32; 4] = named(0x765A_30F3, 0xF624, 0x4C6F, [0xA8, 0x28, 0xAC, 0xE9, 0x48, 0x62, 0x24, 0x45]);
const HEAP: [u32; 4] = named(0x8EFB_471D, 0x616C, 0x4F49, [0x90, 0xF7, 0x12, 0x7B, 0xB7, 0x63, 0xFA, 0x51]);
// Resource states, as d3d12.h has them.
const RENDER_TARGET: u32 = 0x4;
const SHADER_READS: u32 = 0x80;
const COPIED_TO: u32 = 0x400;
const PRESENT: u32 = 0;
/// Rows copied from a buffer start a multiple of this apart.
const PITCH: usize = 256;

/// A descriptor heap of `kind` (0 views, 2 render targets) holding `count`, seen by shaders if
/// `seen`.
fn heap(device: Com, kind: u32, count: u32, seen: bool) -> Com {
    let (described, mut made) = ([kind, count, seen as u32, 0], null_mut());
    // SAFETY: the device's CreateDescriptorHeap (14) from a whole description.
    unsafe { function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 14)(device, &described, &HEAP, &mut made) };
    made
}

/// Where `heap`'s descriptors start: for the CPU (9), or for the GPU (10).
fn start_of(heap: Com, at: usize) -> u64 {
    let mut start = 0u64;
    // SAFETY: the heap's GetCPU/GPUDescriptorHandleForHeapStart, its result through a pointer.
    unsafe { function::<unsafe extern "system" fn(Com, *mut u64) -> *mut u64>(heap, at)(heap, &mut start) };
    start
}

impl Pass12 {
    /// The pass on `device` (`ID3D12Device`); nothing where it can't be made.
    pub(crate) fn new(device: Com) -> Option<Pass12> {
        let (corner, pixel) = (compiled(SHADERS, c"corner", c"vs_5_1")?, compiled(SHADERS, c"pixel", c"ps_5_1")?);
        let signature = compiled(SIGNATURE, c"ROOT", c"rootsig_1_0")?;
        let mut made = Pass12 { root: null_mut(), pipeline: null_mut(), views: null_mut(), targets: null_mut(), step: 0, gains: null_mut(), upload: None, size: [0; 2], ready: false, waiting: None };
        type MakeRoot = unsafe extern "system" fn(Com, u32, *const u32, usize, *const [u32; 4], *mut Com) -> i32;
        // SAFETY: the device's CreateRootSignature (16) from compiled words, and
        // GetDescriptorHandleIncrementSize (15) of views (0).
        unsafe {
            function::<MakeRoot>(device, 16)(device, 0, signature.as_ptr(), signature.len() * 4, &ROOT, &mut made.root);
            made.step = function::<unsafe extern "system" fn(Com, u32) -> u32>(device, 15)(device, 0) as usize;
        }
        (made.views, made.targets) = (heap(device, 0, 2, true), heap(device, 2, 1, false));
        made.pipeline = if made.root.is_null() { null_mut() } else { pipeline(device, made.root, &corner, &pixel) };
        (![made.root, made.pipeline, made.views, made.targets].iter().any(|made| made.is_null())).then_some(made)
    }

    /// Readies the gains for `record`: the part that changed written to the upload buffer (all
    /// of them, into a new texture, where the window has another size). Says whether they are
    /// ready. (The device is done with both: each frame is waited for.)
    pub(crate) fn prepare(&mut self, device: Com, gains: &mut Gains) -> bool {
        let size = gains.size;
        if size != self.size {
            self.release_gains();
            let row = (size[0] as usize * 8).next_multiple_of(PITCH);
            self.gains = crate::gpu_d3d12::texture_of(device, size, FLOATS).unwrap_or(null_mut());
            self.upload = crate::gpu_d3d12::buffer(device, 2, row * size[1] as usize, 0xAC3);
            (self.size, self.ready, gains.changed) = (size, false, Some([0, 0, size[0], size[1]]));
        }
        let Some((_, written)) = self.upload.filter(|_| !self.gains.is_null()) else { return false };
        self.waiting = gains.changed.take();
        let Some([left, top, right, bottom]) = self.waiting else { return true };
        let row = ((right - left) as usize * 8).next_multiple_of(PITCH);
        for y in top..bottom {
            let from = &gains.plane[y as usize * size[0] as usize + left as usize..][..(right - left) as usize];
            // SAFETY: the buffer holds a row of the whole window for each of its rows.
            unsafe { std::ptr::copy_nonoverlapping(from.as_ptr().cast::<u8>(), written.add((y - top) as usize * row), from.len() * 8) };
        }
        true
    }

    /// Releases the gains' texture and upload buffer.
    fn release_gains(&mut self) {
        let upload = self.upload.take().map_or(null_mut(), |(buffer, _)| buffer);
        // SAFETY: each held once, here; the device is done with them.
        unsafe { [std::mem::replace(&mut self.gains, null_mut()), upload].into_iter().for_each(|object| release(object)) };
    }

    /// Records into `list` (open) the gains waiting copied up, and `picture` (the window's
    /// texture, copied into before and after) drawn into `back` (an scRGB back buffer, presented
    /// before and after), both `size` pixels, with `constants`.
    pub(crate) fn record(&self, device: Com, list: Com, picture: Com, back: Com, size: [i32; 2], constants: [f32; 4]) {
        use crate::gpu_d3d12::{Barrier, barrier};
        self.copy_waiting(list);
        let (target, views, viewport) = (start_of(self.targets, 9) as usize, start_of(self.views, 9) as usize, [0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0]);
        let ([before, after], gains) = (
            [[barrier(picture, COPIED_TO, SHADER_READS), barrier(back, PRESENT, RENDER_TARGET)], [barrier(picture, SHADER_READS, COPIED_TO), barrier(back, RENDER_TARGET, PRESENT)]],
            barrier(self.gains, COPIED_TO, SHADER_READS),
        );
        type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
        type View = unsafe extern "system" fn(Com, Com, *const c_void, usize);
        // SAFETY: the device's CreateShaderResourceView (18) and CreateRenderTargetView (20) of
        // whole textures into their heaps; the list's methods by place in its vtable, each with
        // what it takes: 26 ResourceBarrier, 30 SetGraphicsRootSignature, 25 SetPipelineState,
        // 28 SetDescriptorHeaps, 32 SetGraphicsRootDescriptorTable, 36
        // SetGraphicsRoot32BitConstants, 20 IASetPrimitiveTopology (a list, 4), 21
        // RSSetViewports, 22 RSSetScissorRects, 46 OMSetRenderTargets, 12 DrawInstanced.
        unsafe {
            function::<View>(device, 18)(device, picture, null(), views);
            function::<View>(device, 18)(device, self.gains, null(), views + self.step);
            function::<View>(device, 20)(device, back, null(), target);
            if !self.ready || self.waiting.is_some() {
                function::<Barriers>(list, 26)(list, 1, &gains);
            }
            function::<Barriers>(list, 26)(list, 2, before.as_ptr());
            function::<unsafe extern "system" fn(Com, Com)>(list, 30)(list, self.root);
            function::<unsafe extern "system" fn(Com, Com)>(list, 25)(list, self.pipeline);
            function::<unsafe extern "system" fn(Com, u32, *const Com)>(list, 28)(list, 1, &self.views);
            function::<unsafe extern "system" fn(Com, u32, u64)>(list, 32)(list, 0, start_of(self.views, 10));
            function::<unsafe extern "system" fn(Com, u32, u32, *const f32, u32)>(list, 36)(list, 1, 4, constants.as_ptr(), 0);
            function::<unsafe extern "system" fn(Com, u32)>(list, 20)(list, 4);
            function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(list, 21)(list, 1, &viewport);
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(list, 22)(list, 1, &[0, 0, size[0], size[1]]);
            function::<unsafe extern "system" fn(Com, u32, *const usize, i32, *const usize)>(list, 46)(list, 1, &target, 0, null());
            function::<unsafe extern "system" fn(Com, u32, u32, u32, u32)>(list, 12)(list, 3, 1, 0, 0);
            #[cfg(test)]
            tests::record_kept(list, back, size);
            function::<Barriers>(list, 26)(list, 2, after.as_ptr());
        }
    }

    /// Records the waiting gains copied from the upload buffer into their texture (made ready to
    /// be copied into first, where it was ready for shaders).
    fn copy_waiting(&self, list: Com) {
        use crate::gpu_d3d12::{Barrier, Place, barrier};
        let (Some([left, top, right, bottom]), Some((upload, _))) = (self.waiting, self.upload) else { return };
        let row = ((right - left) as usize * 8).next_multiple_of(PITCH) as u32;
        let to = Place { resource: self.gains, kind: 0, offset: 0, footprint: [0; 5] };
        let from = Place { resource: upload, kind: 1, offset: 0, footprint: [FLOATS, (right - left) as u32, (bottom - top) as u32, 1, row] };
        // SAFETY: the list's ResourceBarrier (26) and CopyTextureRegion (16): the footprint lies
        // in the buffer as `prepare` wrote it, the place in the texture.
        unsafe {
            if self.ready {
                function::<unsafe extern "system" fn(Com, u32, *const Barrier)>(list, 26)(list, 1, &barrier(self.gains, SHADER_READS, COPIED_TO));
            }
            function::<unsafe extern "system" fn(Com, *const Place, u32, u32, u32, *const Place, *const Box3)>(list, 16)(list, &to, left as u32, top as u32, 0, &from, null());
        }
    }

    /// Notes that what `record` recorded was run: the gains' texture is ready for shaders.
    pub(crate) fn ran(&mut self) {
        (self.ready, self.waiting) = (true, None);
    }
}

impl Drop for Pass12 {
    fn drop(&mut self) {
        self.release_gains();
        // SAFETY: each is held once, by this pass; the device is done with them.
        unsafe { [self.pipeline, self.root, self.views, self.targets].into_iter().for_each(|object| release(object)) };
    }
}

/// The pipeline state: one triangle over the target, nothing culled, no depth, not blended, into
/// an scRGB target.
fn pipeline(device: Com, root: Com, corner: &[u32], pixel: &[u32]) -> Com {
    let none = Bytecode { code: null(), size: 0 };
    // Not blended: one, zero, added, for colour and alpha (2, 1, 1, 2, 1, 1); logic "no op" (4)
    // off; all four channels written.
    let target = TargetBlend { enable: 0, logic_enable: 0, numbers: [2, 1, 1, 2, 1, 1, 4], write_mask: 0xf };
    let described = PipelineDescription {
        root,
        shaders: [Bytecode { code: corner.as_ptr(), size: corner.len() * 4 }, Bytecode { code: pixel.as_ptr(), size: pixel.len() * 4 }, none, none, none],
        stream_output: [0; 4],
        blends: [0; 2],
        targets: [target; 8],
        sample_mask: u32::MAX,
        // Solid (3), nothing culled (1), depth clip on.
        rasterizer: [3, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0],
        // Depth and stencil off, compared "always" (8), all bits, kept (1).
        depth: [0, 0, 8, 0, 0xffff, 1, 1, 1, 8, 1, 1, 1, 8],
        input_layout: [0; 2],
        strip_cut: 0,
        topology: 3,
        target_count: 1,
        formats: [FLOATS, 0, 0, 0, 0, 0, 0, 0],
        depth_format: 0,
        samples: [1, 0],
        node_mask: 0,
        cached: [0; 2],
        flags: 0,
    };
    let mut made = null_mut();
    type Make = unsafe extern "system" fn(Com, *const PipelineDescription, *const [u32; 4], *mut Com) -> i32;
    // SAFETY: the device's CreateGraphicsPipelineState (10); the description and the code it
    // points to are whole.
    unsafe { function::<Make>(device, 10)(device, &described, &PIPELINE, &mut made) };
    made
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::gpu_windows::tests::{Faked, fake};
    use crate::hdr::from_half;
    use crate::platform::Renderer;
    use crate::windows::sys::*;
    use crate::windows::tests::{ALONE, is_on_its_own_desktop, on_a_desktop_of_its_own, pump};
    use crate::{Color, Flow, Picture, Window};
    use std::cell::{Cell, RefCell};

    /// A test's process has an HDR screen, as every thread sees it (a GPU device is made on a
    /// thread of its own).
    pub(crate) static AN_HDR_SCREEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    thread_local! {
        /// The screens as a test has them: the first (holding the desktop's top left corner),
        /// and every other.
        static FAKED: Cell<Option<(Screen, Screen)>> = const { Cell::new(None) };
        /// What scRGB presents showed: each frame's row length and pixels (red, green, blue,
        /// alpha), last last.
        static KEPT: RefCell<Vec<(usize, Vec<[f32; 4]>)>> = const { RefCell::new(Vec::new()) };
    }

    /// The first screen.
    fn first_screen() -> HANDLE {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MonitorFromPoint(at: POINT, otherwise: u32) -> HANDLE;
        }
        unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, 1) }
    }

    /// The screen `monitor` is, as a test has it.
    pub(crate) fn faked(monitor: HANDLE) -> Option<Screen> {
        let (first, others) = FAKED.get()?;
        Some(if monitor == first_screen() { first } else { others })
    }

    #[test]
    fn the_structures_are_laid_out_as_windows_lays_them_out() {
        assert_eq!(size_of::<OutputDescription>(), 152);
        assert_eq!(std::mem::offset_of!(OutputDescription, monitor), 88);
        assert_eq!(std::mem::offset_of!(OutputDescription, most), 140);
        assert_eq!(size_of::<PipelineDescription>(), 656);
        assert_eq!(std::mem::offset_of!(PipelineDescription, formats), 580);
        assert_eq!(size_of::<TargetBlend>(), 40);
    }

    #[test]
    fn the_shaders_compile_for_both_paths() {
        for (entry, target) in [(c"corner", c"vs_4_0"), (c"pixel", c"ps_4_0"), (c"corner", c"vs_5_1"), (c"pixel", c"ps_5_1")] {
            assert!(compiled(SHADERS, entry, target).is_some(), "{entry:?} for {target:?}");
        }
        assert!(compiled(SIGNATURE, c"ROOT", c"rootsig_1_0").is_some(), "the root signature");
    }

    #[test]
    fn this_computers_screens_say_whether_they_are_hdr() {
        // Read only: what each screen is, as a window on it would hear.
        unsafe extern "system" fn each(monitor: HANDLE, _: HANDLE, _: *mut RECT, _: isize) -> i32 {
            println!("{monitor:#x}: {:?}", screen_of(monitor));
            1
        }
        let start = std::time::Instant::now();
        unsafe { EnumDisplayMonitors(0, null(), each, 0) };
        println!("read in {:?}", start.elapsed());
        assert_eq!(screen_of(0), Screen::SDR, "no screen, no HDR");
    }

    /// Keeps a copy of an scRGB back buffer about to be presented by Direct3D 11, if a test asked.
    pub(crate) fn keep(device: Com, context: Com, back: Com, size: [i32; 2]) {
        if !crate::gpu_windows::tests::faked().keep {
            return;
        }
        let [width, height] = size.map(|side| side as u32);
        let described =
            TextureDescription { width, height, mip_levels: 1, array_size: 1, format: FLOATS, sample_count: 1, sample_quality: 0, usage: 3, bind_flags: 0, cpu_access_flags: 0x20000, misc_flags: 0 };
        let (mut staging, mut mapped) = (null_mut(), crate::layer_d3d11::Pixels { first: null(), row_pitch: 0, slice_pitch: 0 });
        unsafe {
            assert!(function::<MakeTexture>(device, 5)(device, &described, null(), &mut staging) >= 0);
            function::<unsafe extern "system" fn(Com, Com, Com)>(context, 47)(context, staging, back);
            assert!(function::<unsafe extern "system" fn(Com, Com, u32, u32, u32, *mut crate::layer_d3d11::Pixels) -> i32>(context, 14)(context, staging, 0, 1, 0, &mut mapped) >= 0);
            let rows = (0..height as usize).map(|row| std::slice::from_raw_parts(mapped.first.cast::<u8>().add(row * mapped.row_pitch as usize).cast::<[u16; 4]>(), width as usize));
            let pixels = rows.flatten().map(|pixel| pixel.map(from_half)).collect();
            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 15)(context, staging, 0);
            release(staging);
            KEPT.with_borrow_mut(|kept| kept.push((width as usize, pixels)));
        }
    }

    thread_local! {
        /// Direct3D 12's copy of its last scRGB back buffer: the buffer read back, and its size.
        static READ_BACK: RefCell<Option<(Com, *mut u8, [i32; 2])>> = const { RefCell::new(None) };
    }

    /// Records a copy of an scRGB back buffer (being drawn into) into a buffer read back, if a
    /// test asked; `read_kept` reads it once the frame ran.
    pub(crate) fn record_kept(list: Com, back: Com, size: [i32; 2]) {
        use crate::gpu_d3d12::{Barrier, Place, barrier};
        if !crate::gpu_windows::tests::faked().keep {
            return;
        }
        let mut device = null_mut();
        let iid = named(0x1898_19F1, 0x1DB6, 0x4B57, [0xBE, 0x54, 0x18, 0x21, 0x33, 0x9B, 0x85, 0xF7]);
        unsafe { function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(list, 7)(list, &iid, &mut device) };
        let pitch = (size[0] as usize * 8).next_multiple_of(PITCH);
        let (buffer, there) = crate::gpu_d3d12::buffer(device, 3, pitch * size[1] as usize, COPIED_TO).expect("a buffer to read back");
        unsafe { release(device) };
        let to = Place { resource: buffer, kind: 1, offset: 0, footprint: [FLOATS, size[0] as u32, size[1] as u32, 1, pitch as u32] };
        let from = Place { resource: back, kind: 0, offset: 0, footprint: [0; 5] };
        type Barriers = unsafe extern "system" fn(Com, u32, *const Barrier);
        unsafe {
            function::<Barriers>(list, 26)(list, 1, &barrier(back, RENDER_TARGET, 0x800));
            function::<unsafe extern "system" fn(Com, *const Place, u32, u32, u32, *const Place, *const Box3)>(list, 16)(list, &to, 0, 0, 0, &from, null());
            function::<Barriers>(list, 26)(list, 1, &barrier(back, 0x800, RENDER_TARGET));
        }
        READ_BACK.set(Some((buffer, there, size)));
    }

    /// Reads what `record_kept` copied, once its frame ran, and keeps it.
    pub(crate) fn read_kept() {
        let Some((buffer, there, size)) = READ_BACK.take() else { return };
        let ([width, height], pitch) = (size.map(|side| side as usize), (size[0] as usize * 8).next_multiple_of(PITCH));
        let rows = (0..height).map(|row| unsafe { std::slice::from_raw_parts(there.add(row * pitch).cast::<[u16; 4]>(), width) });
        let pixels = rows.flatten().map(|pixel| pixel.map(from_half)).collect();
        unsafe { release(buffer) };
        KEPT.with_borrow_mut(|kept| kept.push((width, pixels)));
    }

    /// The app: a white box, the same at twice and eight times SDR white, an SDR gradient, an HDR
    /// picture from 0 to 4 times white, and the headroom it last heard.
    struct Lit {
        picture: Option<Picture>,
        headroom: f32,
    }

    fn lit(app: &mut Lit, ui: &mut crate::Ui) {
        let ramp: Vec<[f32; 4]> = (0..=16).map(|x| [x as f32 / 4.0, x as f32 / 4.0, x as f32 / 4.0, 1.0]).collect();
        let picture = *app.picture.get_or_insert_with(|| ui.add_hdr_picture(17, &ramp));
        ui.el().flow(Flow::Stack).size(300.0, 200.0).fill(Color::rgb(0x808080)).open();
        ui.el().offset(10.0, 10.0).size(40.0, 20.0).fill(Color::WHITE).show();
        ui.el().offset(60.0, 10.0).size(40.0, 20.0).fill(Color::WHITE).brightness(2.0).show();
        ui.el().offset(110.0, 10.0).size(40.0, 20.0).fill(Color::WHITE).brightness(8.0).show();
        ui.el().offset(10.0, 40.0).size(256.0, 10.0).gradient_across(Color::BLACK, Color::WHITE).show();
        ui.el().offset(10.0, 60.0).picture(picture).size(17.0, 4.0).show();
        ui.close();
        app.headroom = ui.headroom();
    }

    /// Keeps an HDR frame a path presented: its row length and pixels in scRGB.
    pub(crate) fn keep_frame(width: usize, pixels: Vec<[f32; 4]>) {
        KEPT.with_borrow_mut(|kept| kept.push((width, pixels)));
    }

    /// The last scRGB frame presented: its row length and pixels.
    fn last() -> (usize, Vec<[f32; 4]>) {
        KEPT.with_borrow(|kept| kept.last().cloned()).expect("no scRGB frame presented")
    }

    /// What the window shows once it shows `want`, five seconds at most: a GPU's frame goes to
    /// the screen after the messages that drew it, later on a busy computer. (The last picture
    /// where it never does.)
    fn showing(hwnd: HANDLE, want: &[u32]) -> Vec<u32> {
        let since = std::time::Instant::now();
        loop {
            let (shown, _) = crate::windows::tests::picture(hwnd);
            let same = shown.len() == want.len() && shown.iter().zip(want).all(|(a, b)| a & 0xff_ffff == b & 0xff_ffff);
            if same || since.elapsed() > std::time::Duration::from_secs(5) {
                return shown;
            }
            pump();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Whether `value` is within `part` of `wanted` (16-bit floats keep three digits: a part in
    /// 500; HDR10's 10 bits of PQ, a part in 50).
    fn near(value: f32, wanted: f32, part: f32) -> bool {
        (value - wanted).abs() <= wanted.abs() * part + part / 5.0
    }

    /// sRGB's curve undone: the linear value of a byte.
    fn linear(byte: u32) -> f32 {
        let part = byte as f32 / 255.0;
        if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
    }

    /// Opens the app on `path`, its screen HDR with SDR white at 200 nits and a 1,000-nit peak,
    /// and checks the HDR frame as scRGB (each value within `part`): SDR white at 2.5, twice it
    /// at 5, eight times capped at the peak (12.5), the gradient and the picture linear. Then
    /// the screen turns SDR: the frame goes back to what the CPU draws, byte for byte.
    fn lit_by(path: Renderer, software: bool, part: f32) {
        assert!(is_on_its_own_desktop(), "this is the desktop the person works on: no window is shown on it");
        let hdr = Screen { hdr: true, white: 200.0, peak: 1000.0 };
        FAKED.set(Some((hdr, Screen::SDR)));
        AN_HDR_SCREEN.store(true, Ordering::Relaxed);
        fake(Faked { software, keep: true, ..Faked::default() });
        let app = std::rc::Rc::new(RefCell::new(Lit { picture: None, headroom: 0.0 }));
        let shared = app.clone();
        let hwnd = crate::windows::open(Window { renderer: path, position: Some((0, 0)), width: 300.0, height: 200.0, ..Window::default() }, Box::new(move |ui| lit(&mut shared.borrow_mut(), ui)));
        unsafe { ShowWindow(hwnd, 4) };
        // The window's run after the device came hears the screen; the next draws it lit.
        for _ in 0..3 {
            unsafe { PostMessageW(hwnd, 0x8001, 0, 0) };
            pump();
        }
        assert_eq!(app.borrow().headroom, 5.0, "{path:?}: the app hears the headroom");
        let (wide, pixels) = last();
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetDpiForWindow(hwnd: HANDLE) -> u32;
        }
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        let at = |x: f32, y: f32| pixels[(y * scale) as usize * wide + (x * scale) as usize];
        // What the CPU draws of the app at that scale.
        let mut trial = crate::testing::Trial::new(Lit { picture: None, headroom: 0.0 }, lit);
        trial.resize(300.0, 200.0, scale);
        let surface = crate::Surface::draw(&mut trial.ui, 64);
        let white = 200.0 / 80.0;
        let grey = linear(0x80) * white;
        println!(
            "{path:?} at scale {scale}: grey {:?}, white {:?}, twice {:?}, eight times {:?}, picture {:?}",
            at(5.0, 5.0),
            at(30.0, 20.0),
            at(80.0, 20.0),
            at(130.0, 20.0),
            (0..=16).map(|step| at(10.5 + step as f32, 62.0)[0]).collect::<Vec<_>>()
        );
        for (place, wanted) in [((5.0, 5.0), grey), ((30.0, 20.0), white), ((80.0, 20.0), 2.0 * white), ((130.0, 20.0), 1000.0 / 80.0)] {
            let got = at(place.0, place.1);
            assert!(got[..3].iter().all(|value| near(*value, wanted, part)) && got[3] == 1.0, "{path:?} at {place:?}: {got:?}, not {wanted}");
        }
        // The gradient: every pixel the linear value of its byte, times white.
        assert_eq!((surface.width, surface.pixels.len() / surface.width), (wide, pixels.len() / wide), "{path:?}: the same size");
        for x in (10.0 * scale) as usize..(266.0 * scale) as usize {
            let (byte, got) = (surface.pixels[(45.0 * scale) as usize * surface.width + x] & 0xff, pixels[(45.0 * scale) as usize * wide + x]);
            assert!(near(got[2], linear(byte) * white, part), "{path:?}: the gradient at {x}: {got:?} for {byte}");
        }
        // The picture: a quarter of white more every pixel (within 1%: the CPU draws its SDR
        // picture smoothed at other scales).
        for step in 0..=16 {
            let got = at(10.5 + step as f32, 62.0);
            assert!((got[0] - step as f32 / 4.0 * white).abs() <= white * part.max(0.01), "{path:?}: the picture at {step}: {got:?}");
        }
        if path == Renderer::Vulkan {
            metadata_follows_the_screen(hwnd, hdr);
        }
        // The screen goes SDR: the frame is the CPU's again.
        FAKED.set(Some((Screen::SDR, Screen::SDR)));
        screens_changed();
        let before = KEPT.with_borrow(|kept| kept.len());
        for _ in 0..2 {
            unsafe { PostMessageW(hwnd, 0x8001, 0, 0) };
            pump();
        }
        assert_eq!(app.borrow().headroom, 1.0);
        if path == Renderer::OpenGL {
            // A window's float pixel format is its own for life: on an SDR screen its frame is
            // the CPU's pixels made linear, white at 1.0, as Windows shows SDR.
            let (_, frame) = last();
            let apart = frame.iter().zip(&surface.pixels).flat_map(|(got, cpu)| [16, 8, 0].into_iter().zip(got).map(move |(shift, value)| (value - linear(cpu >> shift & 0xff)).abs()));
            let worst = apart.fold(0.0, f32::max);
            assert!(worst <= 1.0 / 1024.0, "{path:?}: an SDR screen gets the CPU's pixels, linear (apart by {worst})");
        } else {
            assert_eq!(KEPT.with_borrow(|kept| kept.len()), before, "{path:?}: no scRGB frame on an SDR screen");
            let shown = showing(hwnd, &surface.pixels);
            let differ = shown.iter().zip(&surface.pixels).filter(|(a, b)| *a & 0xff_ffff != *b & 0xff_ffff).count();
            assert!(shown.len() == surface.pixels.len() && differ == 0, "{path:?}: an SDR screen shows the CPU's pixels ({differ} differ)");
        }
        // HDR again; moved to the screen beside it, SDR; and back.
        let wake = || {
            for _ in 0..2 {
                unsafe { PostMessageW(hwnd, 0x8001, 0, 0) };
                pump();
            }
        };
        FAKED.set(Some((hdr, Screen::SDR)));
        screens_changed();
        wake();
        assert_eq!(app.borrow().headroom, 5.0, "{path:?}: HDR again");
        unsafe { SetWindowPos(hwnd, 0, GetSystemMetrics(0) + 100, 100, 0, 0, 0x15) };
        wake();
        if unsafe { MonitorFromWindow(hwnd, 2) } == first_screen() {
            println!("one screen: no move to another checked");
        } else {
            assert_eq!(app.borrow().headroom, 1.0, "{path:?}: on the SDR screen beside");
            let before = KEPT.with_borrow(|kept| kept.len());
            unsafe { SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x15) };
            wake();
            assert_eq!(app.borrow().headroom, 5.0, "{path:?}: back on the HDR screen");
            assert!(KEPT.with_borrow(|kept| kept.len()) > before, "{path:?}: presented in scRGB again");
            println!("{path:?}: moved to the screen beside and back, followed");
        }
        unsafe { DestroyWindow(hwnd) };
        pump();
        fake(Faked::default());
        FAKED.set(None);
        AN_HDR_SCREEN.store(false, Ordering::Relaxed);
    }

    /// Vulkan, on `hdr`: the swap chain's HDR metadata is set again when the screen's SDR white
    /// changes, then when it changes back (where the device sets metadata).
    fn metadata_follows_the_screen(hwnd: HANDLE, hdr: Screen) {
        let described = || crate::gpu_vulkan_windows::tests::DESCRIBED.get();
        if described().is_none() {
            return println!("Vulkan: no VK_EXT_hdr_metadata, nothing to check");
        }
        assert_eq!(described(), Some(hdr), "Vulkan: the metadata of the screen");
        for screen in [Screen { white: 300.0, ..hdr }, hdr] {
            FAKED.set(Some((screen, Screen::SDR)));
            screens_changed();
            for _ in 0..2 {
                unsafe { PostMessageW(hwnd, 0x8001, 0, 0) };
                pump();
            }
            assert_eq!(described(), Some(screen), "Vulkan: the metadata follows the screen");
        }
    }

    #[test]
    fn direct3d_11_presents_brighter_than_white_in_scrgb() {
        on_a_desktop_of_its_own("hdr_windows::tests::on_a_desktop_nobody_sees_direct3d_11_presents_in_scrgb");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_direct3d_11_presents_in_scrgb() {
        if std::env::var_os(ALONE).is_some() {
            lit_by(Renderer::Direct3D11, true, 1.0 / 500.0);
        }
    }

    #[test]
    fn direct3d_12_presents_brighter_than_white_in_scrgb() {
        on_a_desktop_of_its_own("hdr_windows::tests::on_a_desktop_nobody_sees_direct3d_12_presents_in_scrgb");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_direct3d_12_presents_in_scrgb() {
        if std::env::var_os(ALONE).is_some() {
            lit_by(Renderer::Direct3D12, true, 1.0 / 500.0);
        }
    }

    #[test]
    fn on_this_computers_own_gpu_every_path_presents_in_hdr() {
        on_a_desktop_of_its_own("hdr_windows::tests::on_a_desktop_nobody_sees_the_own_gpu_presents_in_hdr");
    }
    #[test]
    #[ignore = "run by the test before it, in a process of its own that is started on a desktop of its own"]
    fn on_a_desktop_nobody_sees_the_own_gpu_presents_in_hdr() {
        if std::env::var_os(ALONE).is_none() {
            return;
        }
        // (Vulkan in scRGB, and in HDR10 as where a GPU offers no scRGB.)
        let paths = [(Renderer::Direct3D11, false), (Renderer::Direct3D12, false), (Renderer::Vulkan, false), (Renderer::Vulkan, true), (Renderer::OpenGL, false)];
        for (path, hdr10) in paths {
            if crate::gpu_windows::Device::gpu(path, false).is_none_or(|device| device.renderer() != path) {
                println!("no {path:?} on a GPU of its own: nothing to check");
                continue;
            }
            crate::hdr_vulkan::tests::HDR10_FIRST.set(hdr10);
            lit_by(path, false, if hdr10 { 1.0 / 50.0 } else { 1.0 / 500.0 });
        }
    }
}
