//! The iOS window shown through Metal, with the feature `gpu` (`gpu_metal`): a `CAMetalLayer`
//! over the page, where the safe area is. The CPU draws each place that changed into the window's
//! `Surface` as without the feature, and that place goes up; with `gpu-draw` the GPU draws large
//! places itself. Either way the pixels shown are the CPU's, byte for byte (within a few levels
//! at edges where the GPU draws).
//!
//! - The device is opened on a thread of its own when the window first shows: the CPU shows the
//!   first frames.
//! - A new size or scale (the screen turned, Split View, another screen) gives the layer a new
//!   texture, drawn whole.
//! - In the background nothing goes to the GPU, as iOS asks: the last frame is waited for as the
//!   app leaves, and what changed meanwhile is drawn when it comes back.
//! - A device lost is let go of and opened again, drawn whole; after three, the CPU shows the
//!   window. The CPU shows it whole whenever the GPU stops.
use crate::geom::rect;
use crate::gpu_metal::{Metal, Way};
use crate::macos_sys::{NSRect, Obj};
use crate::platform::{Renderer, ShownBy};
use crate::surface::Surface;
use crate::ui::Ui;

/// What the window keeps of Metal: the GPU and the window's part on it, whether the app is in the
/// background, and what is to be done when the GPU shows again.
pub(crate) struct Screen {
    metal: Metal,
    way: Way,
    away: bool,
    /// The window is to be shown again whole, though nothing changed.
    again: bool,
    /// The GPU stopped showing the window: the CPU is to draw all of it.
    afresh: bool,
}

impl Screen {
    /// What a window asking for `renderer` keeps (as `TAUT_RENDERER` says, if set): nothing
    /// open yet.
    pub(crate) fn new(renderer: Renderer) -> Screen {
        Screen { metal: Metal::new(renderer), way: Way::Unasked, away: false, again: false, afresh: false }
    }

    /// What shows the window now.
    pub(crate) fn renderer(&self) -> Renderer {
        match (&self.metal.gpu, &self.way) {
            (Some(gpu), Way::Gpu(_)) => gpu.renderer(),
            _ => Renderer::Cpu,
        }
    }

    /// The app goes to the background (`away`): no GPU work from now on, the last frame waited
    /// for; or comes back: the window is shown again.
    pub(crate) fn set_away(&mut self, away: bool) {
        if let (true, Some(gpu), Way::Gpu(shows)) = (away, &self.metal.gpu, &self.way) {
            gpu.settle(shows);
        }
        (self.away, self.again) = (away, self.again || !away);
    }

    /// Draws what changed of `ui` and shows it through Metal, where Metal shows the window: a
    /// layer over `area` (in points) of `under` (the view's layer), at `scale`. Each place is
    /// drawn by the CPU into `surface` and put up, or with `gpu-draw` drawn by the GPU. Says how
    /// it was shown.
    pub(crate) fn draw(&mut self, ui: &mut Ui, surface: &mut Surface, under: Obj, area: NSRect, scale: f32) -> ShownBy {
        if self.away {
            return if matches!(self.way, Way::Gpu(_)) { ShownBy::Gpu } else { ShownBy::Cpu };
        }
        self.mend();
        let size = [(ui.size().w * scale).round() as usize, (ui.size().h * scale).round() as usize];
        self.ask(under, area, size, scale);
        let (Some(gpu), Way::Gpu(shows)) = (self.metal.gpu.as_mut(), &mut self.way) else { return self.by_cpu() };
        if shows.size() != (size, scale) && !gpu.fit(shows, area, size, scale) {
            self.unshow();
            self.way = Way::Cpu;
            return self.by_cpu();
        }
        let everything = rect(0.0, 0.0, ui.size().w, ui.size().h);
        let places = if shows.take_whole() { vec![everything] } else { ui.changes().collect() };
        places.iter().for_each(|place| gpu.draw_place(shows, surface, ui, *place, (size, scale)));
        ui.changed();
        if !places.is_empty() || std::mem::take(&mut self.again) {
            gpu.show(shows, false);
        }
        ShownBy::Gpu
    }

    /// Lets go of a lost device (the window's part first), and takes in one opened, or asks for
    /// one.
    fn mend(&mut self) {
        if self.metal.gpu.as_ref().is_some_and(|gpu| gpu.lost) {
            self.unshow();
            self.metal.let_go();
        }
        self.metal.keep_opening();
    }

    /// Shows the window through the GPU where it is not yet asked and there is one: a layer over
    /// `area` of `under`, `size` pixels at `scale`; else the CPU does.
    fn ask(&mut self, under: Obj, area: NSRect, size: [usize; 2], scale: f32) {
        let Some(gpu) = self.metal.gpu.as_mut().filter(|_| matches!(self.way, Way::Unasked)) else { return };
        self.way = gpu.window(under, area, size, scale).map_or(Way::Cpu, Way::Gpu);
    }

    /// Lets go of the window's part on the GPU: the CPU draws all of it next, and the GPU is
    /// asked again.
    fn unshow(&mut self) {
        if matches!(self.way, Way::Gpu(_)) {
            self.metal.unshow(&mut self.way);
            self.afresh = true;
        }
        self.way = Way::Unasked;
    }

    /// The CPU shows the window: all of it, if the GPU showed it till now.
    fn by_cpu(&mut self) -> ShownBy {
        if std::mem::take(&mut self.afresh) { ShownBy::CpuAfresh } else { ShownBy::Cpu }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.metal.unshow(&mut self.way);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Point, Size, pt};
    use crate::gpu_metal::tests::{one, read};
    use crate::macos_sys::{Pool, made};
    use crate::{Button, Color, Event, LocalClipboard, Mods, text::BuiltInFont};

    /// A window shown through a `Screen`: its view's layer is a plain layer.
    struct Bench {
        screen: Screen,
        ui: Ui,
        surface: Surface,
        under: Obj,
        app: Box<dyn FnMut(&mut Ui)>,
    }

    /// An interface with a count and a button, and a rounded box.
    fn counter() -> Box<dyn FnMut(&mut Ui)> {
        let mut count = 0;
        Box::new(move |ui: &mut Ui| {
            ui.column().pad(10.0).gap(8.0).open();
            ui.label(&format!("Pressed {count} times"));
            count += ui.button("Press").clicked as u32;
            ui.el().size(120.0, 40.0).fill(Color::rgb(0x3060c0)).radius(8.0).show();
            ui.close();
        })
    }

    impl Bench {
        /// `app`, `size` units at `scale`, shown as `renderer` asks.
        fn new(renderer: Renderer, size: Size, scale: f32, app: Box<dyn FnMut(&mut Ui)>) -> Bench {
            let ui = Ui::new(Box::new(BuiltInFont), Box::new(LocalClipboard::default()));
            let mut bench = Bench { screen: Screen::new(renderer), ui, surface: Surface::default(), under: made(c"CALayer"), app };
            bench.resize(size, scale);
            bench
        }

        fn resize(&mut self, size: Size, scale: f32) {
            self.ui.resize(size, scale);
            self.run();
        }

        fn run(&mut self) {
            let app = &mut self.app;
            self.ui.run(&mut |ui| app(ui));
        }

        fn draw(&mut self) -> ShownBy {
            let size = self.ui.size();
            let area = NSRect::new(0.0, 0.0, f64::from(size.w), f64::from(size.h));
            let scale = self.ui.scale();
            let shown = self.screen.draw(&mut self.ui, &mut self.surface, self.under, area, scale);
            if shown == ShownBy::CpuAfresh {
                self.surface = Surface::default();
            }
            if shown != ShownBy::Gpu {
                self.surface.update(&mut self.ui);
                self.ui.changed();
            }
            shown
        }

        /// Draws once Metal is open (the CPU draws until then), the CPU drawing every part (with
        /// `gpu-draw`, Metal draws them all for a path named).
        fn draw_open(&mut self) -> ShownBy {
            self.screen.metal.keep_opening();
            self.screen.metal.opened();
            #[cfg(feature = "gpu-draw")]
            if let Some(gpu) = self.screen.metal.gpu.as_mut() {
                gpu.drawing = crate::gpu_draw::Drawing::Never;
            }
            self.draw()
        }

        /// What Metal shows, once its last frame is done.
        fn shown(&mut self) -> Vec<u32> {
            let (Some(gpu), Way::Gpu(shows)) = (&self.screen.metal.gpu, &mut self.screen.way) else { panic!("Metal shows nothing") };
            let texture = shows.texture();
            read(gpu, shows, texture)
        }

        /// Checks a frame against the interface as the CPU draws it afresh: at most `tolerance`
        /// levels apart; says how many pixels differ.
        fn check(&mut self, frame: &[u32], tolerance: u32, what: &str) -> usize {
            let drawn = Surface::draw(&mut self.ui, 64);
            assert_eq!(frame.len(), drawn.pixels.len(), "{what}: the texture is the window's size");
            let off = |a: u32, b: u32| [16, 8, 0].map(|shift| (a >> shift & 255).abs_diff(b >> shift & 255)).into_iter().max().unwrap_or(0);
            let most = frame.iter().zip(&drawn.pixels).map(|(a, b)| off(*a, *b)).max().unwrap_or(0);
            let differ = frame.iter().zip(&drawn.pixels).filter(|(a, b)| off(**a, **b) > 0).count();
            eprintln!("{what}: {differ} pixels differ, at most by {most}");
            assert!(most <= tolerance, "{what}: {most} levels apart");
            differ
        }

        fn click(&mut self, at: Point) {
            self.ui.event(Event::Move(at));
            self.ui.event(Event::Press(Button::Left, Mods::NONE));
            self.ui.event(Event::Release(Button::Left));
            self.run();
        }

        fn lose(&mut self) {
            self.screen.metal.gpu.as_mut().expect("Metal is open").lose();
        }
    }

    #[test]
    fn metal_shows_the_cpus_pixels_byte_for_byte_and_what_changed_goes_up() {
        let (_pool, _one) = (Pool::new(), one());
        let mut bench = Bench::new(Renderer::Metal, Size { w: 300.0, h: 220.0 }, 3.0, counter());
        assert_eq!(bench.draw(), ShownBy::Cpu, "the CPU shows the first frame, while Metal opens");
        assert_eq!(bench.draw_open(), ShownBy::Gpu);
        assert_eq!(bench.screen.renderer(), Renderer::Metal);
        let frame = bench.shown();
        assert_eq!(bench.check(&frame, 0, "the first frame through Metal"), 0);
        bench.click(pt(30.0, 40.0));
        assert_eq!(bench.draw(), ShownBy::Gpu);
        let frame = bench.shown();
        assert_eq!(bench.check(&frame, 0, "after a click"), 0);
    }

    #[test]
    fn a_turned_screen_gets_a_new_texture_drawn_whole() {
        let (_pool, _one) = (Pool::new(), one());
        let mut bench = Bench::new(Renderer::Gpu, Size { w: 300.0, h: 220.0 }, 2.0, counter());
        bench.draw_open();
        bench.resize(Size { w: 220.0, h: 300.0 }, 3.0);
        assert_eq!(bench.draw(), ShownBy::Gpu);
        let frame = bench.shown();
        assert_eq!(bench.check(&frame, 0, "turned, at a new scale"), 0);
    }

    #[test]
    fn nothing_goes_to_the_gpu_in_the_background_and_the_window_comes_back_up_to_date() {
        let (_pool, _one) = (Pool::new(), one());
        let mut bench = Bench::new(Renderer::Metal, Size { w: 200.0, h: 160.0 }, 2.0, counter());
        bench.draw_open();
        let before = bench.shown();
        bench.screen.set_away(true);
        bench.click(pt(30.0, 40.0));
        assert_eq!(bench.draw(), ShownBy::Gpu);
        assert!(bench.shown() == before, "in the background the texture stays as it was");
        bench.screen.set_away(false);
        assert_eq!(bench.draw(), ShownBy::Gpu);
        let frame = bench.shown();
        assert!(frame != before);
        assert_eq!(bench.check(&frame, 0, "back from the background"), 0);
    }

    #[test]
    fn a_lost_device_is_opened_again_and_after_too_many_the_cpu_shows() {
        let (_pool, _one) = (Pool::new(), one());
        let mut bench = Bench::new(Renderer::Metal, Size { w: 200.0, h: 160.0 }, 1.0, counter());
        bench.draw_open();
        for lost in 1..=4 {
            bench.lose();
            bench.click(pt(30.0, 40.0));
            assert_eq!(bench.draw(), ShownBy::CpuAfresh, "lost {lost} times: the CPU shows the window whole");
            assert_eq!(bench.screen.renderer(), Renderer::Cpu);
            let shown = bench.draw_open();
            if lost <= 3 {
                assert_eq!(shown, ShownBy::Gpu, "a new device after {lost} lost");
                let frame = bench.shown();
                assert_eq!(bench.check(&frame, 0, &format!("a new device after {lost} lost")), 0);
            } else {
                assert_eq!(shown, ShownBy::Cpu, "one lost too many: the CPU shows the window for good");
            }
        }
    }

    #[test]
    fn the_cpu_asked_shows_the_window_itself() {
        let (_pool, _one) = (Pool::new(), one());
        let mut bench = Bench::new(Renderer::Cpu, Size { w: 100.0, h: 80.0 }, 1.0, counter());
        assert_eq!((bench.draw_open(), bench.screen.renderer()), (ShownBy::Cpu, Renderer::Cpu));
    }

    #[cfg(feature = "gpu-draw")]
    #[test]
    fn metal_draws_every_kind_of_part_within_a_few_levels_of_the_cpu() {
        let (_pool, _one) = (Pool::new(), one());
        let mut pictures = None;
        let app = Box::new(move |ui: &mut Ui| crate::gpu_draw::tests::every_kind(ui, &mut pictures));
        let mut bench = Bench::new(Renderer::Metal, Size { w: 360.0, h: 210.0 }, 3.0, app);
        bench.draw_open();
        bench.screen.metal.gpu.as_mut().expect("Metal").drawing = crate::gpu_draw::Drawing::Always;
        // A new scale: all of it drawn again, by the GPU.
        bench.resize(Size { w: 360.0, h: 210.0 }, 2.0);
        assert_eq!(bench.draw(), ShownBy::Gpu);
        assert!(bench.screen.metal.gpu.as_ref().is_some_and(crate::gpu_metal::Gpu::has_drawn), "Metal drew the parts itself");
        let frame = bench.shown();
        bench.check(&frame, crate::gpu_draw::tests::TOLERANCE, "drawn by Metal");
    }
}
