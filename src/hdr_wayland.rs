//! HDR on Wayland (the feature `hdr`, see `hdr`): each screen's brightness from the
//! compositor's colour management (`wp_color_manager_v1`), and windows lit on an HDR screen
//! handed over in PQ.
//!
//! - A screen: its output's image description (`wp_color_management_output_v1`): SDR white is
//!   its reference luminance, its peak the most it targets. Asked again whenever the compositor
//!   says it changed.
//! - A window lit on an HDR screen is described as PQ with sRGB primaries
//!   (`wp_image_description_creator_params_v1`) and handed over in buffers of 16 bits a channel
//!   (`wl_shm` ABGR16161616). The CPU still draws its pixels, into memory of the window's; what
//!   changed is put into a free buffer in PQ: SDR white at the protocol's reference, 203 nits,
//!   times each colour's gain (`hdr`), at most the screen's headroom. The compositor maps it
//!   to the screen.
//! - Without the protocol, its parametric descriptions, PQ, sRGB primaries or that buffer
//!   format, windows stay SDR, as with an SDR screen.
use super::{Host, Link, Sheet, on_pixels};
use crate::canvas::Canvas;
use crate::geom::{Rect, rect};
use crate::hdr::{Gains, Screen};
use crate::ui::Ui;
use crate::wayland_wire::{Arg, Interface, Message, Shared};
use std::os::fd::AsRawFd;

/// DRM's ABGR16161616: red, green, blue and alpha, 16 bits each, in that order in memory.
const DEEP: u32 = u32::from_le_bytes(*b"AB48");
/// The protocol's numbers: the parametric feature, the PQ transfer function, the sRGB
/// primaries, the perceptual rendering intent.
const PARAMETRIC: u32 = 1;
const PQ: u32 = 11;
const SRGB: u32 = 1;
const PERCEPTUAL: u32 = 0;
/// SDR white in PQ, as the protocol has it: in nits.
const REFERENCE: f32 = 203.0;

/// What the compositor's colour management offers, and the screens it describes.
#[derive(Debug, Default)]
pub(super) struct Colors {
    /// It takes parametric descriptions, PQ, the sRGB primaries, and deep buffers.
    parametric: bool,
    pq: bool,
    srgb: bool,
    deep: bool,
    /// The PQ description windows are handed over in, once asked (0: not yet), and whether the
    /// compositor made it.
    described: u32,
    ready: bool,
    outputs: Vec<Output>,
}

/// An output's colour: its object, its colour object, the description and information asked
/// for it (0: none), its reference and target luminances so far, and its screen.
#[derive(Debug)]
struct Output {
    output: u32,
    colors: u32,
    description: u32,
    info: u32,
    said: [f32; 2],
    screen: Screen,
}

impl Colors {
    /// Whether windows can be handed over in PQ.
    fn can(&self) -> bool {
        self.parametric && self.pq && self.srgb && self.deep
    }

    /// The screen of a window on `on` (outputs, in the order it entered them): the first one
    /// described; before it is on any, the first output's.
    fn screen_of(&self, on: &[u32]) -> Screen {
        #[cfg(test)]
        if let Some(faked) = tests::FAKED.get() {
            return faked;
        }
        let entered = on.iter().find_map(|output| self.outputs.iter().find(|known| known.output == *output));
        entered.or(self.outputs.first()).map_or(Screen::SDR, |output| output.screen)
    }

    /// Asks an output's description again.
    fn ask(&mut self, link: &Link, at: usize) {
        let output = &mut self.outputs[at];
        output.description = link.wire.make(Interface::Description);
        link.wire.send(output.colors, 1, &[Arg::Uint(output.description)]);
    }
}

/// A window's HDR: its gains; its surface's colour object (0: none) and whether it is handed
/// over in PQ; the PQ code of each SDR byte; its pixels as the CPU draws them; and its deep
/// buffers.
#[derive(Default)]
pub(super) struct Lit {
    pub(super) gains: Gains,
    colors: u32,
    pub(super) on: bool,
    table: Vec<u16>,
    headroom: f32,
    frame: Vec<u32>,
    buffers: Vec<Deep>,
}

/// A deep `wl_shm` buffer: as `Buffer`, its lacking places in pixels (left, top, right,
/// bottom).
struct Deep {
    id: u32,
    pool: u32,
    memory: Shared,
    size: [i32; 2],
    busy: bool,
    missing: Vec<[i32; 4]>,
}

impl Deep {
    /// A deep buffer `size` pixels, all of it lacking.
    fn new(link: &Link, size: [i32; 2]) -> Option<Deep> {
        let len = size[0] as usize * size[1] as usize * 8;
        let memory = Shared::new(len)?;
        let (wire, pool, id) = (&link.wire, link.wire.make(Interface::Pool), link.wire.make(Interface::Buffer));
        wire.send(link.has.shm, 0, &[Arg::Uint(pool), Arg::File(memory.file.as_raw_fd()), Arg::Int(len as i32)]);
        wire.send(pool, 0, &[Arg::Uint(id), Arg::Int(0), Arg::Int(size[0]), Arg::Int(size[1]), Arg::Int(size[0] * 8), Arg::Uint(DEEP)]);
        Some(Deep { id, pool, memory, size, busy: false, missing: vec![[0, 0, size[0], size[1]]] })
    }

    /// Destroys it and frees its memory.
    fn destroy(self, link: &Link) {
        link.wire.send(self.id, 0, &[]);
        link.wire.send(self.pool, 1, &[]);
    }

    /// Its samples: red, green, blue, alpha for each pixel.
    fn samples(&mut self) -> &mut [u16] {
        let pixels = self.memory.pixels();
        // SAFETY: the same memory, as half as large numbers, aligned for them.
        unsafe { std::slice::from_raw_parts_mut(pixels.as_mut_ptr().cast::<u16>(), pixels.len() * 2) }
    }
}

/// A PQ code (0 to 65,535) for linear light, 1.0 being SDR white.
fn pq_code(light: f32) -> u16 {
    let [m1, m2, c1, c2, c3] = [0.159_301_76, 78.843_75, 0.835_937_5, 18.851_563, 18.6875];
    let raised = (light.max(0.0) * REFERENCE / 10_000.0).min(1.0).powf(m1);
    (((c1 + c2 * raised) / (1.0 + c3 * raised)).powf(m2) * 65_535.0 + 0.5) as u16
}

/// Gains and offset of a pixel nothing lights.
const UNLIT: [f32; 4] = [1.0, 1.0, 1.0, 0.0];

/// sRGB's curve undone: a byte's linear value.
fn linear(byte: u32) -> f32 {
    let part = byte as f32 / 255.0;
    if part <= 0.04045 { part / 12.92 } else { ((part + 0.055) / 1.055).powf(2.4) }
}

impl Lit {
    /// Puts `place` (left, top, right, bottom) of the window's pixels into `deep` in PQ, each
    /// lit by its gains, at most the headroom.
    fn put(&self, deep: &mut Deep, [left, top, right, bottom]: [i32; 4]) {
        // (Gains of another size are of the window before a resize: none until they are taken.)
        let (wide, fits) = (deep.size[0] as usize, self.gains.size == deep.size && !self.gains.plane.is_empty());
        let samples = deep.samples();
        for y in top as usize..bottom as usize {
            let row = &self.frame[y * wide..][..wide];
            let gains = fits.then(|| &self.gains.plane[y * wide..][..wide]);
            let out = &mut samples[y * wide * 4..][..wide * 4];
            (left as usize..right as usize).for_each(|x| self.put_one(row[x], gains.map_or(UNLIT, |gains| gains[x].map(crate::hdr::from_half)), &mut out[x * 4..x * 4 + 4]));
        }
    }

    /// One pixel (`0xAARRGGBB`, premultiplied) in PQ, lit by `gains` (red, green, blue, and
    /// a gain map's offset; `hdr`).
    #[inline]
    fn put_one(&self, pixel: u32, [red_gain, green_gain, blue_gain, offset]: [f32; 4], out: &mut [u16]) {
        let [alpha, red, green, blue] = pixel.to_be_bytes();
        let code = |byte: u8, gain: f32| {
            let gain = gain.min(self.headroom);
            if gain == 1.0 && offset == 0.0 { self.table[byte as usize] } else { pq_code(((linear(byte as u32) + offset) * gain - offset).max(0.0)) }
        };
        out.copy_from_slice(&[code(red, red_gain), code(green, green_gain), code(blue, blue_gain), alpha as u16 * 257]);
    }

    /// The compositor released buffer `id`. Says whether it was one of these: one of another
    /// size than `size` goes.
    pub(super) fn released(&mut self, link: &Link, id: u32, size: [i32; 2]) -> bool {
        let Some(at) = self.buffers.iter().position(|deep| deep.id == id) else { return false };
        self.buffers[at].busy = false;
        if self.buffers[at].size != size {
            self.buffers.remove(at).destroy(link);
        }
        true
    }

    /// Lets go of its buffers and colour object.
    pub(super) fn destroy(mut self, link: &Link) {
        std::mem::take(&mut self.buffers).into_iter().for_each(|deep| deep.destroy(link));
        let colors = std::mem::take(&mut self.colors);
        if colors != 0 {
            link.wire.send(colors, 0, &[]);
        }
    }

    /// A buffer to put into: one not held, of the window's size, or a new one if there are
    /// fewer than two. Nothing where the window must wait.
    fn free(&mut self, link: &Link, size: [i32; 2]) -> Option<usize> {
        if let Some(at) = self.buffers.iter().position(|deep| !deep.busy && deep.size == size) {
            return Some(at);
        }
        if self.buffers.len() >= 2 {
            return None;
        }
        self.buffers.push(Deep::new(link, size)?);
        Some(self.buffers.len() - 1)
    }
}

impl Sheet {
    /// Draws what changed where the window is handed over in PQ: painted into its pixels, put
    /// into a free deep buffer with what that lacks, handed over. Says whether it committed;
    /// nothing where it is SDR.
    pub(super) fn draw_lit(&mut self, link: &Link, scale: f32, paint: &mut dyn FnMut(&mut Canvas)) -> Option<bool> {
        if !self.lit.on {
            return None;
        }
        let (size, units) = (self.pixels, rect(0.0, 0.0, self.units[0] as f32, self.units[1] as f32));
        if self.lit.frame.len() != size[0] as usize * size[1] as usize {
            (self.lit.frame, self.changed) = (vec![0; size[0] as usize * size[1] as usize], vec![units]);
        }
        let Some(at) = self.lit.free(link, size) else { return Some(self.pay(link)) };
        let areas: Vec<Rect> = std::mem::take(&mut self.changed).into_iter().map(|area| area.intersect(units)).filter(|area| !area.is_empty()).collect();
        for area in &areas {
            let [_, top, _, bottom] = on_pixels(*area, scale, size);
            let mut canvas = Canvas::new(&mut self.lit.frame[(top * size[0]) as usize..(bottom * size[0]) as usize], size[0] as usize, (bottom - top) as usize, (0, top), scale);
            canvas.clip(area.snap_to_pixels(scale));
            paint(&mut canvas);
        }
        let changed: Vec<[i32; 4]> = areas.iter().map(|area| on_pixels(*area, scale, size)).collect();
        let mut deep = self.lit.buffers.remove(at);
        let mut places = std::mem::take(&mut deep.missing);
        places.extend_from_slice(&changed);
        places.into_iter().for_each(|place| self.lit.put(&mut deep, place));
        let wire = &link.wire;
        wire.send(self.surface, 1, &[Arg::Object(deep.id), Arg::Int(0), Arg::Int(0)]);
        for [left, top, right, bottom] in &changed {
            wire.send(self.surface, 9, &[Arg::Int(*left), Arg::Int(*top), Arg::Int(right - left), Arg::Int(bottom - top)]);
        }
        deep.busy = true;
        self.lit.buffers.iter_mut().for_each(|other| other.missing.extend_from_slice(&changed));
        self.lit.buffers.insert(at, deep);
        self.waiting = wire.make(Interface::Callback);
        wire.send(self.surface, 3, &[Arg::Uint(self.waiting)]);
        wire.send(self.surface, 6, &[]);
        (self.hurry, self.owed) = (false, false);
        Some(true)
    }

    /// Hands the window over in PQ (`described`) from its next commit, or as SDR again; all of
    /// it is drawn anew.
    fn go_lit(&mut self, link: &Link, colors: &Colors, on: bool) {
        if on == self.lit.on {
            return;
        }
        let all = rect(0.0, 0.0, self.units[0] as f32, self.units[1] as f32);
        if on && self.lit.colors == 0 {
            self.lit.colors = link.wire.make(Interface::ColorSurface);
            link.wire.send(link.has.colors, 2, &[Arg::Uint(self.lit.colors), Arg::Object(self.surface)]);
        }
        match on {
            true => link.wire.send(self.lit.colors, 1, &[Arg::Object(colors.described), Arg::Uint(PERCEPTUAL)]),
            false => link.wire.send(self.lit.colors, 2, &[]),
        }
        if !on {
            std::mem::take(&mut self.lit.buffers).into_iter().for_each(|deep| deep.destroy(link));
            self.lit.frame = Vec::new();
            self.buffers.iter_mut().for_each(|buffer| buffer.missing = vec![all]);
        }
        (self.lit.on, self.changed) = (on, vec![all]);
    }
}

impl<App: FnMut(&mut Ui)> Host<App> {
    /// Takes a message of colour management: what the compositor takes (and the buffer formats
    /// `wl_shm` takes), an output's description changed, made or failed, what it says, and the
    /// windows' PQ description made or failed.
    pub(super) fn colored(&mut self, mut message: Message) {
        let (link, colors) = (self.link.clone(), &mut self.colors);
        let output = colors.outputs.iter().position(|output| [output.colors, output.description, output.info].contains(&message.object));
        let windows = message.object == colors.described;
        match (message.of, message.opcode, output) {
            (Interface::Shm, 0, _) => colors.deep |= message.uint() == DEEP,
            (Interface::ColorManager, 1, _) => colors.parametric |= message.uint() == PARAMETRIC,
            (Interface::ColorManager, 2, _) => colors.pq |= message.uint() == PQ,
            (Interface::ColorManager, 3, _) => colors.srgb |= message.uint() == SRGB,
            (Interface::Description, 0 | 1, None) if windows => (colors.ready, self.rerun) = (message.opcode == 1, true),
            (Interface::ColorOutput, 0, Some(at)) => colors.ask(&link, at),
            (Interface::Description, 0, Some(at)) => colors.outputs[at].screen = Screen::SDR,
            (Interface::Description, 1, Some(at)) => {
                let output = &mut colors.outputs[at];
                output.info = link.wire.make(Interface::DescriptionInfo);
                link.wire.send(output.description, 1, &[Arg::Uint(output.info)]);
                link.wire.send(output.description, 0, &[]);
            }
            (Interface::DescriptionInfo, 6, Some(at)) => colors.outputs[at].said[0] = [message.uint(), message.uint(), message.uint()][2] as f32,
            (Interface::DescriptionInfo, 8, Some(at)) => colors.outputs[at].said[1] = [message.uint(), message.uint()][1] as f32,
            (Interface::DescriptionInfo, 0, Some(at)) => self.rerun |= colors.outputs[at].take_said(),
            _ => {}
        }
    }

    /// Gets each output's colour object and asks its description, where not yet; and the
    /// windows' PQ description, where the compositor takes it.
    fn watch_colors(&mut self) {
        let (link, colors) = (self.link.clone(), &mut self.colors);
        if link.has.colors == 0 {
            return;
        }
        colors.outputs.retain(|output| self.outputs.iter().any(|(id, ..)| *id == output.output));
        for (output, ..) in &self.outputs {
            if colors.outputs.iter().any(|known| known.output == *output) {
                continue;
            }
            let made = link.wire.make(Interface::ColorOutput);
            link.wire.send(link.has.colors, 1, &[Arg::Uint(made), Arg::Object(*output)]);
            colors.outputs.push(Output { output: *output, colors: made, description: 0, info: 0, said: [0.0; 2], screen: Screen::SDR });
            colors.ask(&link, colors.outputs.len() - 1);
        }
        if colors.can() && colors.described == 0 {
            let creator = link.wire.make(Interface::DescriptionCreator);
            link.wire.send(link.has.colors, 5, &[Arg::Uint(creator)]);
            link.wire.send(creator, 1, &[Arg::Uint(PQ)]);
            link.wire.send(creator, 3, &[Arg::Uint(SRGB)]);
            colors.described = link.wire.make(Interface::Description);
            link.wire.send(creator, 0, &[Arg::Uint(colors.described)]);
        }
    }

    /// After a run, takes in what lights window `at` on its screen: its gains drawn again where
    /// they changed, and that part to be drawn; the window handed over in PQ while lit on an
    /// HDR screen. A new headroom is the app's to hear: it runs again.
    pub(super) fn light(&mut self, at: usize) {
        self.watch_colors();
        let (link, shown) = (self.link.clone(), &mut self.windows[at]);
        let screen = self.colors.screen_of(&shown.on);
        let (lit, scale, size) = (&mut shown.sheet.lit, shown.scale, shown.sheet.pixels);
        let taken = super::within(&mut self.ui, shown.known, |ui| {
            let news = ui.set_screen(screen);
            let glows = if screen.hdr && !shown.sheet.clear { ui.glows() } else { Vec::new() };
            (news, lit.gains.take(glows, size, &ui.light.maps))
        });
        let Some((news, part)) = taken else { return };
        lit.gains.changed = None;
        (lit.headroom, self.rerun) = (screen.headroom(), self.rerun | news);
        if let Some([left, top, right, bottom]) = part {
            shown.sheet.changed.push(rect(left as f32 / scale, top as f32 / scale, (right - left) as f32 / scale, (bottom - top) as f32 / scale));
        }
        let colors = &self.colors;
        let on = screen.hdr && shown.sheet.lit.gains.any() && colors.can() && colors.ready;
        if on && shown.sheet.lit.table.is_empty() {
            shown.sheet.lit.table = (0..256).map(|byte| pq_code(linear(byte))).collect();
        }
        shown.sheet.go_lit(&link, colors, on);
    }
}

impl Output {
    /// Takes in what its description said, at its end. Says whether its screen changed.
    fn take_said(&mut self) -> bool {
        let [white, peak] = self.said;
        let screen = match white > 0.0 {
            true => Screen { hdr: peak > white, white, peak: peak.max(white) },
            false => Screen::SDR,
        };
        (self.info, self.said) = (0, [0.0; 2]);
        std::mem::replace(&mut self.screen, screen) != screen
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        /// The screen every window is on, as a test has it.
        pub(crate) static FAKED: Cell<Option<Screen>> = const { Cell::new(None) };
    }

    #[test]
    fn sdr_white_is_handed_over_at_the_protocols_reference_in_pq() {
        // 203 nits is 0.580_69 of PQ's range; 10,000 nits all of it.
        assert!(pq_code(1.0).abs_diff((0.580_69 * 65_535.0) as u16) <= 2, "{}", pq_code(1.0));
        assert_eq!(pq_code(10_000.0 / REFERENCE), 65_535);
        assert_eq!(pq_code(0.0), 0);
        assert!(pq_code(2.0) > pq_code(1.0) && pq_code(1.0) > pq_code(linear(0x80)));
        let lit = Lit { table: (0..256).map(|byte| pq_code(linear(byte))).collect(), headroom: 5.0, ..Lit::default() };
        let mut out = [0u16; 4];
        lit.put_one(0xffff_ffff, UNLIT, &mut out);
        assert_eq!(out, [pq_code(1.0), pq_code(1.0), pq_code(1.0), 65_535]);
        lit.put_one(0xff80_ff00, [2.0, 2.0, 2.0, 0.0], &mut out);
        assert_eq!(out, [pq_code(linear(0x80) * 2.0), pq_code(2.0), 0, 65_535]);
        // Past the headroom, capped.
        lit.put_one(0xffff_ffff, [8.0, 8.0, 8.0, 0.0], &mut out);
        assert_eq!(out[0], pq_code(5.0));
        // Each colour its own gain; a gain map's offset before and after.
        lit.put_one(0xffff_ffff, [2.0, 3.0, 0.5, 0.0], &mut out);
        assert_eq!(out[..3], [pq_code(2.0), pq_code(3.0), pq_code(0.5)]);
        lit.put_one(0xff80_8080, [2.0, 2.0, 2.0, 0.25], &mut out);
        assert_eq!(out[0], pq_code((linear(0x80) + 0.25) * 2.0 - 0.25));
    }

    /// Whether a lit window is handed over as it is now: in PQ, a deep buffer holding all of
    /// it, nothing changed since.
    fn drawn_lit(host: &super::super::tests::Windows) -> bool {
        let sheet = &host.windows[0].sheet;
        sheet.lit.on && sheet.lit.buffers.iter().any(|deep| deep.missing.is_empty()) && sheet.changed.is_empty()
    }

    #[test]
    #[ignore]
    fn a_window_lit_on_an_hdr_screen_is_handed_over_in_pq() {
        use crate::{Color, Flow, Window};
        // The test bed's compositors have no HDR screen: the window's is said to be one.
        FAKED.set(Some(Screen { hdr: true, white: 200.0, peak: 1000.0 }));
        let heard = std::rc::Rc::new(Cell::new(0.0));
        let told = heard.clone();
        let window = Window { title: "HDR", width: 200.0, height: 100.0, ..Default::default() };
        let shown = super::super::tests::shown(window, false, move |ui| {
            ui.el().flow(Flow::Stack).size(200.0, 100.0).fill(Color::rgb(0x808080)).open();
            ui.el().offset(10.0, 10.0).size(40.0, 20.0).fill(Color::WHITE).show();
            ui.el().offset(60.0, 10.0).size(40.0, 20.0).fill(Color::WHITE).brightness(2.0).show();
            ui.el().offset(110.0, 10.0).size(40.0, 20.0).fill(Color::WHITE).brightness(8.0).show();
            ui.close();
            told.set(ui.headroom());
        });
        let Some((_alone, mut host)) = shown else { return eprintln!("no compositor: not tested") };
        let colors = &host.colors;
        println!("{}: parametric {}, PQ {}, sRGB {}, deep buffers {}", std::env::var("TAUT_DESK").unwrap_or_default(), colors.parametric, colors.pq, colors.srgb, colors.deep);
        if !colors.can() {
            return eprintln!("the compositor takes no PQ in deep buffers: not tested");
        }
        use super::super::tests::waits;
        assert!(waits(&mut host, drawn_lit), "the window is handed over in PQ");
        assert_eq!(heard.get(), 5.0, "the app hears the headroom");
        let (scale, sheet) = (host.windows[0].scale, &mut host.windows[0].sheet);
        let deep = sheet.lit.buffers.iter_mut().find(|deep| deep.missing.is_empty()).expect("a whole buffer");
        let wide = deep.size[0] as usize;
        let samples = deep.samples().to_vec();
        let at = |x: f32, y: f32| &samples[((y * scale) as usize * wide + (x * scale) as usize) * 4..][..4];
        let grey = pq_code(linear(0x80));
        let wanted = [((5.0, 5.0), grey), ((30.0, 20.0), pq_code(1.0)), ((80.0, 20.0), pq_code(2.0)), ((130.0, 20.0), pq_code(5.0))];
        println!("grey {:?}, white {:?}, twice {:?}, eight times {:?}", at(5.0, 5.0), at(30.0, 20.0), at(80.0, 20.0), at(130.0, 20.0));
        for ((x, y), code) in wanted {
            assert_eq!(at(x, y), [code, code, code, 65_535], "at {x}, {y}");
        }
        // Where the compositor shows a test its screen (Weston's capture), the window's grey is
        // grey there and its white white: it took the PQ as meant, for its SDR screen.
        std::thread::sleep(std::time::Duration::from_millis(300));
        if let Some((_, pixels)) = crate::compositors::tests::Desk::new().and_then(|mut desk| desk.picture()) {
            let near = |wanted: u32| pixels.iter().filter(|pixel| [16, 8, 0].iter().all(|shift| (*pixel >> shift & 0xff).abs_diff(wanted) <= 3)).count();
            let (grey, white) = (near(0x80), near(0xff));
            println!("on the screen: {grey} grey pixels, {white} white");
            assert!(grey >= 15_000 && white >= 2_000, "the compositor shows the window's grey and white: {grey}, {white}");
        }
        // The screen turns SDR: the window is handed over as ever, the CPU's pixels.
        FAKED.set(Some(Screen::SDR));
        host.rerun = true;
        let sdr = |host: &super::super::tests::Windows| {
            !host.windows[0].sheet.lit.on && host.windows[0].sheet.buffers.iter().any(|buffer| buffer.missing.is_empty()) && host.windows[0].sheet.changed.is_empty()
        };
        let back = heard.clone();
        assert!(waits(&mut host, move |host| sdr(host) && back.get() == 1.0), "the window is handed over as SDR again, its app hearing so: {}", heard.get());
        let fresh = crate::surface::Surface::draw(&mut host.ui, 64).pixels;
        let sheet = &mut host.windows[0].sheet;
        let held = sheet.buffers.iter_mut().find(|buffer| buffer.missing.is_empty()).unwrap();
        assert!(held.memory.pixels().to_vec() == fresh, "the CPU's pixels");
        FAKED.set(None);
    }
}
