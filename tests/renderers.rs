//! taut under a program's own renderer: the Direct3D 9, 11 and 12, OpenGL and Vulkan layers,
//! and the examples using them, checked against the surface they show. A gallery frame,
//! rendered over a known image, is read back and compared with the surface composited over that
//! image by hand; a change updates the texture where it changed and nowhere else; the interface
//! lies at the program's viewport and inside it; the state a layer sets for drawing is restored
//! as the program had it; and errors come back as values, in the system's own words.
//!
//! Nothing is drawn on screen. Direct3D 11 and 12 render on Windows' software rasterizer
//! (WARP); Direct3D 9Ex on the graphics card, in a program of its own started on a desktop of
//! its own; OpenGL through the example's own WGL on a never-shown window, and on Linux through
//! EGL with no display (Mesa's llvmpipe); Vulkan on the system's first device (lavapipe in the
//! tests on Linux). A test needing an X display says so, and runs on its own:
//! `xvfb-run cargo test --test renderers -- --ignored`. With `TAUT_PICTURES` naming a folder,
//! each renderer's gallery frame is saved there.
use std::ffi::c_void;
use taut::geom::Size;
use taut::{Color, Event, Key, LocalClipboard, Mods, Surface, TextSystem, Ui};

#[cfg(windows)]
#[allow(dead_code)]
#[path = "../examples/d3d11.rs"]
mod d3d11;

#[macro_use]
#[allow(dead_code)]
#[path = "../examples/opengl.rs"]
mod opengl;

#[allow(dead_code, unused_attributes)]
#[path = "../examples/gallery.rs"]
mod gallery;

/// The letters of the system, where this library knows how to reach them.
fn letters() -> Box<dyn TextSystem> {
    taut::platform::system_text()
}

/// The gallery, `width` by `height` pixels at `scale`, on a page that is clear: where the
/// gallery draws nothing, and where it draws see-through, what is under it shows.
struct Gallery {
    ui: Ui,
    app: gallery::App,
}

impl Gallery {
    fn new(width: usize, height: usize, scale: f32) -> Gallery {
        let mut ui = Ui::new(letters(), Box::new(LocalClipboard::default()));
        ui.resize(Size { w: width as f32 / scale, h: height as f32 / scale }, scale);
        let mut gallery = Gallery { ui, app: gallery::app() };
        gallery.run();
        gallery.run();
        gallery
    }

    fn run(&mut self) {
        let app = &mut self.app;
        self.ui.run(&mut |ui| {
            gallery::describe(app, ui);
            ui.theme.bg = Color::NONE;
        });
    }
}

/// An interface of nothing at all, `width` by `height` pixels, run once: every pixel clear.
fn nothing(width: usize, height: usize) -> Ui {
    let mut ui = Ui::new(Box::new(taut::text::BuiltInFont), Box::new(LocalClipboard::default()));
    ui.resize(Size { w: width as f32, h: height as f32 }, 1.0);
    ui.run(&mut |ui| ui.theme.bg = Color::NONE);
    ui
}

/// A picture to draw over, of every colour and every degree of solid, in bands.
fn under(width: usize, height: usize) -> Vec<u32> {
    let pixel = |x: usize, y: usize| ((x * 7 % 256) | (y * 5 % 256) << 8 | ((x + y) * 3 % 256) << 16 | (255 - (x * y) % 97) << 24) as u32;
    (0..width * height).map(|at| pixel(at % width, at / width)).collect()
}

/// A surface of every pixel there can be, as a renderer is handed one: for each of the 256
/// degrees of solid (a row), colours from none to as much as that allows.
fn every_pixel() -> Surface {
    let pixel = |c: u32, a: u32| c.min(a) | (c / 2).min(a) << 8 | (255 - c).min(a) << 16 | a << 24;
    Surface { width: 256, height: 256, pixels: (0..256 * 256).map(|at| pixel(at % 256, at / 256)).collect() }
}

/// `top` laid over `under`, both with their colour multiplied by how solid they are: what a
/// renderer's "one, one minus source alpha" works out, here exactly, rounded to the nearest.
fn over(top: u32, under: u32) -> u32 {
    let clear = 255 - (top >> 24);
    [0, 8, 16, 24].into_iter().fold(0, |out, shift| out | (((top >> shift & 255) * 255 + (under >> shift & 255) * clear + 127) / 255) << shift)
}

/// How far `got` is from `surface` laid over `under`, at most, in any channel of any pixel;
/// and how many pixels are off at all.
fn off(got: &[u32], surface: &Surface, under: &[u32]) -> (u32, usize) {
    assert_eq!(got.len(), surface.pixels.len(), "the frame is as large as the surface");
    let channel = |a: u32, b: u32, shift: u32| (a >> shift & 255).abs_diff(b >> shift & 255);
    let differs = |(got, want): (&u32, u32)| [0, 8, 16, 24].into_iter().map(|shift| channel(*got, want, shift)).max().unwrap();
    let wanted = surface.pixels.iter().zip(under).map(|(top, under)| over(*top, *under));
    let off: Vec<u32> = got.iter().zip(wanted).map(differs).collect();
    (off.iter().copied().max().unwrap_or(0), off.iter().filter(|off| **off > 0).count())
}

/// A renderer works in floating point and turns its result back into 8 bits by rounding to
/// the nearest, where this file rounds whole numbers: the two can differ by one at a half
/// (Direct3D allows 0.6 of a step in that rounding, OpenGL and Vulkan as much), never by more.
const TOLERANCE: u32 = 1;

/// The rows of a picture `width` wide, the last first: OpenGL counts rows from the bottom.
fn upside_down(pixels: &[u32], width: usize) -> Vec<u32> {
    pixels.rchunks(width).flatten().copied().collect()
}

/// Saves a frame, where the tests are asked to (`TAUT_PICTURES`).
fn picture(name: &str, width: usize, height: usize, pixels: &[u32]) {
    if let Some(folder) = std::env::var_os("TAUT_PICTURES") {
        let file = std::path::Path::new(&folder).join(format!("layer-{name}-{}.png", std::env::consts::OS));
        assert_eq!(pixels.len(), width * height, "the picture is whole");
        std::fs::write(file, taut::png::encode(width, pixels).unwrap()).unwrap();
    }
}

/// What a renderer is handed: `Surface::update` draws what changed, says where in pixels, and
/// nothing else; all of it, the first time and at a new size; and nothing, when nothing changed.
#[test]
fn a_surface_says_where_it_changed_in_pixels() {
    for scale in [1.0, 1.25] {
        let (mut gallery, mut surface) = (Gallery::new(900, 600, scale), Surface::default());
        assert_eq!(surface.update(&mut gallery.ui), [[0, 0, 900, 600]], "all of it, the first time");
        assert!(surface.pixels == Surface::draw(&mut gallery.ui, 64).pixels);
        assert!(surface.update(&mut gallery.ui).is_empty(), "nothing, when nothing changed");
        for step in 0..3 {
            let before = surface.pixels.clone();
            gallery.ui.event(Event::Key(Key::Tab, Mods::NONE));
            gallery.run();
            let places = surface.update(&mut gallery.ui);
            println!("at {scale}, step {step}: {places:?}");
            assert!(surface.pixels == Surface::draw(&mut gallery.ui, 64).pixels, "the surface is the gallery as it is now");
            let inside = |at: usize| places.iter().any(|[left, top, right, bottom]| (*left..*right).contains(&(at % 900)) && (*top..*bottom).contains(&(at / 900)));
            assert!(surface.pixels.iter().zip(&before).enumerate().all(|(at, (now, was))| now == was || inside(at)), "every pixel that changed is in a place said");
            let area: usize = places.iter().map(|[left, top, right, bottom]| (right - left) * (bottom - top)).sum();
            assert!(area > 0 && area * 10 < 900 * 600, "a small part changed: {places:?}");
        }
        gallery.ui.resize(Size { w: 640.0 / scale, h: 400.0 / scale }, scale);
        gallery.run();
        assert_eq!(surface.update(&mut gallery.ui), [[0, 0, 640, 400]], "all of it, at a new size");
        assert!(surface.pixels == Surface::draw(&mut gallery.ui, 64).pixels);
    }
}

/// What any other renderer is handed: each place as an upload, its bytes read straight from
/// the surface, row by row the surface's width apart; a place reaching outside is cut to the
/// surface, and an empty one has no bytes.
#[test]
fn a_surface_hands_out_each_place_as_texture_uploads_take_it() {
    let surface = Surface { width: 7, height: 5, pixels: (0..35).map(|at| 0x0101_0101 * at).collect() };
    let up = surface.upload([2, 1, 5, 4]);
    assert_eq!((up.x, up.y, up.width, up.height, up.bytes_per_row), (2, 1, 3, 3, 28));
    assert_eq!(up.bytes.len(), 28 * 2 + 3 * 4, "up to the last pixel of the last row, no further");
    for (row, y) in (1..4).enumerate() {
        for (column, x) in (2..5).enumerate() {
            let at = row * 28 + column * 4;
            assert_eq!(u32::from_le_bytes(up.bytes[at..at + 4].try_into().unwrap()), surface.pixels[y * 7 + x], "pixel {x}, {y}: bytes B, G, R, A");
        }
    }
    let cut = surface.upload([5, 3, 9, 8]);
    assert_eq!((cut.x, cut.y, cut.width, cut.height, cut.bytes.len()), (5, 3, 2, 2, 28 + 8));
    assert!(surface.upload([3, 3, 3, 4]).bytes.is_empty() && surface.upload([9, 9, 12, 12]).bytes.is_empty());
    assert_eq!(format!("{up:?}"), "Upload { x: 2, y: 1, width: 3, height: 3, bytes: 68, bytes_per_row: 28 }");
}

/// What the checks below ask of a renderer: its layer, and a way to look at it.
trait Renderer {
    /// The layer brought up to date with the interface (its `update`).
    fn update(&mut self, ui: &mut Ui);
    /// The layer drawn (its `draw`) into a target `size` pixels large that holds `under`,
    /// with the program's viewport at `viewport` (left, top, width, height, from the top
    /// left); the target read back.
    fn drawn(&mut self, ui: &mut Ui, under: &[u32], size: [usize; 2], viewport: [i32; 4]) -> Vec<u32>;
    /// Puts `pixels` into all of the layer's texture, by hand.
    fn fill(&mut self, pixels: &[u32]);
    /// The layer's texture, read back.
    fn texture(&mut self) -> Vec<u32>;
    /// What the layer holds of the interface.
    fn surface(&self) -> &Surface;
}

/// The layer drawn over `under`, in a target its own size, the viewport all of it.
fn drawn_whole(renderer: &mut dyn Renderer, ui: &mut Ui, under: &[u32]) -> Vec<u32> {
    renderer.update(ui);
    let [width, height] = [renderer.surface().width, renderer.surface().height];
    renderer.drawn(ui, under, [width, height], [0, 0, width as i32, height as i32])
}

/// A frame of the gallery, at two scales, and every pixel there can be, drawn over a picture
/// of every colour: each is the surface laid over that picture, to the rounding of a step. And
/// what the layer holds is the gallery, as it is drawn whole.
fn shows_the_surface_over_what_was_drawn(renderer: &mut dyn Renderer, name: &str) {
    for (scale, width, height) in [(1.0, 900, 600), (1.25, 700, 480)] {
        let mut gallery = Gallery::new(width, height, scale);
        let under = under(width, height);
        let got = drawn_whole(renderer, &mut gallery.ui, &under);
        let surface = Surface::draw(&mut gallery.ui, 64);
        assert!(renderer.surface().pixels == surface.pixels, "{name}: the layer holds the gallery");
        let (most, pixels) = off(&got, &surface, &under);
        println!("{name}, the gallery at {scale}: {pixels} pixels off, by {most} at most");
        assert!(most <= TOLERANCE, "{name}, the gallery at {scale}: off by {most}");
        assert!(surface.pixels.iter().any(|p| p >> 24 == 0) && surface.pixels.iter().any(|p| (1..255).contains(&(p >> 24))), "the page is clear, and some pixels are half solid");
        if scale == 1.0 {
            picture(name, width, height, &got);
        }
    }
    let (surface, mut ui) = (every_pixel(), nothing(256, 256));
    renderer.update(&mut ui);
    renderer.fill(&surface.pixels);
    let under = under(256, 256);
    let got = renderer.drawn(&mut ui, &under, [256, 256], [0, 0, 256, 256]);
    let (most, pixels) = off(&got, &surface, &under);
    println!("{name}, every pixel: {pixels} off, by {most} at most");
    assert!(most <= TOLERANCE, "{name}, every pixel: off by {most}");
}

/// The keyboard comes to the gallery's first part, and then to the next two: a ring is drawn,
/// and moves. Each time the texture is first filled with a mark by hand, and the layer brought
/// up to date: the texture is then the mark or the new picture in every pixel, the new picture
/// wherever it changed, and the mark in all but a small part.
fn is_given_only_the_places_that_changed(renderer: &mut dyn Renderer, name: &str) {
    const MARK: u32 = 0x8040_2010;
    for scale in [1.0, 1.25] {
        let mut gallery = Gallery::new(900, 600, scale);
        renderer.update(&mut gallery.ui);
        for step in 0..3 {
            let before = renderer.surface().pixels.clone();
            renderer.fill(&vec![MARK; before.len()]);
            gallery.ui.event(Event::Key(Key::Tab, Mods::NONE));
            gallery.run();
            renderer.update(&mut gallery.ui);
            let (texture, after) = (renderer.texture(), renderer.surface().pixels.clone());
            assert!(after == Surface::draw(&mut gallery.ui, 64).pixels, "{name}: the layer holds the gallery as it is now");
            let copied = texture.iter().filter(|texel| **texel != MARK).count();
            println!("{name} at {scale}, step {step}: {copied} pixels copied");
            assert!(texture.iter().zip(&after).all(|(texel, pixel)| *texel == MARK || texel == pixel), "{name}: each pixel is the mark or the picture");
            let changed = after.iter().zip(&before).zip(&texture).filter(|((now, was), _)| now != was);
            assert!(changed.clone().count() > 0 && changed.into_iter().all(|((now, _), texel)| texel == now), "{name}: every pixel that changed was copied");
            assert!(copied > 0 && copied * 10 < after.len(), "{name}: only a small part was copied ({copied} pixels)");
            renderer.fill(&after);
        }
        let under = under(900, 600);
        let got = drawn_whole(renderer, &mut gallery.ui, &under);
        let (most, _) = off(&got, renderer.surface(), &under);
        assert!(most <= TOLERANCE, "{name}: off by {most} after the changes");
    }
}

/// The program's viewport lies inside its target, away from the corner, and is smaller than
/// the interface: the interface's top left corner is at the viewport's, nothing of it lies
/// outside the viewport, and the program's picture is as it was everywhere else.
fn lies_at_the_programs_viewport(renderer: &mut dyn Renderer, name: &str) {
    let (target, [x, y, width, height]) = ([420, 300], [40, 30, 300, 200]);
    let mut gallery = Gallery::new(320, 230, 1.0);
    let under = under(target[0], target[1]);
    let got = renderer.drawn(&mut gallery.ui, &under, target, [x, y, width, height]);
    let surface = renderer.surface();
    let inside = |at: usize| (x..x + width).contains(&((at % target[0]) as i32)) && (y..y + height).contains(&((at / target[0]) as i32));
    let pixel = |at: usize| surface.pixels[(at / target[0] - y as usize) * surface.width + at % target[0] - x as usize];
    let wanted = Surface { width: target[0], height: target[1], pixels: (0..under.len()).map(|at| if inside(at) { pixel(at) } else { 0 }).collect() };
    let (most, pixels) = off(&got, &wanted, &under);
    println!("{name}, at the program's viewport: {pixels} pixels off, by {most} at most");
    assert!(most <= TOLERANCE, "{name}: at the program's viewport, off by {most}");
    assert!(got.iter().zip(&under).enumerate().all(|(at, (got, under))| inside(at) || got == under), "{name}: nothing is drawn outside the viewport");
    picture(&format!("{name}-viewport"), target[0], target[1], &got);
}

#[cfg(windows)]
mod direct3d_11 {
    use super::d3d11::direct3d::{Com, check, function, release};
    use super::*;
    use std::ptr::{null, null_mut};
    use taut::layer_d3d11::Layer;

    #[link(name = "d3d11")]
    unsafe extern "system" {
        fn D3D11CreateDevice(adapter: Com, driver: u32, software: isize, flags: u32, levels: *const u32, count: u32, version: u32, device: *mut Com, level: *mut u32, context: *mut Com) -> i32;
    }

    /// A device on Windows' own software renderer (5), which needs no graphics card and no
    /// window, or on the graphics card (1); of the newest feature level, or of `level`.
    fn device(driver: u32, level: Option<u32>) -> (Com, Com) {
        let (mut device, mut context, levels) = (null_mut(), null_mut(), level.as_ref().map_or(null(), |level| level as *const u32));
        check(unsafe { D3D11CreateDevice(null_mut(), driver, 0, 0x20, levels, level.is_some() as u32, 7, &mut device, null_mut(), &mut context) }, "making a device");
        (device, context)
    }

    #[repr(C)]
    #[derive(Default)]
    struct TextureDescription {
        width: u32,
        height: u32,
        mip_levels: u32,
        array_size: u32,
        format: u32,
        sample_count: u32,
        sample_quality: u32,
        usage: u32,
        bind_flags: u32,
        cpu_access_flags: u32,
        misc_flags: u32,
    }

    /// A texture of the layer's kind of pixel (B8G8R8A8_UNORM).
    fn texture(device: Com, width: usize, height: usize, usage: u32, bind: u32, access: u32) -> Com {
        let desc = TextureDescription {
            width: width as u32,
            height: height as u32,
            mip_levels: 1,
            array_size: 1,
            format: 87,
            sample_count: 1,
            usage,
            bind_flags: bind,
            cpu_access_flags: access,
            ..Default::default()
        };
        let mut texture = null_mut();
        check(unsafe { function::<unsafe extern "system" fn(Com, *const TextureDescription, *const c_void, *mut Com) -> i32>(device, 5)(device, &desc, null(), &mut texture) }, "making a texture");
        texture
    }

    /// Puts `pixels` into all of a texture `width` wide (context function 48).
    fn fill(context: Com, texture: Com, pixels: &[u32], width: usize) {
        type Update = unsafe extern "system" fn(Com, Com, u32, *const c_void, *const u32, u32, u32);
        unsafe { function::<Update>(context, 48)(context, texture, 0, null(), pixels.as_ptr(), (width * 4) as u32, 0) };
    }

    #[repr(C)]
    struct Mapped {
        data: *mut u8,
        row_pitch: u32,
        depth_pitch: u32,
    }
    type Map = unsafe extern "system" fn(Com, Com, u32, u32, u32, *mut Mapped) -> i32;

    /// The pixels of a texture, read back through one the processor can read (context
    /// functions 47: copy all, 14: map, 15: unmap).
    fn read(device: Com, context: Com, texture: Com, width: usize, height: usize) -> Vec<u32> {
        let staging = self::texture(device, width, height, 3, 0, 0x20000);
        let mut mapped = Mapped { data: null_mut(), row_pitch: 0, depth_pitch: 0 };
        let mut pixels = vec![0u32; width * height];
        unsafe {
            function::<unsafe extern "system" fn(Com, Com, Com)>(context, 47)(context, staging, texture);
            check(function::<Map>(context, 14)(context, staging, 0, 1, 0, &mut mapped), "reading a texture");
            for (y, row) in pixels.chunks_mut(width).enumerate() {
                std::ptr::copy_nonoverlapping(mapped.data.add(y * mapped.row_pitch as usize), row.as_mut_ptr().cast::<u8>(), width * 4);
            }
            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 15)(context, staging, 0);
            release(staging);
        }
        pixels
    }

    /// A render target that holds `under`, `size` pixels large, bound with the viewport
    /// `[x, y, width, height]`; and its view. (Device function 9 makes the view; context
    /// functions 33 and 44 bind it and set the viewport.)
    fn target(device: Com, context: Com, under: &[u32], size: [usize; 2], [x, y, width, height]: [i32; 4]) -> (Com, Com) {
        let target = texture(device, size[0], size[1], 0, 0x20, 0);
        fill(context, target, under, size[0]);
        let (mut view, viewport) = (null_mut(), [x as f32, y as f32, width as f32, height as f32, 0.0, 1.0]);
        unsafe {
            check(function::<unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32>(device, 9)(device, target, null(), &mut view), "making a render target");
            function::<unsafe extern "system" fn(Com, u32, *const Com, Com)>(context, 33)(context, 1, &view, null_mut());
            function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(context, 44)(context, 1, &viewport);
        }
        (target, view)
    }

    /// The layer, on a device of Windows' own software renderer.
    struct Direct3D {
        device: Com,
        context: Com,
        layer: Layer,
    }

    impl Direct3D {
        fn new() -> Direct3D {
            let (device, context) = device(5, None);
            let layer = unsafe { Layer::new(device, context) }.unwrap_or_else(|failed| panic!("{failed}"));
            Direct3D { device, context, layer }
        }
    }

    impl Renderer for Direct3D {
        fn update(&mut self, ui: &mut Ui) {
            self.layer.update(ui).unwrap_or_else(|failed| panic!("{failed}"));
        }
        fn drawn(&mut self, ui: &mut Ui, under: &[u32], size: [usize; 2], viewport: [i32; 4]) -> Vec<u32> {
            let (target, view) = target(self.device, self.context, under, size, viewport);
            self.layer.draw(ui).unwrap_or_else(|failed| panic!("{failed}"));
            let pixels = read(self.device, self.context, target, size[0], size[1]);
            unsafe { [view, target].into_iter().for_each(|object| release(object)) };
            pixels
        }
        fn fill(&mut self, pixels: &[u32]) {
            fill(self.context, self.layer.texture(), pixels, self.layer.surface().width);
        }
        fn texture(&mut self) -> Vec<u32> {
            read(self.device, self.context, self.layer.texture(), self.layer.surface().width, self.layer.surface().height)
        }
        fn surface(&self) -> &Surface {
            self.layer.surface()
        }
    }

    impl Drop for Direct3D {
        fn drop(&mut self) {
            // (The layer holds on to the device and context until it goes too.)
            unsafe { [self.context, self.device].into_iter().for_each(|object| release(object)) };
        }
    }

    #[test]
    fn direct3d_11_shows_the_surface_over_what_was_drawn() {
        shows_the_surface_over_what_was_drawn(&mut Direct3D::new(), "Direct3D 11");
    }

    #[test]
    fn direct3d_11_is_given_only_the_places_that_changed() {
        is_given_only_the_places_that_changed(&mut Direct3D::new(), "Direct3D 11");
    }

    #[test]
    fn direct3d_11_lies_at_the_programs_viewport() {
        lies_at_the_programs_viewport(&mut Direct3D::new(), "Direct3D 11");
    }

    type Compile = unsafe extern "system" fn(*const u8, usize, *const u8, *const c_void, *const c_void, *const u8, *const u8, u32, u32, *mut Com, *mut Com) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(library: isize, name: *const u8) -> *const c_void;
    }

    /// A shader of the test's own, compiled by Windows' compiler, and made on the device as a
    /// vertex shader (device function 12) or a pixel shader (15); and its code.
    fn shader(device: Com, source: &str, target: &str, make: usize) -> (Com, Vec<u8>) {
        let library: Vec<u16> = "d3dcompiler_47.dll\0".encode_utf16().collect();
        let (target, mut code, mut errors, mut shader) = (format!("{target}\0"), null_mut(), null_mut(), null_mut());
        unsafe {
            let compile: Compile = std::mem::transmute(GetProcAddress(LoadLibraryW(library.as_ptr()), c"D3DCompile".as_ptr().cast()));
            check(compile(source.as_ptr(), source.len(), null(), null(), null(), c"main".as_ptr().cast(), target.as_ptr(), 0, 0, &mut code, &mut errors), "compiling a shader");
            let (start, length) = (function::<unsafe extern "system" fn(Com) -> *const u8>(code, 3)(code), function::<unsafe extern "system" fn(Com) -> usize>(code, 4)(code));
            check(function::<unsafe extern "system" fn(Com, *const u8, usize, Com, *mut Com) -> i32>(device, make)(device, start, length, null_mut(), &mut shader), "making a shader");
            let bytes = std::slice::from_raw_parts(start, length).to_vec();
            [code, errors].into_iter().for_each(|blob| release(blob));
            (shader, bytes)
        }
    }

    /// An input layout of one element, `AT`, four numbers (R32G32B32A32_FLOAT, 2), for the
    /// vertex shader of `code` (device function 11).
    fn layout(device: Com, code: &[u8]) -> Com {
        #[repr(C)]
        struct Element(*const u8, u32, u32, u32, u32, u32, u32);
        let (element, mut layout) = (Element(c"AT".as_ptr().cast(), 0, 2, 0, 0, 0, 0), null_mut());
        check(
            unsafe { function::<unsafe extern "system" fn(Com, *const Element, u32, *const u8, usize, *mut Com) -> i32>(device, 11)(device, &element, 1, code.as_ptr(), code.len(), &mut layout) },
            "making an input layout",
        );
        layout
    }

    /// Everything of a context's that the layer sets, as the context says it now: the vertex
    /// and pixel shaders, the pixel shader's first resource, the input layout, blending, depth,
    /// the rasterizer, as objects; the topology, blend factor, sample mask and stencil
    /// reference, the viewports and the scissor rectangles, as numbers.
    fn state(context: Com) -> ([Com; 7], Vec<u32>) {
        let (mut objects, mut factor, mut numbers) = ([null_mut(); 7], [0.0f32; 4], [0u32; 4]);
        let (mut viewports, mut scissors, mut counts) = ([0.0f32; 6 * 16], [0i32; 4 * 16], [16u32, 16]);
        let [vertex, pixel, resource, layout, blend, depth, rasterizer] = &mut objects;
        unsafe {
            function::<unsafe extern "system" fn(Com, *mut Com, *mut Com, *mut u32)>(context, 76)(context, vertex, null_mut(), &mut 0);
            function::<unsafe extern "system" fn(Com, *mut Com, *mut Com, *mut u32)>(context, 74)(context, pixel, null_mut(), &mut 0);
            function::<unsafe extern "system" fn(Com, u32, u32, *mut Com)>(context, 73)(context, 0, 1, resource);
            function::<unsafe extern "system" fn(Com, *mut Com)>(context, 78)(context, layout);
            function::<unsafe extern "system" fn(Com, *mut u32)>(context, 83)(context, &mut numbers[0]);
            function::<unsafe extern "system" fn(Com, *mut Com, *mut f32, *mut u32)>(context, 91)(context, blend, factor.as_mut_ptr(), &mut numbers[1]);
            function::<unsafe extern "system" fn(Com, *mut Com, *mut u32)>(context, 92)(context, depth, &mut numbers[2]);
            function::<unsafe extern "system" fn(Com, *mut Com)>(context, 94)(context, rasterizer);
            function::<unsafe extern "system" fn(Com, *mut u32, *mut f32)>(context, 95)(context, &mut counts[0], viewports.as_mut_ptr());
            function::<unsafe extern "system" fn(Com, *mut u32, *mut i32)>(context, 96)(context, &mut counts[1], scissors.as_mut_ptr());
            objects.iter().for_each(|object| release(*object));
        }
        let numbers = numbers.into_iter().chain(factor.map(f32::to_bits)).chain(counts).chain(viewports.map(f32::to_bits)).chain(scissors.map(|n| n as u32)).collect();
        (objects, numbers)
    }

    /// The program has set everything the layer sets, its own way: after the layer is drawn,
    /// the context says the same of each as it said before.
    #[test]
    fn direct3d_11_puts_back_what_it_sets() {
        let mut direct3d = Direct3D::new();
        let (device, context) = (direct3d.device, direct3d.context);
        let (vertex, code) = shader(device, "float4 main(float4 at : AT) : SV_Position { return at; }", "vs_4_0", 12);
        let (pixel, _) = shader(device, "float4 main() : SV_Target { return 1; }", "ps_4_0", 15);
        let layout = layout(device, &code);
        let picture = texture(device, 4, 4, 0, 0x8, 0);
        let (mut view, mut blend, mut depth, mut rasterizer) = (null_mut(), null_mut(), null_mut(), null_mut());
        // Adding (2, 2, 1), depth tested and written (1, 1, 2), and drawn as lines with the
        // backs of shapes culled (2, 3).
        let mut blending = [0u32; 2 + 8 * 8];
        blending[2..10].copy_from_slice(&[1, 2, 2, 1, 2, 2, 1, 0x0f]);
        let depthing: [u32; 13] = [1, 1, 2, 0, 0xffff, 1, 1, 1, 8, 1, 1, 1, 8];
        let rasterizing: [u32; 10] = [2, 3, 0, 0, 0, 0, 1, 0, 0, 0];
        unsafe {
            check(function::<unsafe extern "system" fn(Com, Com, *const c_void, *mut Com) -> i32>(device, 7)(device, picture, null(), &mut view), "a view");
            check(function::<unsafe extern "system" fn(Com, *const [u32; 66], *mut Com) -> i32>(device, 20)(device, &blending, &mut blend), "blending");
            check(function::<unsafe extern "system" fn(Com, *const [u32; 13], *mut Com) -> i32>(device, 21)(device, &depthing, &mut depth), "depth");
            check(function::<unsafe extern "system" fn(Com, *const [u32; 10], *mut Com) -> i32>(device, 22)(device, &rasterizing, &mut rasterizer), "rasterizing");
            function::<unsafe extern "system" fn(Com, Com, *const Com, u32)>(context, 11)(context, vertex, null(), 0);
            function::<unsafe extern "system" fn(Com, Com, *const Com, u32)>(context, 9)(context, pixel, null(), 0);
            function::<unsafe extern "system" fn(Com, u32, u32, *const Com)>(context, 8)(context, 0, 1, &view);
            function::<unsafe extern "system" fn(Com, u32)>(context, 24)(context, 1);
            function::<unsafe extern "system" fn(Com, Com)>(context, 17)(context, layout);
            function::<unsafe extern "system" fn(Com, Com, *const f32, u32)>(context, 35)(context, blend, [0.25f32, 0.5, 0.75, 1.0].as_ptr(), 0x00ff_00ff);
            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 36)(context, depth, 7);
            function::<unsafe extern "system" fn(Com, Com)>(context, 43)(context, rasterizer);
            function::<unsafe extern "system" fn(Com, u32, *const [i32; 4])>(context, 45)(context, 2, [[1, 2, 30, 40], [5, 6, 70, 80]].as_ptr());
        }
        let mut gallery = Gallery::new(320, 230, 1.0);
        let (target, render_view) = target(device, context, &under(400, 300), [400, 300], [12, 8, 350, 260]);
        let viewports = [[12.0f32, 8.0, 350.0, 260.0, 0.0, 1.0], [100.0, 100.0, 50.0, 50.0, 0.25, 0.75]];
        unsafe { function::<unsafe extern "system" fn(Com, u32, *const [f32; 6])>(context, 44)(context, 2, viewports.as_ptr()) };
        let before = state(context);
        assert!(before.0.iter().all(|object| !object.is_null()), "everything was set");
        direct3d.layer.draw(&mut gallery.ui).unwrap();
        assert_eq!(state(context), before, "the context is as the program had it");
        let got = read(device, context, target, 400, 300);
        assert!(got.iter().zip(under(400, 300)).filter(|(got, under)| *got != under).count() > 10_000, "the layer drew, though the program's states would have hidden it");
        unsafe { [vertex, pixel, layout, picture, view, blend, depth, rasterizer, target, render_view].into_iter().for_each(|object| release(object)) };
    }

    /// What goes wrong comes back as a value, with Windows' own words: shaders of model 4 on
    /// a device of feature level 9.1.
    #[test]
    fn direct3d_11_says_what_went_wrong() {
        let (device, context) = device(5, Some(0x9100));
        let failed = unsafe { Layer::new(device, context) }.expect_err("a device of level 9.1 takes no shaders of model 4");
        println!("{failed}");
        assert_eq!((failed.operation, failed.code as u32), ("making the vertex shader", 0x8007_0057));
        assert!(!failed.message.is_empty() && failed.to_string().contains(&failed.message), "Windows' words are said");
        unsafe { [context, device].into_iter().for_each(|object| release(object)) };
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: isize, menu: isize, instance: isize, param: *const c_void) -> isize;
        fn DestroyWindow(hwnd: isize) -> i32;
    }

    /// A window that is never shown: Windows' own kind that shows a picture, with no frame,
    /// that never takes the keyboard and has no button on the task bar.
    pub fn unseen_window() -> isize {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let window = unsafe { CreateWindowExW(0x0800_0080, class.as_ptr(), [0u16].as_ptr(), 0x8000_0000, -30000, -30000, 320, 200, 0, 0, 0, null()) };
        assert_ne!(window, 0);
        window
    }

    pub fn close(window: isize) {
        unsafe { DestroyWindow(window) };
    }

    #[test]
    fn the_direct3d_11_example_draws_into_its_window_at_any_size() {
        let window = unseen_window();
        let mut device = d3d11::direct3d::Device::new(window);
        let mut layer = unsafe { Layer::new(device.device, device.context) }.unwrap();
        for (width, height) in [(320, 200), (480, 300)] {
            device.resize(width as u32, height as u32);
            let mut gallery = Gallery::new(width, height, 1.0);
            device.clear([51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 1.0]);
            layer.draw(&mut gallery.ui).unwrap();
            let mut buffer = null_mut();
            const TEXTURE_2D: [u32; 4] = [0x6f15_aaf2, 0x4e89_d208, 0x9548_b49a, 0x9c4f_d335];
            check(unsafe { function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(device.swap, 9)(device.swap, 0, &TEXTURE_2D, &mut buffer) }, "finding the buffer");
            let got = read(device.device, device.context, buffer, width, height);
            unsafe { release(buffer) };
            let (most, pixels) = off(&got, layer.surface(), &vec![0xff33_6699; width * height]);
            println!("the example's own device, {width} by {height}: {pixels} pixels off, by {most} at most");
            assert!(most <= TOLERANCE);
            device.show();
        }
        drop((layer, device));
        close(window);
    }

    /// How long `upload` takes: to come back, and until the device has done what it asked
    /// (an event that the device marks done once it is past it: context functions 28, 111
    /// and 29 put it in, send everything on, and ask).
    fn timed(context: Com, query: Com, upload: &mut dyn FnMut()) -> [f64; 2] {
        let start = std::time::Instant::now();
        upload();
        let called = start.elapsed().as_secs_f64() * 1e6;
        unsafe {
            function::<unsafe extern "system" fn(Com, Com)>(context, 28)(context, query);
            function::<unsafe extern "system" fn(Com)>(context, 111)(context);
            while function::<unsafe extern "system" fn(Com, Com, *mut c_void, u32, u32) -> i32>(context, 29)(context, query, null_mut(), 0, 0) != 0 {}
        }
        [called, start.elapsed().as_secs_f64() * 1e6]
    }

    /// Copies the rows of `place` of the surface into mapped memory whose rows are `pitch` apart.
    unsafe fn copy_rows(surface: &Surface, mapped: &Mapped, [left, top, right, bottom]: [usize; 4]) {
        for y in top..bottom {
            let from = &surface.pixels[y * surface.width + left..y * surface.width + right];
            unsafe { std::ptr::copy_nonoverlapping(from.as_ptr().cast::<u8>(), mapped.data.add(y * mapped.row_pitch as usize + left * 4), from.len() * 4) };
        }
    }

    /// Three ways to get a change to a texture, a small one (a button's worth) and all of a
    /// surface as large as a screen, timed in turns: the layer's (`UpdateSubresource` from
    /// the surface itself); a texture the processor writes, copied over on the device; and a
    /// texture the processor writes that is drawn from itself, which must be written whole
    /// each time.
    ///   cargo test --release --test renderers -- --ignored how_long --nocapture
    #[test]
    #[ignore]
    fn how_long_direct3d_11_takes_to_copy_a_change() {
        let (width, height, rounds) = (1920, 1080, 300);
        let surface = Surface { width, height, pixels: (0..width * height).map(|at| (at as u32).wrapping_mul(2_654_435_761)).collect() };
        let sizes = [[880, 500, 1000, 532], [0, 0, width, height]];
        for (name, (device, context)) in [("WARP", device(5, None)), ("graphics card", device(1, None))] {
            let layered = texture(device, width, height, 0, 0x8, 0);
            let staging = texture(device, width, height, 3, 0, 0x10000);
            let dynamic = texture(device, width, height, 2, 0x8, 0x10000);
            let mut query = null_mut();
            check(unsafe { function::<unsafe extern "system" fn(Com, *const [u32; 2], *mut Com) -> i32>(device, 24)(device, &[0, 0], &mut query) }, "making an event");
            let mut best = [[[f64::MAX; 2]; 2]; 3];
            type Update = unsafe extern "system" fn(Com, Com, u32, *const [u32; 6], *const u32, u32, u32);
            for _ in 0..rounds {
                for (size, place) in sizes.iter().enumerate() {
                    let area = [place[0] as u32, place[1] as u32, 0, place[2] as u32, place[3] as u32, 1];
                    let ways: [&mut dyn FnMut(); 3] = [
                        &mut || unsafe { function::<Update>(context, 48)(context, layered, 0, &area, &surface.pixels[place[1] * width + place[0]], (width * 4) as u32, 0) },
                        &mut || unsafe {
                            let mut mapped = Mapped { data: null_mut(), row_pitch: 0, depth_pitch: 0 };
                            check(function::<Map>(context, 14)(context, staging, 0, 2, 0, &mut mapped), "mapping");
                            copy_rows(&surface, &mapped, *place);
                            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 15)(context, staging, 0);
                            type Copy = unsafe extern "system" fn(Com, Com, u32, u32, u32, u32, Com, u32, *const [u32; 6]);
                            function::<Copy>(context, 46)(context, layered, 0, place[0] as u32, place[1] as u32, 0, staging, 0, &area);
                        },
                        &mut || unsafe {
                            let mut mapped = Mapped { data: null_mut(), row_pitch: 0, depth_pitch: 0 };
                            check(function::<Map>(context, 14)(context, dynamic, 0, 4, 0, &mut mapped), "mapping");
                            copy_rows(&surface, &mapped, [0, 0, width, height]);
                            function::<unsafe extern "system" fn(Com, Com, u32)>(context, 15)(context, dynamic, 0);
                        },
                    ];
                    for (way, upload) in ways.into_iter().enumerate() {
                        let [called, done] = timed(context, query, upload);
                        best[way][size] = [best[way][size][0].min(called), best[way][size][1].min(done)];
                    }
                }
            }
            for (way, method) in ["UpdateSubresource", "staging + copy", "dynamic, whole"].iter().enumerate() {
                let [[small_called, small_done], [whole_called, whole_done]] = best[way];
                println!("{name}, {method}: small {small_called:.0} us to call, {small_done:.0} until done; whole {whole_called:.0} to call, {whole_done:.0} until done");
            }
            unsafe { [query, layered, staging, dynamic, context, device].into_iter().for_each(|object| release(object)) };
        }
    }
}

functions!(Checks {
    glGenFramebuffers: fn(i32, *mut u32);
    glBindFramebuffer: fn(u32, u32);
    glFramebufferTexture2D: fn(u32, u32, u32, u32, i32);
    glCheckFramebufferStatus: fn(u32) -> u32;
    glDeleteFramebuffers: fn(i32, *const u32);
    glReadPixels: fn(i32, i32, i32, i32, u32, u32, *mut c_void);
    glGetTexImage: fn(u32, i32, u32, u32, *mut c_void);
    glGetString: fn(u32) -> *const std::ffi::c_char;
    glGetIntegerv: fn(u32, *mut i32);
    glIsEnabled: fn(u32) -> u8;
    glEnable: fn(u32);
    glDisable: fn(u32);
    glGenTextures: fn(i32, *mut u32);
    glDeleteTextures: fn(i32, *const u32);
    glBindTexture: fn(u32, u32);
    glActiveTexture: fn(u32);
    glTexImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glTexSubImage2D: fn(u32, i32, i32, i32, i32, i32, u32, u32, *const c_void);
    glPixelStorei: fn(u32, i32);
    glGenBuffers: fn(i32, *mut u32);
    glBindBuffer: fn(u32, u32);
    glDeleteBuffers: fn(i32, *const u32);
    glGenVertexArrays: fn(i32, *mut u32);
    glBindVertexArray: fn(u32);
    glCreateShader: fn(u32) -> u32;
    glShaderSource: fn(u32, i32, *const *const std::ffi::c_char, *const i32);
    glCompileShader: fn(u32);
    glCreateProgram: fn() -> u32;
    glAttachShader: fn(u32, u32);
    glLinkProgram: fn(u32);
    glUseProgram: fn(u32);
    glBlendFuncSeparate: fn(u32, u32, u32, u32);
    glBlendEquationSeparate: fn(u32, u32);
    glPolygonMode: fn(u32, u32);
    glViewport: fn(i32, i32, i32, i32);
    glScissor: fn(i32, i32, i32, i32);
});

// (macOS: no context of the kind made here.)
#[cfg(any(windows, all(unix, not(target_os = "macos"))))]
mod open_gl {
    use super::*;
    use std::ffi::CStr;
    use taut::layer_opengl::Layer;

    // OpenGL's numbers, as glcorearb.h has them.
    const TEXTURE_2D: u32 = 0x0DE1;
    const BGRA: u32 = 0x80E1;
    const UNSIGNED_INT_8_8_8_8_REV: u32 = 0x8367;
    const FRAMEBUFFER: u32 = 0x8D40;

    /// How a context's functions are found by name.
    type Find = Box<dyn Fn(&CStr) -> *const c_void>;

    /// A context current on this thread, with nothing on any screen: the example's own WGL
    /// context on a window that is never shown; or EGL with no display, Mesa's. And how its
    /// functions are found.
    #[cfg(windows)]
    pub fn context() -> (Find, isize) {
        let window = super::direct3d_11::unseen_window();
        super::opengl::wgl::context(window);
        (Box::new(super::opengl::wgl::find), window)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    pub fn context() -> (Find, super::opengl::egl::Context) {
        use super::opengl::egl;
        let context = egl::context(egl::PLATFORM_SURFACELESS, std::ptr::null_mut(), None).expect("EGL gives no OpenGL 3.3 of the core profile without a display");
        let find = context.egl.eglGetProcAddress;
        (Box::new(move |name| unsafe { find(name.as_ptr()) }), context)
    }

    /// Which OpenGL this is, as it says.
    fn renderer(checks: &Checks) -> String {
        // SAFETY: OpenGL hands back a text that ends in a zero (0x1F01: who draws, 0x1F02: the version).
        let string = |what| unsafe { CStr::from_ptr((checks.glGetString)(what)).to_string_lossy().into_owned() };
        format!("{}, {}", string(0x1F01), string(0x1F02))
    }

    /// The layer, in a context of this thread's.
    struct OpenGl<Keep> {
        checks: Checks,
        layer: Layer,
        /// What the context needs kept for as long as it is used.
        _keep_alive: Keep,
    }

    impl<Keep> OpenGl<Keep> {
        fn new((find, keep_alive): (Find, Keep)) -> OpenGl<Keep> {
            let checks = Checks::load(&mut |name| find(name));
            println!("OpenGL: {}", renderer(&checks));
            let layer = unsafe { Layer::new(&mut |name| find(name)) }.unwrap_or_else(|failed| panic!("{failed}"));
            OpenGl { checks, layer, _keep_alive: keep_alive }
        }
    }

    impl<Keep> Renderer for OpenGl<Keep> {
        fn update(&mut self, ui: &mut Ui) {
            unsafe { self.layer.update(ui) }.unwrap_or_else(|failed| panic!("{failed}"));
        }
        fn drawn(&mut self, ui: &mut Ui, under: &[u32], [width, height]: [usize; 2], [x, y, wide, high]: [i32; 4]) -> Vec<u32> {
            let checks = &self.checks;
            let (mut target, mut framebuffer, mut pixels) = (0, 0, vec![0u32; width * height]);
            // A texture holding the picture underneath, its rows turned over, drawn into
            // through a framebuffer as its first colour (0x8CE0); and read back. The viewport
            // is said from the bottom, as OpenGL counts.
            unsafe {
                (checks.glGenTextures)(1, &mut target);
                (checks.glBindTexture)(TEXTURE_2D, target);
                (checks.glTexImage2D)(TEXTURE_2D, 0, 0x8058, width as i32, height as i32, 0, BGRA, UNSIGNED_INT_8_8_8_8_REV, upside_down(under, width).as_ptr().cast());
                (checks.glGenFramebuffers)(1, &mut framebuffer);
                (checks.glBindFramebuffer)(FRAMEBUFFER, framebuffer);
                (checks.glFramebufferTexture2D)(FRAMEBUFFER, 0x8CE0, TEXTURE_2D, target, 0);
                assert_eq!((checks.glCheckFramebufferStatus)(FRAMEBUFFER), 0x8CD5, "the framebuffer is complete");
                (checks.glViewport)(x, height as i32 - y - high, wide, high);
                self.layer.draw(ui).unwrap_or_else(|failed| panic!("{failed}"));
                (checks.glReadPixels)(0, 0, width as i32, height as i32, BGRA, UNSIGNED_INT_8_8_8_8_REV, pixels.as_mut_ptr().cast());
                (checks.glBindFramebuffer)(FRAMEBUFFER, 0);
                (checks.glDeleteFramebuffers)(1, &framebuffer);
                (checks.glDeleteTextures)(1, &target);
            }
            upside_down(&pixels, width)
        }
        fn fill(&mut self, pixels: &[u32]) {
            let surface = self.layer.surface();
            unsafe {
                (self.checks.glBindTexture)(TEXTURE_2D, self.layer.texture());
                (self.checks.glTexSubImage2D)(TEXTURE_2D, 0, 0, 0, surface.width as i32, surface.height as i32, BGRA, UNSIGNED_INT_8_8_8_8_REV, pixels.as_ptr().cast());
            }
        }
        fn texture(&mut self) -> Vec<u32> {
            let mut pixels = vec![0u32; self.layer.surface().pixels.len()];
            unsafe {
                (self.checks.glBindTexture)(TEXTURE_2D, self.layer.texture());
                (self.checks.glGetTexImage)(TEXTURE_2D, 0, BGRA, UNSIGNED_INT_8_8_8_8_REV, pixels.as_mut_ptr().cast());
            }
            pixels
        }
        fn surface(&self) -> &Surface {
            self.layer.surface()
        }
    }

    #[test]
    fn opengl_shows_the_surface_over_what_was_drawn() {
        shows_the_surface_over_what_was_drawn(&mut OpenGl::new(context()), "OpenGL");
    }

    #[test]
    fn opengl_is_given_only_the_places_that_changed() {
        is_given_only_the_places_that_changed(&mut OpenGl::new(context()), "OpenGL");
    }

    #[test]
    fn opengl_lies_at_the_programs_viewport() {
        lies_at_the_programs_viewport(&mut OpenGl::new(context()), "OpenGL");
    }

    /// Every number of the context's that the layer sets: what is switched on (blending,
    /// depth, stencil, culling, the scissor, sRGB), the viewport, the scissor box, blending's
    /// factors and equations, the program, the vertex array, the active unit, polygons'
    /// filling, the pixel unpack buffer and how rows are unpacked; and the first unit's
    /// texture (read with that unit made active, and the active one put back).
    fn state(checks: &Checks) -> Vec<i32> {
        let switches = [0x0BE2, 0x0B71, 0x0B90, 0x0B44, 0x0C11, 0x8DB9].map(|what| unsafe { (checks.glIsEnabled)(what) } as i32);
        let names = [0x0BA2, 0x0C10, 0x80C9, 0x80C8, 0x80CB, 0x80CA, 0x8009, 0x883D, 0x8B8D, 0x85B5, 0x84E0, 0x0B40, 0x88EF, 0x0CF2, 0x0CF3, 0x0CF4, 0x0CF5];
        let mut numbers: Vec<i32> = switches.to_vec();
        for name in names {
            let mut values = [-1; 4];
            unsafe { (checks.glGetIntegerv)(name, values.as_mut_ptr()) };
            numbers.extend(values);
        }
        let (mut unit, mut texture) = (0, 0);
        unsafe {
            (checks.glGetIntegerv)(0x84E0, &mut unit);
            (checks.glActiveTexture)(0x84C0);
            (checks.glGetIntegerv)(0x8069, &mut texture);
            (checks.glActiveTexture)(unit as u32);
        }
        numbers.push(texture);
        numbers
    }

    /// A program of the test's own, which draws nothing.
    fn program(checks: &Checks) -> u32 {
        let sources = [(0x8B31, c"#version 330 core\nvoid main() { gl_Position = vec4(0.0); }"), (0x8B30, c"#version 330 core\nout vec4 c;\nvoid main() { c = vec4(1.0); }")];
        unsafe {
            let program = (checks.glCreateProgram)();
            for (kind, source) in sources {
                let shader = (checks.glCreateShader)(kind);
                (checks.glShaderSource)(shader, 1, &source.as_ptr(), std::ptr::null());
                (checks.glCompileShader)(shader);
                (checks.glAttachShader)(program, shader);
            }
            (checks.glLinkProgram)(program);
            program
        }
    }

    /// The program has set everything the layer sets, its own way (rows unpacked from a
    /// buffer, with rows of its own length and pixels skipped, among them): the layer copies
    /// the change as it should, draws, and after it the context says the same of each as it
    /// said before.
    #[test]
    fn opengl_puts_back_what_it_sets() {
        let mut opengl = OpenGl::new(context());
        let mut gallery = Gallery::new(320, 230, 1.0);
        opengl.update(&mut gallery.ui);
        let checks = &opengl.checks;
        let (mut texture, mut corners, mut buffer) = (0, 0, 0);
        let own = program(checks);
        unsafe {
            (checks.glGenTextures)(1, &mut texture);
            (checks.glGenVertexArrays)(1, &mut corners);
            (checks.glGenBuffers)(1, &mut buffer);
            [0x0BE2, 0x0C11].into_iter().for_each(|what| (checks.glDisable)(what));
            [0x0B71, 0x0B90, 0x0B44, 0x8DB9].into_iter().for_each(|what| (checks.glEnable)(what));
            (checks.glScissor)(3, 4, 50, 60);
            (checks.glBlendFuncSeparate)(0x0300, 0x0301, 0x0304, 0x0305);
            (checks.glBlendEquationSeparate)(0x800A, 0x800B);
            (checks.glPolygonMode)(0x0408, 0x1B01);
            (checks.glUseProgram)(own);
            (checks.glBindVertexArray)(corners);
            (checks.glActiveTexture)(0x84C0);
            (checks.glBindTexture)(TEXTURE_2D, texture);
            (checks.glActiveTexture)(0x84C3);
            (checks.glBindBuffer)(0x88EC, buffer);
            [(0x0CF2, 7), (0x0CF3, 3), (0x0CF4, 2), (0x0CF5, 1)].into_iter().for_each(|(what, to)| (checks.glPixelStorei)(what, to));
        }
        gallery.ui.event(Event::Key(Key::Tab, Mods::NONE));
        gallery.run();
        let mut framebuffer = 0;
        let (width, height) = (400, 300);
        let mut target = 0;
        unsafe {
            (checks.glGenTextures)(1, &mut target);
            (checks.glActiveTexture)(0x84C1);
            (checks.glBindTexture)(TEXTURE_2D, target);
            (checks.glBindBuffer)(0x88EC, 0);
            (checks.glPixelStorei)(0x0CF2, 0);
            (checks.glTexImage2D)(TEXTURE_2D, 0, 0x8058, width, height, 0, BGRA, UNSIGNED_INT_8_8_8_8_REV, under(400, 300).as_ptr().cast());
            (checks.glPixelStorei)(0x0CF2, 7);
            (checks.glBindBuffer)(0x88EC, buffer);
            (checks.glActiveTexture)(0x84C3);
            (checks.glGenFramebuffers)(1, &mut framebuffer);
            (checks.glBindFramebuffer)(FRAMEBUFFER, framebuffer);
            (checks.glFramebufferTexture2D)(FRAMEBUFFER, 0x8CE0, TEXTURE_2D, target, 0);
            (checks.glViewport)(12, 8, 350, 260);
        }
        let before = state(checks);
        unsafe { opengl.layer.draw(&mut gallery.ui) }.unwrap();
        let checks = &opengl.checks;
        assert_eq!(state(checks), before, "the context is as the program had it");
        assert!(opengl.texture() == opengl.surface().pixels, "the change was copied as it should be");
        let mut got = vec![0u32; (width * height) as usize];
        let checks = &opengl.checks;
        unsafe {
            (checks.glBindBuffer)(0x88EB, 0);
            (checks.glReadPixels)(0, 0, width, height, BGRA, UNSIGNED_INT_8_8_8_8_REV, got.as_mut_ptr().cast());
            (checks.glBindFramebuffer)(FRAMEBUFFER, 0);
            (checks.glDeleteFramebuffers)(1, &framebuffer);
            [texture, target].iter().for_each(|name| (checks.glDeleteTextures)(1, name));
            (checks.glDeleteBuffers)(1, &buffer);
        }
        assert!(got.iter().zip(under(400, 300)).filter(|(got, under)| **got != *under).count() > 10_000, "the layer drew, though the program's states would have hidden it");
    }

    /// What goes wrong comes back as a value, with what OpenGL said: a function this OpenGL
    /// does not have, and an interface wider than its textures.
    #[test]
    fn opengl_says_what_went_wrong() {
        let (find, _keep_alive) = context();
        let failed = unsafe { Layer::new(&mut |name| if name == c"glBlendFuncSeparate" { std::ptr::null() } else { find(name) }) }.expect_err("no layer without a function");
        assert_eq!(failed.to_string(), "finding OpenGL's functions failed: this OpenGL has no glBlendFuncSeparate");
        let mut layer = unsafe { Layer::new(&mut |name| find(name)) }.unwrap();
        let checks = Checks::load(&mut |name| find(name));
        let mut most = 0;
        unsafe { (checks.glGetIntegerv)(0x0D33, &mut most) };
        let failed = unsafe { layer.draw(&mut nothing(most as usize + 1, 2)) }.expect_err("an interface wider than OpenGL's textures is not drawn");
        println!("{failed}");
        assert_eq!(failed.operation, "making the texture");
        assert!(failed.message.contains(&most.to_string()), "what OpenGL can make is said");
        unsafe { layer.delete() };
    }
}

/// The example's own window on X, through EGL: on a display made for the test by xvfb-run,
/// never on anybody's desktop (the tests check that the display is one xvfb-run made).
#[cfg(all(unix, not(target_os = "macos")))]
mod on_x11 {
    use super::opengl::Program;
    use super::opengl::egl::{self, XEvent, Xlib, dlopen, dlsym};
    use super::*;
    use std::ffi::{c_int, c_uchar, c_uint, c_ulong};
    use taut::Button;
    use taut::geom::pt;

    library!(More, "libX11.so.6", {
        XResizeWindow: fn(*mut c_void, c_ulong, c_uint, c_uint) -> c_int;
        XSync: fn(*mut c_void, c_int) -> c_int;
        XKeysymToKeycode: fn(*mut c_void, c_ulong) -> c_uchar;
    });
    library!(Fake, "libXtst.so.6", {
        XTestFakeMotionEvent: fn(*mut c_void, c_int, c_int, c_int, c_ulong) -> c_int;
        XTestFakeButtonEvent: fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int;
        XTestFakeKeyEvent: fn(*mut c_void, c_uint, c_int, c_ulong) -> c_int;
    });

    /// The display is one xvfb-run made for this run: its key to the display is in a folder of
    /// xvfb-run's own.
    fn on_a_display_of_its_own() {
        let key = std::env::var("XAUTHORITY").unwrap_or_default();
        assert!(key.contains("xvfb-run"), "run these under xvfb-run, on a display of their own (XAUTHORITY is {key:?})");
    }

    /// Every event X has for the display, handed to `each` as X gave it.
    fn read_all(x: &Xlib, display: *mut c_void, mut each: impl FnMut(&mut XEvent)) {
        while unsafe { (x.XPending)(display) } > 0 {
            let mut event = XEvent { room: [0; 24] };
            unsafe { (x.XNextEvent)(display, &mut event) };
            each(&mut event);
        }
    }

    #[test]
    #[ignore = "needs an X display: run under xvfb-run"]
    fn the_opengl_example_draws_into_its_x_window_at_any_size() {
        on_a_display_of_its_own();
        let (x, more) = (Xlib::open().unwrap(), More::open().unwrap());
        let (display, window, _) = egl::window(&x, 320, 200);
        let context = egl::context(egl::PLATFORM_X11, display, Some(window)).expect("EGL gives OpenGL 3.3 of the core profile for an X window");
        let checks = Checks::load(&mut |name| context.find(name));
        let mut program = Program::new(&mut |name| context.find(name), Ui::new(letters(), Box::new(LocalClipboard::default())));
        for (width, height) in [(320, 200), (480, 300)] {
            unsafe { ((more.XResizeWindow)(display, window, width as c_uint, height as c_uint), (more.XSync)(display, 0)) };
            read_all(&x, display, |_| {});
            program.resize(width, height, 1.0);
            program.frame();
            let mut got = vec![0u32; width * height];
            unsafe { (checks.glReadPixels)(0, 0, width as i32, height as i32, 0x80E1, 0x8367, got.as_mut_ptr().cast()) };
            let got = upside_down(&got, width);
            // The program's own colour is what shows where the interface draws nothing.
            let (ground, surface) = (got[width * height - 1], program.layer.surface());
            assert_eq!(surface.pixels[width * height - 1], 0, "the corner is clear");
            assert_eq!((surface.width, surface.height), (width, height));
            let (most, pixels) = off(&got, surface, &vec![ground; width * height]);
            println!("the example's own window, {width} by {height}: {pixels} pixels off, by {most} at most");
            assert!(most <= TOLERANCE);
            assert!(surface.pixels.iter().any(|p| p >> 24 == 255), "the interface is there");
            unsafe { (context.egl.eglSwapBuffers)(context.display, context.surface) };
        }
    }

    #[test]
    #[ignore = "needs an X display: run under xvfb-run"]
    fn what_is_done_in_an_x_window_reaches_the_interface() {
        on_a_display_of_its_own();
        let (x, more, fake) = (Xlib::open().unwrap(), More::open().unwrap(), Fake::open().expect("libXtst"));
        let (display, _, _) = egl::window(&x, 320, 200);
        unsafe { (more.XSync)(display, 0) };
        read_all(&x, display, |_| {});
        let key = |name: c_ulong| unsafe { (more.XKeysymToKeycode)(display, name) } as c_uint;
        // The pointer comes to the window, clicks, turns the wheel up; then a and Tab are typed.
        unsafe {
            (fake.XTestFakeMotionEvent)(display, -1, 30, 40, 0);
            [(1, 1), (1, 0), (4, 1), (4, 0)].iter().for_each(|(button, down)| _ = (fake.XTestFakeButtonEvent)(display, *button, *down, 0));
            [(0x61, 1), (0x61, 0), (0xff09, 1), (0xff09, 0)].iter().for_each(|(name, down)| _ = (fake.XTestFakeKeyEvent)(display, key(*name), *down, 0));
            (more.XSync)(display, 0);
        }
        let mut heard = Vec::new();
        read_all(&x, display, |event| egl::events(&x, event, 1.0, |event| heard.push(event)));
        println!("{heard:?}");
        // (Each button says first where the pointer is; and the window may be told it has the
        // keyboard.)
        assert_eq!(heard.first(), Some(&Event::Move(pt(30.0, 40.0))));
        heard.retain(|event| !matches!(event, Event::Focused(_) | Event::Move(_)));
        let wanted = [
            Event::Press(Button::Left, Mods::NONE),
            Event::Release(Button::Left),
            Event::Wheel(0.0, -48.0),
            Event::Key(Key::Char('a'), Mods::NONE),
            Event::Text("a".into()),
            Event::Key(Key::Tab, Mods::NONE),
        ];
        assert_eq!(heard, wanted);
    }
}

#[cfg(windows)]
#[allow(dead_code)]
#[path = "../examples/d3d12.rs"]
mod d3d12;

#[cfg(windows)]
mod direct3d_12 {
    use super::d3d12::direct3d::{ALLOCATOR, Com, DEVICE, FENCE, HEAP, LIST, QUEUE, RESOURCE, check, function, named, release, transition};
    use super::*;
    use std::ptr::{null, null_mut};
    use taut::layer_d3d12::Layer;

    #[link(name = "d3d12")]
    unsafe extern "system" {
        fn D3D12CreateDevice(adapter: Com, level: u32, interface: *const [u32; 4], device: *mut Com) -> i32;
    }
    #[link(name = "dxgi")]
    unsafe extern "system" {
        fn CreateDXGIFactory2(flags: u32, interface: *const [u32; 4], factory: *mut Com) -> i32;
    }
    const FACTORY: [u32; 4] = named(0x1BC6_EA02, 0xEF36, 0x464F, [0xBF, 0x0C, 0x21, 0xCA, 0x39, 0xE5, 0x16, 0x8A]);
    const ADAPTER: [u32; 4] = named(0x2411_E7E1, 0x12AC, 0x4CCF, [0xBD, 0x14, 0x97, 0x98, 0xE8, 0x53, 0x4D, 0xC0]);
    const DEVICE_5: [u32; 4] = named(0x8B4F_173B, 0x2FEA, 0x4B80, [0x8F, 0x58, 0x43, 0x07, 0x19, 0x1A, 0xB9, 0x5D]);
    // Resource states.
    const RENDER_TARGET: u32 = 0x4;
    const PIXEL_SHADER_RESOURCE: u32 = 0x80;
    const COPY_DEST: u32 = 0x400;
    const COPY_SOURCE: u32 = 0x800;
    const GENERIC_READ: u32 = 0xAC3;
    /// Rows of a texture in a buffer are a multiple of this many bytes apart.
    const PITCH: usize = 256;

    /// Direct3D 12 makes one device per adapter in a program: a test that removes it (or makes
    /// one) would take it from a test running beside it. So they take turns.
    fn one_device() -> std::sync::MutexGuard<'static, ()> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[repr(C)]
    struct Description {
        dimension: u32,
        alignment: u64,
        width: u64,
        height: u32,
        depth: u16,
        levels: u16,
        format: u32,
        samples: [u32; 2],
        layout: u32,
        flags: u32,
    }

    #[repr(C)]
    struct CopyPlace {
        resource: Com,
        kind: u32,
        offset: u64,
        footprint: [u32; 5],
    }

    /// A device of Direct3D 12 on Windows' own software renderer (WARP), which needs no
    /// graphics card and no window; its queue; and a command list of the test's own.
    pub struct Gpu {
        pub device: Com,
        pub queue: Com,
        allocator: Com,
        list: Com,
        fence: Com,
        fenced: u64,
    }

    impl Gpu {
        fn warp() -> Gpu {
            let (mut factory, mut adapter, mut device, mut queue) = (null_mut(), null_mut(), null_mut(), null_mut());
            unsafe {
                check(CreateDXGIFactory2(0, &FACTORY, &mut factory), "making a DXGI factory");
                check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(factory, 27)(factory, &ADAPTER, &mut adapter), "finding WARP");
                check(D3D12CreateDevice(adapter, 0xB000, &DEVICE, &mut device), "making a device");
                check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(device, 8)(device, &[0; 4], &QUEUE, &mut queue), "making a queue");
                [adapter, factory].into_iter().for_each(|object| release(object));
            }
            Gpu::on(device, queue)
        }

        /// The test's own commands on the device and queue (whose references it takes over).
        fn on(device: Com, queue: Com) -> Gpu {
            let mut gpu = Gpu { device, queue, allocator: null_mut(), list: null_mut(), fence: null_mut(), fenced: 0 };
            unsafe {
                check(function::<unsafe extern "system" fn(Com, u32, *const [u32; 4], *mut Com) -> i32>(device, 9)(device, 0, &ALLOCATOR, &mut gpu.allocator), "making an allocator");
                check(
                    function::<unsafe extern "system" fn(Com, u32, u32, Com, Com, *const [u32; 4], *mut Com) -> i32>(device, 12)(device, 0, 0, gpu.allocator, null_mut(), &LIST, &mut gpu.list),
                    "making a list",
                );
                check(function::<unsafe extern "system" fn(Com) -> i32>(gpu.list, 9)(gpu.list), "closing the list");
                check(function::<unsafe extern "system" fn(Com, u64, u32, *const [u32; 4], *mut Com) -> i32>(device, 36)(device, 0, 0, &FENCE, &mut gpu.fence), "making a fence");
            }
            gpu
        }

        /// Records with `record`, runs it, and waits until it is done.
        fn run(&mut self, record: &mut dyn FnMut(Com)) {
            let list = self.list;
            self.fenced += 1;
            unsafe {
                check(function::<unsafe extern "system" fn(Com) -> i32>(self.allocator, 8)(self.allocator), "resetting the allocator");
                check(function::<unsafe extern "system" fn(Com, Com, Com) -> i32>(list, 10)(list, self.allocator, null_mut()), "resetting the list");
                record(list);
                check(function::<unsafe extern "system" fn(Com) -> i32>(list, 9)(list), "closing the list");
                function::<unsafe extern "system" fn(Com, u32, *const Com)>(self.queue, 10)(self.queue, 1, &list);
                function::<unsafe extern "system" fn(Com, Com, u64) -> i32>(self.queue, 14)(self.queue, self.fence, self.fenced);
                function::<unsafe extern "system" fn(Com, u64, isize) -> i32>(self.fence, 9)(self.fence, self.fenced, 0);
            }
        }

        /// A committed resource in heap `heap` (1: the device's, 2: written by the processor,
        /// 3: read by it).
        fn resource(&self, heap: u32, description: &Description, state: u32) -> Com {
            let (properties, mut made) = ([heap, 0, 0, 0, 0], null_mut());
            type Make = unsafe extern "system" fn(Com, *const [u32; 5], u32, *const Description, u32, *const c_void, *const [u32; 4], *mut Com) -> i32;
            check(unsafe { function::<Make>(self.device, 27)(self.device, &properties, 0, description, state, null(), &RESOURCE, &mut made) }, "making a resource");
            made
        }

        /// A texture of B8G8R8A8_UNORM, drawn into if `target`.
        fn texture(&self, width: usize, height: usize, target: bool, state: u32) -> Com {
            let description = Description { dimension: 3, alignment: 0, width: width as u64, height: height as u32, depth: 1, levels: 1, format: 87, samples: [1, 0], layout: 0, flags: target as u32 };
            self.resource(1, &description, state)
        }

        /// A buffer of `size` bytes in heap `heap`.
        fn buffer(&self, heap: u32, size: usize) -> Com {
            let description = Description { dimension: 1, alignment: 0, width: size as u64, height: 1, depth: 1, levels: 1, format: 0, samples: [1, 0], layout: 1, flags: 0 };
            self.resource(heap, &description, if heap == 2 { GENERIC_READ } else { COPY_DEST })
        }

        /// Copies between a texture `width` by `height` and a buffer laid out with rows `PITCH`
        /// apart (to the texture if `to_texture`), the texture in `state` before and after.
        fn copy(&mut self, texture: Com, state: u32, buffer: Com, [width, height]: [usize; 2], to_texture: bool) {
            let pitch = (width * 4).div_ceil(PITCH) * PITCH;
            let footprint = [87, width as u32, height as u32, 1, pitch as u32];
            let (whole, laid) = (CopyPlace { resource: texture, kind: 0, offset: 0, footprint: [0; 5] }, CopyPlace { resource: buffer, kind: 1, offset: 0, footprint });
            let (copying, [to, from]) = if to_texture { (COPY_DEST, [&whole, &laid]) } else { (COPY_SOURCE, [&laid, &whole]) };
            type CopyRegion = unsafe extern "system" fn(Com, *const CopyPlace, u32, u32, u32, *const CopyPlace, *const c_void);
            self.run(&mut |list| unsafe {
                let barriers = function::<unsafe extern "system" fn(Com, u32, *const super::d3d12::direct3d::Barrier)>(list, 26);
                barriers(list, 1, &transition(texture, state, copying));
                function::<CopyRegion>(list, 16)(list, to, 0, 0, 0, from, null());
                barriers(list, 1, &transition(texture, copying, state));
            });
        }

        /// Puts `pixels` into all of a texture `width` wide that is in `state`.
        fn put(&mut self, texture: Com, state: u32, pixels: &[u32], width: usize) {
            let (height, pitch) = (pixels.len() / width, (width * 4).div_ceil(PITCH) * PITCH);
            let buffer = self.buffer(2, pitch * height);
            let mut there: *mut u8 = null_mut();
            unsafe {
                check(function::<unsafe extern "system" fn(Com, u32, *const c_void, *mut *mut u8) -> i32>(buffer, 8)(buffer, 0, null(), &mut there), "mapping");
                for (y, row) in pixels.chunks(width).enumerate() {
                    std::ptr::copy_nonoverlapping(row.as_ptr().cast::<u8>(), there.add(y * pitch), width * 4);
                }
            }
            self.copy(texture, state, buffer, [width, height], true);
            unsafe { release(buffer) };
        }

        /// The pixels of a texture `width` by `height` that is in `state`.
        fn read(&mut self, texture: Com, state: u32, width: usize, height: usize) -> Vec<u32> {
            let pitch = (width * 4).div_ceil(PITCH) * PITCH;
            let buffer = self.buffer(3, pitch * height);
            self.copy(texture, state, buffer, [width, height], false);
            let (mut there, mut pixels): (*mut u8, _) = (null_mut(), vec![0u32; width * height]);
            unsafe {
                check(function::<unsafe extern "system" fn(Com, u32, *const [usize; 2], *mut *mut u8) -> i32>(buffer, 8)(buffer, 0, &[0, pitch * height], &mut there), "mapping");
                for (y, row) in pixels.chunks_mut(width).enumerate() {
                    std::ptr::copy_nonoverlapping(there.add(y * pitch), row.as_mut_ptr().cast::<u8>(), width * 4);
                }
                release(buffer);
            }
            pixels
        }
    }

    impl Drop for Gpu {
        fn drop(&mut self) {
            unsafe { [self.fence, self.list, self.allocator, self.queue, self.device].into_iter().for_each(|object| release(object)) };
        }
    }

    /// The layer, on WARP.
    struct Direct3D {
        layer: Option<Layer>,
        gpu: Gpu,
    }

    impl Direct3D {
        fn new() -> Direct3D {
            let gpu = Gpu::warp();
            let layer = unsafe { Layer::new(gpu.device, gpu.queue, 87) }.unwrap_or_else(|failed| panic!("{failed}"));
            Direct3D { layer: Some(layer), gpu }
        }

        fn layer(&mut self) -> &mut Layer {
            self.layer.as_mut().unwrap()
        }
    }

    impl Renderer for Direct3D {
        fn update(&mut self, ui: &mut Ui) {
            self.layer().update(ui).unwrap_or_else(|failed| panic!("{failed}"));
        }
        fn drawn(&mut self, ui: &mut Ui, under: &[u32], [width, height]: [usize; 2], area: [i32; 4]) -> Vec<u32> {
            let target = self.gpu.texture(width, height, true, RENDER_TARGET);
            self.gpu.put(target, RENDER_TARGET, under, width);
            let (mut heap, mut view) = (null_mut(), 0usize);
            let layer = self.layer.as_mut().unwrap();
            unsafe {
                check(
                    function::<unsafe extern "system" fn(Com, *const [u32; 4], *const [u32; 4], *mut Com) -> i32>(self.gpu.device, 14)(self.gpu.device, &[2, 1, 0, 0], &HEAP, &mut heap),
                    "making a view's heap",
                );
                function::<unsafe extern "system" fn(Com, *mut usize) -> *mut usize>(heap, 9)(heap, &mut view);
                function::<unsafe extern "system" fn(Com, Com, *const c_void, usize)>(self.gpu.device, 20)(self.gpu.device, target, null(), view);
            }
            self.gpu.run(&mut |list| unsafe {
                function::<unsafe extern "system" fn(Com, u32, *const usize, i32, *const usize)>(list, 46)(list, 1, &view, 0, null());
                layer.draw(ui, list, area).unwrap_or_else(|failed| panic!("{failed}"));
            });
            let got = self.gpu.read(target, RENDER_TARGET, width, height);
            unsafe { [heap, target].into_iter().for_each(|object| release(object)) };
            got
        }
        fn fill(&mut self, pixels: &[u32]) {
            let (texture, width) = (self.layer().texture(), self.layer().surface().width);
            self.gpu.put(texture, PIXEL_SHADER_RESOURCE, pixels, width);
        }
        fn texture(&mut self) -> Vec<u32> {
            let (texture, width, height) = (self.layer().texture(), self.layer().surface().width, self.layer().surface().height);
            self.gpu.read(texture, PIXEL_SHADER_RESOURCE, width, height)
        }
        fn surface(&self) -> &Surface {
            self.layer.as_ref().unwrap().surface()
        }
    }

    #[test]
    fn direct3d_12_shows_the_surface_over_what_was_drawn() {
        let _one = one_device();
        shows_the_surface_over_what_was_drawn(&mut Direct3D::new(), "Direct3D 12");
    }

    #[test]
    fn direct3d_12_is_given_only_the_places_that_changed() {
        let _one = one_device();
        is_given_only_the_places_that_changed(&mut Direct3D::new(), "Direct3D 12");
    }

    #[test]
    fn direct3d_12_lies_at_the_programs_viewport() {
        let _one = one_device();
        lies_at_the_programs_viewport(&mut Direct3D::new(), "Direct3D 12");
    }

    /// The device is removed (as `ID3D12Device5::RemoveDevice` makes it be, which is how a
    /// driver's update or a crash looks): the layer says so in Windows' words, and lets go of
    /// everything without waiting for ever. A layer on a new device then draws as before.
    #[test]
    fn direct3d_12_survives_a_removed_device() {
        let _one = one_device();
        let mut direct3d = Direct3D::new();
        let mut gallery = Gallery::new(480, 320, 1.0);
        let under = under(480, 320);
        let got = drawn_whole(&mut direct3d, &mut gallery.ui, &under);
        assert!(off(&got, &Surface::draw(&mut gallery.ui, 64), &under).0 <= TOLERANCE);
        unsafe {
            let mut removable = null_mut();
            check(function::<unsafe extern "system" fn(Com, *const [u32; 4], *mut Com) -> i32>(direct3d.gpu.device, 0)(direct3d.gpu.device, &DEVICE_5, &mut removable), "asking for ID3D12Device5");
            function::<unsafe extern "system" fn(Com)>(removable, 58)(removable);
            release(removable);
        }
        gallery.ui.event(Event::Key(Key::Tab, Mods::NONE));
        gallery.run();
        let failed = direct3d.layer().update(&mut gallery.ui).expect_err("a removed device draws nothing");
        println!("{failed}");
        assert_eq!(failed.operation, "drawing");
        assert_eq!(failed.code as u32 & 0xFFFF_0000, 0x887A_0000, "a DXGI error: {failed}");
        assert!(!failed.message.is_empty());
        let started = std::time::Instant::now();
        drop(direct3d.layer.take());
        assert!(started.elapsed().as_secs() < 5, "letting go took {:?}", started.elapsed());
        // (Direct3D 12 makes one device per adapter: the removed one is let go of first.)
        drop(direct3d);
        let mut again = Direct3D::new();
        let got = drawn_whole(&mut again, &mut gallery.ui, &under);
        assert!(off(&got, &Surface::draw(&mut gallery.ui, 64), &under).0 <= TOLERANCE, "a new layer on a new device");
    }

    /// What goes wrong comes back as a value, with Windows' own words: render targets of a
    /// depth format (D32_FLOAT, 40), which no render target has.
    #[test]
    fn direct3d_12_says_what_went_wrong() {
        let _one = one_device();
        let gpu = Gpu::warp();
        let failed = unsafe { Layer::new(gpu.device, gpu.queue, 40) }.expect_err("no pipeline draws into targets of a depth format");
        println!("{failed}");
        assert_eq!((failed.operation, failed.code as u32), ("making the pipeline state", 0x8007_0057));
        assert!(!failed.message.is_empty() && failed.to_string().contains(&failed.message), "Windows' words are said");
    }

    #[test]
    fn the_direct3d_12_example_draws_into_its_window_at_any_size() {
        let _one = one_device();
        let window = super::direct3d_11::unseen_window();
        let mut device = d3d12::direct3d::Device::new(window, true);
        let mut layer = unsafe { Layer::new(device.device, device.queue, 87) }.unwrap();
        for (width, height) in [(320, 200), (480, 300)] {
            device.resize(width as u32, height as u32);
            let mut gallery = Gallery::new(width, height, 1.0);
            device.begin([51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 1.0]);
            unsafe { layer.draw(&mut gallery.ui, device.list, [0, 0, width as i32, height as i32]) }.unwrap();
            device.finish();
            let buffer = device.buffers[device.current()];
            unsafe { function::<unsafe extern "system" fn(Com) -> u32>(buffer, 1)(buffer) };
            let mut reader = Gpu::on(device.device, device.queue);
            unsafe { [device.device, device.queue].into_iter().for_each(|object| _ = function::<unsafe extern "system" fn(Com) -> u32>(object, 1)(object)) };
            let got = reader.read(buffer, 0, width, height);
            drop(reader);
            unsafe { release(buffer) };
            let (most, pixels) = off(&got, layer.surface(), &vec![0xff33_6699; width * height]);
            println!("the example's own device, {width} by {height}: {pixels} pixels off, by {most} at most");
            assert!(most <= TOLERANCE);
            device.show();
        }
        drop((layer, device));
        super::direct3d_11::close(window);
    }
}

#[cfg(windows)]
#[allow(dead_code)]
#[path = "../examples/d3d9.rs"]
mod d3d9;

/// Direct3D 9 has no software device: these draw on the graphics card. Each runs in a program of
/// its own, started on a desktop of its own (Direct3D 9 needs a window, which is never shown
/// there either), and ends within two minutes.
#[cfg(windows)]
mod direct3d_9 {
    use super::d3d9::direct3d::{Com, D3DPRESENT_PARAMETERS, Device, check, function, release};
    use super::*;
    use std::ptr::{null, null_mut};
    use taut::layer_d3d9::Layer;

    /// Said to a program that a test started on a desktop of its own: where to say it passed.
    const ALONE: &str = "TAUT_RENDERERS_ALONE";

    #[repr(C)]
    struct Start {
        size: u32,
        reserved: *const u16,
        desktop: *const u16,
        title: *const u16,
        numbers: [u32; 8],
        show: [u16; 2],
        more: *const u8,
        handles: [isize; 3],
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn CreateDesktopW(name: *const u16, device: *const u16, mode: *const c_void, flags: u32, access: u32, security: *const c_void) -> isize;
        fn CloseDesktop(desktop: isize) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateProcessW(app: *const u16, line: *mut u16, process: usize, thread: usize, inherit: i32, flags: u32, env: usize, dir: usize, start: *const Start, made: *mut [isize; 3]) -> i32;
        fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
        fn TerminateProcess(handle: isize, code: u32) -> i32;
        fn GetExitCodeProcess(handle: isize, code: *mut u32) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn SetHandleInformation(handle: isize, mask: u32, flags: u32) -> i32;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// Runs the ignored test `name` in this test program started anew on a desktop of its own,
    /// its output kept in a file; and fails as it fails.
    fn on_a_desktop_of_its_own(name: &str) {
        let folder = std::env::temp_dir();
        let (log, passed) = (folder.join(format!("taut-{}-{name}.txt", std::process::id())), folder.join(format!("taut-{}-{name}.passed", std::process::id())));
        _ = std::fs::remove_file(&passed);
        let output = std::fs::File::create(&log).unwrap();
        let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&output) as isize;
        let program = std::env::current_exe().unwrap();
        let mut line = wide(&format!("\"{}\" --exact direct3d_9::{name} --ignored --nocapture --test-threads=1", program.display()));
        let desktop = format!("taut renderers {} {name}", std::process::id());
        let place = wide(&format!("WinSta0\\{desktop}"));
        // (0x100: the output handles below are the child's.)
        let start = Start {
            size: size_of::<Start>() as u32,
            reserved: null(),
            desktop: place.as_ptr(),
            title: null(),
            numbers: [0, 0, 0, 0, 0, 0, 0, 0x100],
            show: [0; 2],
            more: null(),
            handles: [0, handle, handle],
        };
        let mut made = [0isize; 3];
        // SAFETY: plain calls with whole records. The child is waited for two minutes at most,
        // then ended; every handle is closed.
        let code = unsafe {
            SetHandleInformation(handle, 1, 1);
            let desk = CreateDesktopW(wide(&desktop).as_ptr(), null(), null(), 0, 0x1000_0000, null());
            assert!(desk != 0, "a desktop of its own could not be made");
            let environment = std::env::vars().map(|(key, value)| format!("{key}={value}\0")).chain([format!("{ALONE}={}\0\0", passed.display())]).collect::<String>();
            let environment: Vec<u16> = environment.encode_utf16().collect();
            // (0x400: the environment is in UTF-16; 0x0800_0000: no console.)
            assert!(CreateProcessW(null(), line.as_mut_ptr(), 0, 0, 1, 0x400 | 0x0800_0000, environment.as_ptr() as usize, 0, &start, &mut made) != 0, "the test could not be started");
            let timed_out = WaitForSingleObject(made[0], 120_000) != 0;
            let mut code = 1;
            if timed_out {
                TerminateProcess(made[0], 1);
            }
            GetExitCodeProcess(made[0], &mut code);
            [made[0], made[1]].into_iter().for_each(|handle| _ = CloseHandle(handle));
            CloseDesktop(desk);
            if timed_out { 258 } else { code }
        };
        drop(output);
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        println!("{text}");
        assert!(code == 0 && passed.exists(), "{name} ended with {code}");
        _ = std::fs::remove_file(log);
        _ = std::fs::remove_file(passed);
    }

    /// Whether this program was started by `on_a_desktop_of_its_own`; the test runs only then.
    fn alone() -> bool {
        std::env::var_os(ALONE).is_some()
    }

    /// Marks this program's test passed, for the program that started it.
    fn passed() {
        std::fs::write(std::env::var_os(ALONE).unwrap(), "passed").unwrap();
    }

    /// `D3DVIEWPORT9`.
    #[repr(C)]
    #[derive(Debug, PartialEq, Default)]
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

    /// A surface of A8R8G8B8 (21) in system memory (2) (device 36).
    fn in_memory(device: Com, width: usize, height: usize) -> Com {
        let mut made = null_mut();
        check(
            unsafe { function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, *mut Com, *mut isize) -> i32>(device, 36)(device, width as u32, height as u32, 21, 2, &mut made, null_mut()) },
            "making a surface",
        );
        made
    }

    /// Writes `pixels` into a surface in memory `width` wide, or reads them from it (surface 13
    /// locks, 14 unlocks).
    fn rows(surface: Com, pixels: &mut [u32], width: usize, write: bool) {
        let mut locked = Locked { pitch: 0, bits: null_mut() };
        unsafe {
            check(function::<unsafe extern "system" fn(Com, *mut Locked, *const c_void, u32) -> i32>(surface, 13)(surface, &mut locked, null(), 0), "locking");
            for (y, row) in pixels.chunks_mut(width).enumerate() {
                let there = locked.bits.add(y * locked.pitch as usize);
                match write {
                    true => std::ptr::copy_nonoverlapping(row.as_ptr().cast::<u8>(), there, width * 4),
                    false => std::ptr::copy_nonoverlapping(there, row.as_mut_ptr().cast::<u8>(), width * 4),
                }
            }
            function::<unsafe extern "system" fn(Com) -> i32>(surface, 14)(surface);
        }
    }

    /// Puts `pixels` into all of `surface`, which is in the device's memory (device 30:
    /// UpdateSurface).
    fn put(device: Com, surface: Com, pixels: &[u32], width: usize) {
        let copy = in_memory(device, width, pixels.len() / width);
        rows(copy, &mut pixels.to_vec(), width, true);
        check(unsafe { function::<unsafe extern "system" fn(Com, Com, *const c_void, Com, *const c_void) -> i32>(device, 30)(device, copy, null(), surface, null()) }, "putting pixels");
        unsafe { release(copy) };
    }

    /// The pixels of a render target (device 32: GetRenderTargetData).
    fn read(device: Com, target: Com, width: usize, height: usize) -> Vec<u32> {
        let (copy, mut pixels) = (in_memory(device, width, height), vec![0u32; width * height]);
        check(unsafe { function::<unsafe extern "system" fn(Com, Com, Com) -> i32>(device, 32)(device, target, copy) }, "reading a render target");
        rows(copy, &mut pixels, width, false);
        unsafe { release(copy) };
        pixels
    }

    /// A render target of A8R8G8B8 (device 28), holding `under`.
    fn target(device: Com, under: &[u32], width: usize) -> Com {
        let mut made = null_mut();
        check(
            unsafe {
                function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, u32, i32, *mut Com, *mut isize) -> i32>(device, 28)(
                    device,
                    width as u32,
                    (under.len() / width) as u32,
                    21,
                    0,
                    0,
                    0,
                    &mut made,
                    null_mut(),
                )
            },
            "making a render target",
        );
        put(device, made, under, width);
        made
    }

    /// Draws with `draw` into `target`, in a scene, with the viewport `[x, y, width, height]`;
    /// the program's render target put back after (device 38 gets it, 37 sets it, 47 sets the
    /// viewport, 41 and 42 begin and end the scene).
    fn into(device: Com, target: Com, [x, y, width, height]: [i32; 4], draw: &mut dyn FnMut()) {
        let (mut was, viewport) = (null_mut(), Viewport { x: x as u32, y: y as u32, width: width as u32, height: height as u32, near: 0.0, far: 1.0 });
        unsafe {
            function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(device, 38)(device, 0, &mut was);
            check(function::<unsafe extern "system" fn(Com, u32, Com) -> i32>(device, 37)(device, 0, target), "setting the render target");
            check(function::<unsafe extern "system" fn(Com, *const Viewport) -> i32>(device, 47)(device, &viewport), "setting the viewport");
            check(function::<unsafe extern "system" fn(Com) -> i32>(device, 41)(device), "beginning the scene");
            draw();
            check(function::<unsafe extern "system" fn(Com) -> i32>(device, 42)(device), "ending the scene");
            function::<unsafe extern "system" fn(Com, u32, Com) -> i32>(device, 37)(device, 0, was);
            release(was);
        }
    }

    /// The layer on a device of the graphics card, of the 9Ex kind or the first, showing in a
    /// window that is never shown.
    struct Direct3D {
        layer: Layer,
        device: Device,
        window: isize,
    }

    impl Direct3D {
        fn new(ex: bool) -> Direct3D {
            let window = super::direct3d_11::unseen_window();
            let device = Device::new(window, ex);
            let layer = unsafe { Layer::new(device.device) }.unwrap_or_else(|failed| panic!("{failed}"));
            Direct3D { layer, device, window }
        }
    }

    impl Drop for Direct3D {
        fn drop(&mut self) {
            super::direct3d_11::close(self.window);
        }
    }

    impl Renderer for Direct3D {
        fn update(&mut self, ui: &mut Ui) {
            self.layer.update(ui).unwrap_or_else(|failed| panic!("{failed}"));
        }
        fn drawn(&mut self, ui: &mut Ui, under: &[u32], [width, height]: [usize; 2], viewport: [i32; 4]) -> Vec<u32> {
            let (device, layer) = (self.device.device, &mut self.layer);
            let target = target(device, under, width);
            into(device, target, viewport, &mut || layer.draw(ui).unwrap_or_else(|failed| panic!("{failed}")));
            let got = read(device, target, width, height);
            unsafe { release(target) };
            got
        }
        fn fill(&mut self, pixels: &[u32]) {
            let (device, texture, mut level) = (self.device.device, self.layer.texture(), null_mut());
            unsafe { check(function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(texture, 18)(texture, 0, &mut level), "finding the texture's surface") };
            put(device, level, pixels, self.layer.surface().width);
            unsafe { release(level) };
        }
        /// The texture, drawn as it is (no blending) into a render target of its size, and read.
        fn texture(&mut self) -> Vec<u32> {
            let (device, texture) = (self.device.device, self.layer.texture());
            let (width, height) = (self.layer.surface().width, self.layer.surface().height);
            let target = target(device, &vec![0; width * height], width);
            let (right, bottom) = (width as f32 - 0.5, height as f32 - 0.5);
            let corners: [[f32; 6]; 4] = [[-0.5, -0.5, 0.0, 1.0, 0.0, 0.0], [right, -0.5, 0.0, 1.0, 1.0, 0.0], [-0.5, bottom, 0.0, 1.0, 0.0, 1.0], [right, bottom, 0.0, 1.0, 1.0, 1.0]];
            into(device, target, [0, 0, width as i32, height as i32], &mut || unsafe {
                function::<unsafe extern "system" fn(Com, u32) -> i32>(device, 89)(device, 0x104);
                function::<unsafe extern "system" fn(Com, u32, Com) -> i32>(device, 65)(device, 0, texture);
                for (state, to) in [(7, 0), (22, 1), (27, 0), (28, 0), (137, 0), (174, 0)] {
                    function::<unsafe extern "system" fn(Com, u32, u32) -> i32>(device, 57)(device, state, to);
                }
                for (state, to) in [(1, 2), (2, 2), (4, 2), (5, 2)] {
                    function::<unsafe extern "system" fn(Com, u32, u32, u32) -> i32>(device, 67)(device, 0, state, to);
                }
                for (state, to) in [(5, 1), (6, 1), (7, 0)] {
                    function::<unsafe extern "system" fn(Com, u32, u32, u32) -> i32>(device, 69)(device, 0, state, to);
                }
                function::<unsafe extern "system" fn(Com, u32, u32, *const c_void, u32) -> i32>(device, 83)(device, 5, 2, corners.as_ptr().cast(), 24);
                function::<unsafe extern "system" fn(Com, u32, Com) -> i32>(device, 65)(device, 0, null_mut());
            });
            let got = read(device, target, width, height);
            unsafe { release(target) };
            got
        }
        fn surface(&self) -> &Surface {
            self.layer.surface()
        }
    }

    #[test]
    fn direct3d_9_shows_the_surface_over_what_was_drawn() {
        on_a_desktop_of_its_own("shows_the_surface_over_what_was_drawn_alone");
    }

    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn shows_the_surface_over_what_was_drawn_alone() {
        if alone() {
            shows_the_surface_over_what_was_drawn(&mut Direct3D::new(true), "Direct3D 9Ex");
            passed();
        }
    }

    #[test]
    fn direct3d_9_is_given_only_the_places_that_changed() {
        on_a_desktop_of_its_own("is_given_only_the_places_that_changed_alone");
    }

    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn is_given_only_the_places_that_changed_alone() {
        if alone() {
            is_given_only_the_places_that_changed(&mut Direct3D::new(true), "Direct3D 9Ex");
            passed();
        }
    }

    #[test]
    fn direct3d_9_lies_at_the_programs_viewport() {
        on_a_desktop_of_its_own("lies_at_the_programs_viewport_alone");
    }

    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn lies_at_the_programs_viewport_alone() {
        if alone() {
            lies_at_the_programs_viewport(&mut Direct3D::new(true), "Direct3D 9Ex");
            passed();
        }
    }

    /// Every state the layer sets, as the device says it now: render states, texture stage
    /// states and sampler states the layer sets, the texture of stage 0, the vertex format and
    /// declaration, both shaders, stream 0, the scissor rectangle and the viewport.
    fn state(device: Com) -> Vec<u64> {
        let mut numbers = Vec::new();
        let mut number = |value: u64| numbers.push(value);
        unsafe {
            for state in [7, 8, 14, 15, 19, 20, 22, 26, 27, 28, 29, 52, 128, 136, 137, 152, 168, 171, 174, 194, 206] {
                let mut value = 0u32;
                function::<unsafe extern "system" fn(Com, u32, *mut u32) -> i32>(device, 58)(device, state, &mut value);
                number(value as u64);
            }
            for (stage, state) in [(0, 1), (0, 2), (0, 4), (0, 5), (0, 11), (0, 24), (1, 1), (1, 4)] {
                let mut value = 0u32;
                function::<unsafe extern "system" fn(Com, u32, u32, *mut u32) -> i32>(device, 66)(device, stage, state, &mut value);
                number(value as u64);
            }
            for state in [1, 2, 5, 6, 7, 11] {
                let mut value = 0u32;
                function::<unsafe extern "system" fn(Com, u32, u32, *mut u32) -> i32>(device, 68)(device, 0, state, &mut value);
                number(value as u64);
            }
            // Objects: the texture (64), the vertex declaration (88), the vertex and pixel
            // shaders (93, 108); and stream 0 (101: GetStreamSource), each let go of once read.
            for at in [64usize, 88, 93, 108] {
                let mut object = null_mut();
                match at {
                    64 => function::<unsafe extern "system" fn(Com, u32, *mut Com) -> i32>(device, at)(device, 0, &mut object),
                    _ => function::<unsafe extern "system" fn(Com, *mut Com) -> i32>(device, at)(device, &mut object),
                };
                number(object as u64);
                release(object);
            }
            let (mut stream, mut offset, mut stride) = (null_mut(), 0u32, 0u32);
            function::<unsafe extern "system" fn(Com, u32, *mut Com, *mut u32, *mut u32) -> i32>(device, 101)(device, 0, &mut stream, &mut offset, &mut stride);
            [stream as u64, offset as u64, stride as u64].into_iter().for_each(&mut number);
            release(stream);
            let (mut format, mut scissor, mut viewport) = (0u32, [0i32; 4], Viewport::default());
            function::<unsafe extern "system" fn(Com, *mut u32) -> i32>(device, 90)(device, &mut format);
            function::<unsafe extern "system" fn(Com, *mut [i32; 4]) -> i32>(device, 76)(device, &mut scissor);
            function::<unsafe extern "system" fn(Com, *mut Viewport) -> i32>(device, 48)(device, &mut viewport);
            number(format as u64);
            scissor.into_iter().for_each(|edge| number(edge as u64));
            [viewport.x, viewport.y, viewport.width, viewport.height, viewport.near.to_bits(), viewport.far.to_bits()].into_iter().for_each(|value| number(value as u64));
        }
        numbers
    }

    #[test]
    fn direct3d_9_puts_back_what_it_sets() {
        on_a_desktop_of_its_own("puts_back_what_it_sets_alone");
    }

    /// The program has set everything the layer sets, its own way: after the layer is drawn,
    /// the device says the same of each as it said before.
    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn puts_back_what_it_sets_alone() {
        if !alone() {
            return;
        }
        let mut direct3d = Direct3D::new(true);
        let device = direct3d.device.device;
        let mut gallery = Gallery::new(320, 230, 1.0);
        let (texture, mut buffer) = (null_mut::<c_void>(), null_mut());
        let _ = texture;
        unsafe {
            // Render states, each to something else than the layer sets.
            for (state, to) in [
                (7, 1),
                (8, 2),
                (14, 1),
                (15, 1),
                (19, 5),
                (20, 2),
                (22, 3),
                (26, 1),
                (27, 0),
                (28, 1),
                (29, 1),
                (52, 1),
                (128, 1),
                (136, 0),
                (137, 1),
                (152, 1),
                (168, 0x7),
                (171, 2),
                (174, 0),
                (194, 1),
                (206, 1),
            ] {
                function::<unsafe extern "system" fn(Com, u32, u32) -> i32>(device, 57)(device, state, to);
            }
            for (stage, state, to) in [(0, 1, 4), (0, 2, 1), (0, 4, 4), (0, 5, 1), (0, 11, 1), (0, 24, 2), (1, 1, 2), (1, 4, 2)] {
                function::<unsafe extern "system" fn(Com, u32, u32, u32) -> i32>(device, 67)(device, stage, state, to);
            }
            for (state, to) in [(1, 1), (2, 2), (5, 2), (6, 2), (7, 2), (11, 1)] {
                function::<unsafe extern "system" fn(Com, u32, u32, u32) -> i32>(device, 69)(device, 0, state, to);
            }
            // A vertex buffer of its own in stream 0 (device 26: CreateVertexBuffer; 100:
            // SetStreamSource), a vertex format of its own, and a scissor rectangle.
            check(function::<unsafe extern "system" fn(Com, u32, u32, u32, u32, *mut Com, *mut isize) -> i32>(device, 26)(device, 96, 0, 0x102, 0, &mut buffer, null_mut()), "making a vertex buffer");
            function::<unsafe extern "system" fn(Com, u32, Com, u32, u32) -> i32>(device, 100)(device, 0, buffer, 0, 24);
            function::<unsafe extern "system" fn(Com, u32) -> i32>(device, 89)(device, 0x102);
            function::<unsafe extern "system" fn(Com, *const [i32; 4]) -> i32>(device, 75)(device, &[3, 4, 50, 60]);
        }
        let under = under(400, 300);
        let target = target(device, &under, 400);
        let mut before = Vec::new();
        let layer = &mut direct3d.layer;
        let mut got = Vec::new();
        into(device, target, [12, 8, 350, 260], &mut || {
            before = state(device);
            layer.draw(&mut gallery.ui).unwrap();
            assert_eq!(state(device), before, "the device is as the program had it");
        });
        got.extend(read(device, target, 400, 300));
        unsafe { [target, buffer].into_iter().for_each(|object| release(object)) };
        assert!(got.iter().zip(&under).filter(|(got, under)| got != under).count() > 10_000, "the layer drew, though the program's states would have hidden it");
        passed();
    }

    #[test]
    fn direct3d_9_takes_a_reset_and_a_resize() {
        on_a_desktop_of_its_own("takes_a_reset_and_a_resize_alone");
    }

    /// The device is reset, as after it was lost, with the layer's texture let go of first
    /// (`before_reset`, which a device of the first kind needs) and without (9Ex keeps it): the
    /// layer draws as before. The same at a new size. (A device of the first kind is not made
    /// on a desktop of its own: it says D3DERR_INVALIDCALL there.)
    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn takes_a_reset_and_a_resize_alone() {
        if !alone() {
            return;
        }
        for let_go in [true, false] {
            let mut direct3d = Direct3D::new(true);
            let mut gallery = Gallery::new(320, 200, 1.0);
            let under = under(320, 200);
            let got = drawn_whole(&mut direct3d, &mut gallery.ui, &under);
            assert!(off(&got, &Surface::draw(&mut gallery.ui, 64), &under).0 <= TOLERANCE);
            let params: D3DPRESENT_PARAMETERS = direct3d.device.shown;
            if let_go {
                direct3d.layer.before_reset();
                assert!(direct3d.layer.texture().is_null());
            }
            check(direct3d.device.resize(params.width, params.height), "resetting");
            let got = drawn_whole(&mut direct3d, &mut gallery.ui, &under);
            let (most, _) = off(&got, &Surface::draw(&mut gallery.ui, 64), &under);
            println!("let go first: {let_go}; after a reset, off by {most} at most");
            assert!(most <= TOLERANCE, "after a reset");
            gallery.ui.resize(Size { w: 480.0, h: 300.0 }, 1.0);
            gallery.run();
            if let_go {
                direct3d.layer.before_reset();
            }
            check(direct3d.device.resize(480, 300), "resizing");
            let under = super::under(480, 300);
            let got = drawn_whole(&mut direct3d, &mut gallery.ui, &under);
            assert!(off(&got, &Surface::draw(&mut gallery.ui, 64), &under).0 <= TOLERANCE, "at a new size");
        }
        passed();
    }

    #[test]
    fn direct3d_9_says_what_went_wrong() {
        on_a_desktop_of_its_own("says_what_went_wrong_alone");
    }

    /// What goes wrong comes back as a value: an interface wider than the graphics card's
    /// textures.
    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn says_what_went_wrong_alone() {
        if !alone() {
            return;
        }
        let mut direct3d = Direct3D::new(true);
        let failed = direct3d.layer.update(&mut nothing(100_000, 2)).expect_err("no texture is that wide");
        println!("{failed}");
        assert_eq!((failed.operation, failed.code as u32), ("making the texture", 0x8876_086C));
        let mut gallery = Gallery::new(320, 200, 1.0);
        let under = under(320, 200);
        let got = drawn_whole(&mut direct3d, &mut gallery.ui, &under);
        assert!(off(&got, &Surface::draw(&mut gallery.ui, 64), &under).0 <= TOLERANCE, "it draws again after");
        passed();
    }

    #[test]
    fn the_direct3d_9_example_draws_into_its_window_at_any_size() {
        on_a_desktop_of_its_own("the_example_draws_into_its_window_at_any_size_alone");
    }

    #[test]
    #[ignore = "run by the test above, on a desktop of its own"]
    fn the_example_draws_into_its_window_at_any_size_alone() {
        if !alone() {
            return;
        }
        let window = super::direct3d_11::unseen_window();
        let mut device = Device::new(window, true);
        let mut layer = unsafe { Layer::new(device.device) }.unwrap();
        for (width, height) in [(320, 200), (480, 300)] {
            check(device.resize(width as u32, height as u32), "resizing");
            let mut gallery = Gallery::new(width, height, 1.0);
            device.begin(0xff33_6699);
            layer.draw(&mut gallery.ui).unwrap();
            let mut buffer = null_mut();
            // (Device 18: GetBackBuffer.)
            unsafe { check(function::<unsafe extern "system" fn(Com, u32, u32, u32, *mut Com) -> i32>(device.device, 18)(device.device, 0, 0, 0, &mut buffer), "finding the back buffer") };
            let got = read(device.device, buffer, width, height);
            unsafe { release(buffer) };
            let (most, pixels) = off(&got, layer.surface(), &vec![0xff33_6699; width * height]);
            println!("the example's own device, {width} by {height}: {pixels} pixels off, by {most} at most");
            assert!(most <= TOLERANCE);
            device.show();
        }
        drop((layer, device));
        super::direct3d_11::close(window);
        passed();
    }
}

#[allow(dead_code)]
#[path = "../examples/vulkan.rs"]
mod vulkan;

mod on_vulkan {
    use super::vulkan::{Device, Object, Target, VkBufferImageCopy, VkMemoryBarrier, barrier};
    use super::*;
    use std::ptr::null;
    use taut::layer_vulkan::Layer;

    /// The layer, on the first device the system has, with the example's device and targets.
    struct Vulkan {
        layer: Option<Layer>,
        /// The target whose render pass the layer was made for (any target's is like it).
        first: Option<Target>,
        device: Device,
    }

    impl Vulkan {
        /// Nothing, where the system has no Vulkan (no loader, or no device that draws).
        fn new() -> Option<Vulkan> {
            let device = Device::new()?;
            let first = Target::new(&device, 1, 1);
            let layer = unsafe { Layer::new(&device.handles(first.render_pass)) }.unwrap_or_else(|failed| panic!("{failed}"));
            Some(Vulkan { layer: Some(layer), first: Some(first), device })
        }

        fn layer(&mut self) -> &mut Layer {
            self.layer.as_mut().unwrap()
        }

        /// A buffer of the layer's size, the layer's image copied to it (`to_image` false) or
        /// from it, with `before` done to the buffer first and `after` after.
        fn through_buffer(&mut self, to_image: bool, before: &dyn Fn(*mut u32), after: &mut dyn FnMut(*mut u32)) {
            let (device, image) = (&self.device, self.layer.as_ref().unwrap().image());
            let surface = self.layer.as_ref().unwrap().surface();
            let (width, height) = (surface.width, surface.height);
            let (buffer, memory, there) = device.buffer(width * height * 4, 0x1 | 0x2);
            let (program, commands) = (&device.program, device.commands);
            let whole = VkBufferImageCopy { row_length: width as u32, layers: [0x1, 0, 0, 1], extent: [width as u32, height as u32, 1], ..Default::default() };
            // The image is laid out for shaders (5); for a copy to it (7) or from it (6), and back.
            let layout = if to_image { 7 } else { 6 };
            let (away, back) = (barrier(image, 5, layout, 0x20, 0x1000 | 0x800), barrier(image, layout, 5, 0x1000, 0x20));
            before(there);
            unsafe {
                device.begin();
                (program.vkCmdPipelineBarrier)(commands, 0x80, 0x1000, 0, 0, null(), 0, null(), 1, &away);
                match to_image {
                    true => (program.vkCmdCopyBufferToImage)(commands, buffer, image, 7, 1, &whole),
                    false => (program.vkCmdCopyImageToBuffer)(commands, image, 6, buffer, 1, &whole),
                }
                (program.vkCmdPipelineBarrier)(commands, 0x1000, 0x80, 0, 0, null(), 0, null(), 1, &back);
                // What was copied is made visible to the processor (transfer writes to host reads).
                let by_hand = VkMemoryBarrier { kind: 46, source_access: 0x1000, destination_access: 0x2000, ..Default::default() };
                (program.vkCmdPipelineBarrier)(commands, 0x1000, 0x4000, 0, 1, &by_hand, 0, null(), 0, null());
                device.submit();
            }
            after(there);
            unsafe {
                (program.vkDestroyBuffer)(device.device, buffer, null());
                (program.vkFreeMemory)(device.device, memory, null());
            }
        }
    }

    impl Renderer for Vulkan {
        fn update(&mut self, ui: &mut Ui) {
            unsafe { self.layer().update(ui) }.unwrap_or_else(|failed| panic!("{failed}"));
        }
        fn drawn(&mut self, ui: &mut Ui, under: &[u32], [width, height]: [usize; 2], area: [i32; 4]) -> Vec<u32> {
            let target = Target::new(&self.device, width, height);
            let layer = self.layer.as_mut().unwrap();
            let got = target.frame(&self.device, under, &mut |commands: Object| unsafe { layer.draw(ui, commands, area) }.unwrap_or_else(|failed| panic!("{failed}")));
            target.destroy(&self.device);
            got
        }
        fn fill(&mut self, pixels: &[u32]) {
            let pixels = pixels.to_vec();
            self.through_buffer(true, &|there| unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), there, pixels.len()) }, &mut |_| {});
        }
        fn texture(&mut self) -> Vec<u32> {
            let mut got = vec![0u32; self.layer.as_ref().unwrap().surface().pixels.len()];
            self.through_buffer(false, &|_| {}, &mut |there| unsafe { std::ptr::copy_nonoverlapping(there, got.as_mut_ptr(), got.len()) });
            got
        }
        fn surface(&self) -> &Surface {
            self.layer.as_ref().unwrap().surface()
        }
    }

    impl Drop for Vulkan {
        fn drop(&mut self) {
            self.device.wait();
            unsafe { self.layer.take().unwrap().delete() };
            self.first.take().unwrap().destroy(&self.device);
        }
    }

    #[test]
    fn vulkan_shows_the_surface_over_what_was_drawn() {
        let Some(mut vulkan) = Vulkan::new() else { return println!("no Vulkan on this system: nothing to check") };
        shows_the_surface_over_what_was_drawn(&mut vulkan, "Vulkan");
    }

    #[test]
    fn vulkan_is_given_only_the_places_that_changed() {
        let Some(mut vulkan) = Vulkan::new() else { return println!("no Vulkan on this system: nothing to check") };
        is_given_only_the_places_that_changed(&mut vulkan, "Vulkan");
    }

    #[test]
    fn vulkan_lies_at_the_programs_viewport() {
        let Some(mut vulkan) = Vulkan::new() else { return println!("no Vulkan on this system: nothing to check") };
        lies_at_the_programs_viewport(&mut vulkan, "Vulkan");
    }

    /// Frames drawn one after another, each changing the interface, with the layer's copies
    /// and the program's frames on one queue: every frame shows the interface as it was when
    /// that frame was drawn, never one copy behind or ahead. And a layer made again on the
    /// same device, as after a device was lost and made again, draws as the first did.
    #[test]
    fn vulkan_shows_each_frame_as_it_was_and_is_made_again() {
        let Some(mut vulkan) = Vulkan::new() else { return println!("no Vulkan on this system: nothing to check") };
        let mut gallery = Gallery::new(640, 400, 1.0);
        let under = under(640, 400);
        for step in 0..6 {
            gallery.ui.event(Event::Key(Key::Tab, Mods::NONE));
            gallery.run();
            let got = vulkan.drawn(&mut gallery.ui, &under, [640, 400], [0, 0, 640, 400]);
            let (most, _) = off(&got, &Surface::draw(&mut gallery.ui, 64), &under);
            assert!(most <= TOLERANCE, "step {step}: off by {most}");
        }
        vulkan.device.wait();
        unsafe { vulkan.layer.take().unwrap().delete() };
        vulkan.layer = Some(unsafe { Layer::new(&vulkan.device.handles(vulkan.first.as_ref().unwrap().render_pass)) }.unwrap());
        let got = drawn_whole(&mut vulkan, &mut gallery.ui, &under);
        let (most, _) = off(&got, &Surface::draw(&mut gallery.ui, 64), &under);
        assert!(most <= TOLERANCE, "made again: off by {most}");
    }
}
