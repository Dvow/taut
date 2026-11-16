//! taut under a program's own renderer: the Direct3D 11 and OpenGL layers, and the examples
//! using them and Vulkan, checked against the surface they show. A gallery frame, rendered over
//! a known image, is read back and compared with the surface composited over that image by
//! hand; a change updates the texture where it changed and nowhere else; the interface lies at
//! the program's viewport and inside it; the state a layer sets for drawing is restored as the
//! program had it; and errors come back as values, in the system's own words.
//!
//! Nothing is drawn on screen. Direct3D 11 renders on Windows' software rasterizer (WARP);
//! OpenGL through the example's own WGL on a never-shown window, and on Linux through EGL with
//! no display (Mesa's llvmpipe). A test needing an X display says so, and runs on its own:
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
        assert_eq!((failed.doing, failed.code as u32), ("making the vertex shader", 0x8007_0057));
        assert!(!failed.said.is_empty() && failed.to_string().contains(&failed.said), "Windows' words are said");
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
            for (way, said) in ["UpdateSubresource", "staging + copy", "dynamic, whole"].iter().enumerate() {
                let [[small_called, small_done], [whole_called, whole_done]] = best[way];
                println!("{name}, {said}: small {small_called:.0} us to call, {small_done:.0} until done; whole {whole_called:.0} to call, {whole_done:.0} until done");
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
        let said = |what| unsafe { CStr::from_ptr((checks.glGetString)(what)).to_string_lossy().into_owned() };
        format!("{}, {}", said(0x1F01), said(0x1F02))
    }

    /// The layer, in a context of this thread's.
    struct OpenGl<Keep> {
        checks: Checks,
        layer: Layer,
        /// What the context needs kept for as long as it is used.
        _kept: Keep,
    }

    impl<Keep> OpenGl<Keep> {
        fn new((find, kept): (Find, Keep)) -> OpenGl<Keep> {
            let checks = Checks::load(&mut |name| find(name));
            println!("OpenGL: {}", renderer(&checks));
            let layer = unsafe { Layer::new(&mut |name| find(name)) }.unwrap_or_else(|failed| panic!("{failed}"));
            OpenGl { checks, layer, _kept: kept }
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
        let (find, _kept) = context();
        let failed = unsafe { Layer::new(&mut |name| if name == c"glBlendFuncSeparate" { std::ptr::null() } else { find(name) }) }.expect_err("no layer without a function");
        assert_eq!(failed.to_string(), "finding OpenGL's functions failed: this OpenGL has no glBlendFuncSeparate");
        let mut layer = unsafe { Layer::new(&mut |name| find(name)) }.unwrap();
        let checks = Checks::load(&mut |name| find(name));
        let mut most = 0;
        unsafe { (checks.glGetIntegerv)(0x0D33, &mut most) };
        let failed = unsafe { layer.draw(&mut nothing(most as usize + 1, 2)) }.expect_err("an interface wider than OpenGL's textures is not drawn");
        println!("{failed}");
        assert_eq!(failed.doing, "making the texture");
        assert!(failed.said.contains(&most.to_string()), "what OpenGL can make is said");
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
        heard.retain(|event| !matches!(event, Event::Focus(_) | Event::Move(_)));
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

#[allow(dead_code)]
#[path = "../examples/vulkan.rs"]
mod vulkan;

mod on_vulkan {
    use super::vulkan::{Device, Layer, Target};
    use super::*;

    /// The example's layer and its own device and target, on the first device the system has.
    struct Vulkan {
        device: Device,
        target: Option<Target>,
        layer: Option<Layer>,
        surface: Surface,
    }

    impl Vulkan {
        /// Nothing, where the system has no Vulkan (no loader, or no device that draws).
        fn new() -> Option<Vulkan> {
            let device = Device::new()?;
            let target = Target::new(&device, 1, 1);
            let layer = Layer::new(&device.vk, &device.gpu, target.render_pass, 1, 1);
            Some(Vulkan { device, target: Some(target), layer: Some(layer), surface: Surface::default() })
        }

        /// The places written to the layer's buffer are copied to its image, and nothing else:
        /// each copy also spoils the buffer outside the places, before it is recorded, so that
        /// had more been copied, the image would show it.
        fn copy(&mut self, places: &[[usize; 4]], pixels: &[u32]) {
            let (layer, width) = (self.layer.as_mut().unwrap(), self.surface.width);
            let given = Surface { width, height: self.surface.height, pixels: pixels.to_vec() };
            places.iter().for_each(|place| layer.update(&given, *place));
            let outside = |at: &usize| !places.iter().any(|[left, top, right, bottom]| (*left..*right).contains(&(at % width)) && (*top..*bottom).contains(&(at / width)));
            (0..pixels.len()).filter(outside).for_each(|at| unsafe { *layer.written.add(at) = 0x5a5a_5a5a });
            self.device.begin();
            layer.record(&self.device.vk, self.device.commands);
            self.device.submit();
        }
    }

    impl Renderer for Vulkan {
        fn update(&mut self, ui: &mut Ui) {
            let size = (self.surface.width, self.surface.height);
            let places = self.surface.update(ui);
            if (self.surface.width, self.surface.height) != size {
                let (width, height) = (self.surface.width, self.surface.height);
                self.target.take().unwrap().destroy(&self.device);
                self.target = Some(Target::new(&self.device, width, height));
                self.layer.as_mut().unwrap().resize(&self.device.vk, &self.device.gpu, width, height);
            }
            let pixels = self.surface.pixels.clone();
            self.copy(&places, &pixels);
        }
        fn drawn(&mut self, ui: &mut Ui, under: &[u32], size: [usize; 2], viewport: [i32; 4]) -> Vec<u32> {
            self.update(ui);
            assert_eq!(([self.surface.width, self.surface.height], viewport), (size, [0, 0, size[0] as i32, size[1] as i32]), "the Vulkan example draws over all of its target");
            self.target.as_ref().unwrap().frame(&self.device, self.layer.as_mut().unwrap(), under)
        }
        fn fill(&mut self, pixels: &[u32]) {
            self.copy(&[[0, 0, self.surface.width, self.surface.height]], pixels);
        }
        fn texture(&mut self) -> Vec<u32> {
            // Over nothing at all, each pixel is what the image holds.
            let nothing = vec![0; self.surface.pixels.len()];
            self.target.as_ref().unwrap().frame(&self.device, self.layer.as_mut().unwrap(), &nothing)
        }
        fn surface(&self) -> &Surface {
            &self.surface
        }
    }

    impl Drop for Vulkan {
        fn drop(&mut self) {
            self.layer.take().unwrap().destroy(&self.device.vk, self.device.gpu.device);
            self.target.take().unwrap().destroy(&self.device);
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
}
