//! Video and sound files played by the system's own player: Media Foundation on Windows,
//! GStreamer on Linux, AVFoundation on macOS and iOS, the NDK's media codecs on Android, a
//! `<video>` element on the web; each loaded at run time. Feature `video`.
//!
//! A [`Video`] is the app's: it opens paused. Each run, [`Ui::video`] takes the newest frame and
//! shows it in a box; only that box is redrawn per frame. [`Ui::video_controls`] adds play,
//! position, time, mute and volume, usable by keyboard and screen readers.
//!
//! ```no_run
//! use taut::{Video, Window};
//!
//! let mut video = Video::open("clip.mp4").expect("a player");
//! video.play();
//! taut::run(Window::default(), move |ui| {
//!     ui.column().pad(8.0).gap(8.0).open();
//!     ui.video(&mut video).grow().show();
//!     ui.video_controls(&mut video);
//!     ui.close();
//! });
//! ```

use crate::accessibility::Role;
use crate::color::Color;
use crate::el::El;
use crate::event::{Key, Mods};
use crate::layout::{Align, Length};
use crate::ui::{Icon, Picture, Response, Ui};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[cfg(target_os = "android")]
use crate::video_android::Player;
#[cfg(target_vendor = "apple")]
use crate::video_apple::Player;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
use crate::video_gstreamer::Player;
#[cfg(target_family = "wasm")]
use crate::video_web::Player;
#[cfg(windows)]
use crate::video_windows::Player;

/// Where a video comes from: a file, an address (`https://`, `file://`), or the file's bytes.
/// Text with `://` in it is an address, any other a path. On the web a path is an address
/// relative to the page, or a picked file's (`/picked/<name>`, see [`web::picked`](crate::web)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoSource {
    /// A file on disk.
    File(PathBuf),
    /// An address the system's player reads: `https://`, `file://`, and what else it knows.
    Url(String),
    /// A whole file in memory.
    Bytes(Vec<u8>),
}

impl From<&str> for VideoSource {
    fn from(text: &str) -> Self {
        match text.contains("://") {
            true => VideoSource::Url(text.to_owned()),
            false => VideoSource::File(text.into()),
        }
    }
}
impl From<&Path> for VideoSource {
    fn from(path: &Path) -> Self {
        VideoSource::File(path.to_owned())
    }
}
impl From<PathBuf> for VideoSource {
    fn from(path: PathBuf) -> Self {
        VideoSource::File(path)
    }
}
impl From<Vec<u8>> for VideoSource {
    fn from(bytes: Vec<u8>) -> Self {
        VideoSource::Bytes(bytes)
    }
}

/// Why a video did not open.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum VideoError {
    /// The system has no player taut can use: GStreamer missing on Linux, Media Foundation on
    /// Windows N editions, no page under Node. Its words.
    NoPlayer(String),
    /// The player refused the source: no such file, bytes it can't read. Its words.
    Refused(String),
}

impl std::fmt::Display for VideoError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            VideoError::NoPlayer(why) => write!(f, "no video player: {why}"),
            VideoError::Refused(why) => write!(f, "the video player refused it: {why}"),
        }
    }
}

impl std::error::Error for VideoError {}

/// What the system's player said since it was last asked.
#[derive(Default)]
pub(crate) struct News {
    /// It played to the end.
    pub ended: bool,
    /// It stopped with this error.
    pub error: Option<String>,
    /// The error clears at the next play: a browser waiting for a press.
    pub clears_on_play: bool,
}

/// Numbers each video, for its box's key.
static OPENED: AtomicU64 = AtomicU64::new(0);

/// A video or sound file played by the system's player, with its newest frame. Opens paused,
/// at full volume. Dropping it stops it; [`Ui::drop_video`] also frees its last frame.
pub struct Video {
    player: Player,
    picture: Option<Picture>,
    serial: u64,
    playing: bool,
    looping: bool,
    muted: bool,
    volume: f32,
    fill: bool,
    ended: bool,
    error: Option<String>,
    /// The error clears at the next play.
    clears_on_play: bool,
    position: Duration,
    length: Option<Duration>,
    size: Option<(usize, usize)>,
    /// The interface's time when the player was last asked: once per moment, so a rerun shows
    /// the same.
    asked: Option<Duration>,
    /// A frame is due though paused: after opening or seeking.
    awaited: bool,
    /// The time between frames, as seen; and the last frame's time.
    gap: Duration,
    last: Option<Duration>,
    /// Frames shown so far.
    frames: u64,
    /// The box's size at the last layout, for filling.
    room: (f32, f32),
}

impl std::fmt::Debug for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Video")
            .field("playing", &self.playing)
            .field("position", &self.position)
            .field("length", &self.length)
            .field("size", &self.size)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl Video {
    /// Opens a video or sound file, address or bytes, with sound: `Video::open("clip.mp4")`.
    /// Its size and length come once the player has read it. An error only where the system
    /// has no player or refuses at once; later errors are [`error`](Self::error).
    pub fn open(source: impl Into<VideoSource>) -> Result<Video, VideoError> {
        Video::opened(source.into(), true)
    }
    /// The same with no sound: no sound device is opened (Windows: it plays muted).
    pub fn open_silent(source: impl Into<VideoSource>) -> Result<Video, VideoError> {
        Video::opened(source.into(), false)
    }
    fn opened(source: VideoSource, sound: bool) -> Result<Video, VideoError> {
        #[cfg(not(target_family = "wasm"))]
        if let VideoSource::File(path) = &source {
            std::fs::metadata(path).map_err(|why| VideoError::Refused(format!("{}: {why}", path.display())))?;
        }
        let player = Player::open(source, sound)?;
        let serial = OPENED.fetch_add(1, Ordering::Relaxed);
        Ok(Video {
            player,
            picture: None,
            serial,
            playing: false,
            looping: false,
            muted: !sound,
            volume: 1.0,
            fill: false,
            ended: false,
            error: None,
            clears_on_play: false,
            position: Duration::ZERO,
            length: None,
            size: None,
            asked: None,
            awaited: true,
            gap: Duration::from_millis(16),
            last: None,
            frames: 0,
            room: (0.0, 0.0),
        })
    }

    /// Plays from where it is; from the start after the end. (A browser plays sound only after
    /// a press on the page: before, this gives [`error`](Self::error); played again from a
    /// press, it plays.)
    pub fn play(&mut self) {
        if std::mem::take(&mut self.clears_on_play) {
            self.error = None;
        }
        if self.ended || self.length.is_some_and(|length| self.position >= length) {
            self.player.seek(Duration::ZERO);
        }
        self.ended = false;
        self.playing = true;
        self.player.play();
    }
    /// Pauses.
    pub fn pause(&mut self) {
        self.playing = false;
        self.player.pause();
    }
    /// Whether it is playing.
    pub fn playing(&self) -> bool {
        self.playing
    }
    /// Goes to this time; paused, its frame shows there.
    pub fn seek(&mut self, to: Duration) {
        let to = self.length.map_or(to, |length| to.min(length));
        self.player.seek(to);
        (self.position, self.awaited, self.last) = (to, true, None);
    }
    /// Where it is, as of the last run.
    pub fn position(&self) -> Duration {
        self.position
    }
    /// How long it is, once known (never for a live stream).
    pub fn length(&self) -> Option<Duration> {
        self.length
    }
    /// Its frames' size in pixels, once the first has come.
    pub fn size(&self) -> Option<(usize, usize)> {
        self.size
    }
    /// Sets the volume, from 0 to 1.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = if volume.is_nan() { 0.0 } else { volume.clamp(0.0, 1.0) };
        self.player.set_volume(self.volume);
    }
    /// The volume, from 0 to 1.
    pub fn volume(&self) -> f32 {
        self.volume
    }
    /// Mutes or unmutes it.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.player.set_muted(muted);
    }
    /// Whether it is muted.
    pub fn muted(&self) -> bool {
        self.muted
    }
    /// Plays it again from the start at each end, or stops there.
    pub fn set_looping(&mut self, looping: bool) {
        self.looping = looping;
    }
    /// Whether it loops.
    pub fn looping(&self) -> bool {
        self.looping
    }
    /// Shows its frames filling their box, cut to its shape; or whole, with bars (the default).
    pub fn set_fill(&mut self, fill: bool) {
        (self.fill, self.last) = (fill, None);
    }
    /// True once each time it reached its end (never while looping).
    pub fn ended(&mut self) -> bool {
        std::mem::take(&mut self.ended)
    }
    /// Why it stopped, if the player gave up: a format it can't play, a lost connection.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    /// How many frames it has shown.
    pub fn frames_shown(&self) -> u64 {
        self.frames
    }

    /// Asks the player what changed and takes its newest frame, once per moment of the
    /// interface (a rerun shows the same); asks to run again when the next is due.
    fn update(&mut self, ui: &mut Ui) {
        if self.asked != Some(ui.now) {
            self.ask(ui);
        }
        let waits = self.error.is_none() && (self.playing || self.awaited);
        if waits && !ui.window_minimized() {
            ui.wake_after(if self.playing { self.gap } else { Duration::from_millis(10) });
        }
    }
    fn ask(&mut self, ui: &mut Ui) {
        self.asked = Some(ui.now);
        let news = self.player.news();
        if let Some(error) = news.error {
            (self.error, self.playing, self.clears_on_play) = (Some(error), false, news.clears_on_play);
        }
        if news.ended {
            self.restart_or_stop();
        }
        let mut pixels = Vec::new();
        if let Some((width, at)) = self.player.frame(&mut pixels) {
            self.show(ui, width, pixels, at);
        }
        (self.position, self.length) = (self.player.position(), self.player.length().or(self.length));
    }
    /// At the end: again from the start, or stopped there.
    fn restart_or_stop(&mut self) {
        if !self.looping {
            (self.playing, self.ended) = (false, true);
            return;
        }
        self.player.seek(Duration::ZERO);
        self.player.play();
    }
    /// Shows a new frame of this width, taken at `at`: cut to its box's shape when filling.
    fn show(&mut self, ui: &mut Ui, width: usize, pixels: Vec<u32>, at: Duration) {
        if let Some(gap) = self.last.and_then(|last| at.checked_sub(last)).filter(|gap| !gap.is_zero()) {
            self.gap = gap.clamp(Duration::from_millis(4), Duration::from_millis(100));
        }
        (self.last, self.awaited, self.frames) = (Some(at), false, self.frames + 1);
        self.size = Some((width, pixels.len() / width.max(1)));
        let (width, pixels) = match self.fill {
            true => cut_to(width, pixels, self.room),
            false => (width, pixels),
        };
        match self.picture {
            Some(picture) => ui.set_picture(picture, width, pixels),
            None => self.picture = Some(ui.add_picture(width, pixels)),
        }
    }
}

/// The middle of a frame with the shape of a box `room` wide and high: all of it where the box
/// has no size yet.
fn cut_to(width: usize, pixels: Vec<u32>, (wide, high): (f32, f32)) -> (usize, Vec<u32>) {
    let height = pixels.len() / width.max(1);
    if wide <= 0.0 || high <= 0.0 || height == 0 {
        return (width, pixels);
    }
    let shape = wide / high;
    let (cut_w, cut_h) = match (width as f32 / height as f32) > shape {
        true => (((height as f32 * shape).round() as usize).clamp(1, width), height),
        false => (width, ((width as f32 / shape).round() as usize).clamp(1, height)),
    };
    let (left, top) = ((width - cut_w) / 2, (height - cut_h) / 2);
    let rows = pixels.chunks_exact(width).skip(top).take(cut_h);
    (cut_w, rows.flat_map(|row| &row[left..left + cut_w]).copied().collect())
}

/// A file kind's usual ending, told by its first bytes: for players that pick a reader by name.
#[cfg(any(windows, target_vendor = "apple", target_family = "wasm"))]
pub(crate) fn kind_of(bytes: &[u8]) -> &'static str {
    match bytes {
        [_, _, _, _, b'f', b't', b'y', b'p', b'q', b't', ..] => "mov",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'A', b'V', b'I', ..] => "avi",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'A', b'V', b'E', ..] => "wav",
        [0x1A, 0x45, 0xDF, 0xA3, ..] => "mkv",
        [b'O', b'g', b'g', b'S', ..] => "ogg",
        [b'f', b'L', b'a', b'C', ..] => "flac",
        [b'I', b'D', b'3', ..] | [0xFF, 0xE0..=0xFF, ..] => "mp3",
        [0x30, 0x26, 0xB2, 0x75, ..] => "wmv",
        _ => "mp4",
    }
}

/// A triangle pointing right.
const PLAY: Icon = Icon::solid("M4.5 2.5v11l9-5.5z");
/// Two bars.
const PAUSE: Icon = Icon::solid("M4 2.5h3v11H4zM9 2.5h3v11H9z");
/// A speaker.
const SOUND: Icon = Icon::line("M2 6h2.5L8 3v10L4.5 10H2zM11 5.5a3.5 3.5 0 0 1 0 5");

/// Videos: a box showing one, and the usual controls.
impl Ui {
    /// A box showing the video's newest frame, whole with black bars (or filling it, see
    /// [`Video::set_fill`]), at its pixel size unless sized after: `ui.video(&mut video).grow()
    /// .show()`. Only this box is redrawn per frame.
    pub fn video(&mut self, video: &mut Video) -> El<'_> {
        let key = ("taut video", video.serial);
        if video.fill {
            let room = self.visible_rect(self.id(key));
            video.room = (room.w, room.h);
        }
        video.update(self);
        let (picture, size, fill) = (video.picture, video.size.unwrap_or((0, 0)), video.fill);
        let el = self.el().key(key).fill(Color::BLACK);
        let el = if fill { el.sense() } else { el };
        match picture {
            Some(picture) => el.picture(picture).keep_aspect(),
            None => el.size(size.0 as f32, size.1 as f32),
        }
    }
    /// Play or pause, the position, the time, mute and volume, in a row; `changed` when any
    /// was used. Tab reaches each; arrows move the sliders.
    pub fn video_controls(&mut self, video: &mut Video) -> Response {
        let mut row = self.row().w(Length::Grow(1.0)).gap(8.0).align(Align::Start, Align::Center).open();
        let (icon, hint) = if video.playing { (PAUSE, "Pause") } else { (PLAY, "Play") };
        let toggled = self.icon_button(icon, hint).clicked;
        match (toggled, video.playing) {
            (true, true) => video.pause(),
            (true, false) => video.play(),
            _ => {}
        }
        let length = video.length.unwrap_or_default().as_secs_f32();
        let mut at = video.position.as_secs_f32().min(length);
        let moved = self.named_slider(&mut at, length, "Position", Length::Grow(1.0));
        if moved {
            video.seek(Duration::from_secs_f32(at.max(0.0)));
        }
        self.note(&format!("{} / {}", clock(video.position), clock(video.length.unwrap_or_default())));
        let mut muted = video.muted;
        let muting = self.icon_toggle(&mut muted, SOUND, "Mute").changed;
        if muting {
            video.set_muted(muted);
        }
        let mut volume = video.volume;
        let loud = self.named_slider(&mut volume, 1.0, "Volume", Length::Px(80.0));
        if loud {
            video.set_volume(volume);
        }
        self.close();
        row.changed = toggled || moved || muting || loud;
        self.again |= row.changed;
        row
    }
    /// Drops a video: it stops, and its last frame is freed.
    pub fn drop_video(&mut self, video: Video) {
        if let Some(picture) = video.picture {
            self.drop_picture(picture);
        }
    }

    /// A slider from 0 to `most`, told to readers by `name`: arrows move a twentieth, Home and
    /// End to the ends. Says whether it moved.
    fn named_slider(&mut self, value: &mut f32, most: f32, name: &str, width: Length) -> bool {
        let (t, knob, was) = (self.theme, 14.0, *value);
        let state = self.interact(self.next_id());
        if let (true, Some(p)) = (state.pressed, self.pointer()) {
            *value = most * ((p.x - state.rect.x - knob / 2.0) / (state.rect.w - knob).max(1.0)).clamp(0.0, 1.0);
        }
        if let Some((_, to)) = self.reader_sets.take_if(|(of, _)| *of == state.id) {
            *value = to;
        }
        if state.focused {
            *value += self.slider_keys() * most;
        }
        *value = value.clamp(0.0, most.max(0.0));
        let along = if most > 0.0 { *value / most } else { 0.0 };
        self.row().w(width).h(Length::Px(t.control_height)).radius(4.0).sense().focusable().open();
        self.tell_value_named(&state, Role::Slider, [*value, 0.0, most], name);
        self.tell_step(most / 20.0);
        self.el().w(Length::Grow(along)).h(Length::Px(4.0)).radius(2.0).fill(t.accent).show();
        self.el().size(knob, knob).radius(knob / 2.0).fill(if state.hovered || state.pressed { t.text } else { t.text_dim }).show();
        self.el().w(Length::Grow(1.0 - along)).h(Length::Px(4.0)).radius(2.0).fill(t.line_strong).show();
        self.close();
        *value != was
    }
    /// How far keys move a focused slider, as a share of it.
    fn slider_keys(&mut self) -> f32 {
        let keys = [(Key::Right, 0.05), (Key::Up, 0.05), (Key::Left, -0.05), (Key::Down, -0.05), (Key::Home, -2.0), (Key::End, 2.0)];
        keys.into_iter().filter(|(key, _)| self.key(*key, Mods::NONE)).map(|(_, by)| by).sum()
    }
}

/// A time as "1:05", or "1:02:03" from an hour.
fn clock(time: Duration) -> String {
    let all = time.as_secs();
    match all >= 3600 {
        true => format!("{}:{:02}:{:02}", all / 3600, all / 60 % 60, all % 60),
        false => format!("{}:{:02}", all / 60, all % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (Each system uses some of them.)
    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::*;

    /// Frames 0 to 29 at 10 a second: a lossless H.264 MP4 where the system plays H.264; on
    /// Linux an MJPEG AVI where GStreamer reads those, else Theora in Ogg made by `ffmpeg`.
    /// `sound` adds a silent track.
    pub(super) fn test_file(sound: bool) -> Option<PathBuf> {
        let name = if sound { "with-sound" } else { "silent" };
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        if !crate::video_gstreamer::has_element(c"avidemux") || !crate::video_gstreamer::has_element(c"jpegdec") {
            return theora(name, sound);
        }
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        return Some(written(&format!("{name}.avi"), &avi(30, 10, sound)));
        #[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
        Some(written(&format!("{name}.mp4"), &mp4(30, 10, sound)))
    }

    /// The same frames as Theora in Ogg (every frame a key frame), by `ffmpeg`.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn theora(name: &str, sound: bool) -> Option<PathBuf> {
        let raw: Vec<u8> = (0..30).flat_map(frame_pixels).flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8]).collect();
        let frames = written(&format!("{name}.rgb"), &raw);
        let out = frames.with_extension("ogv");
        let mut ffmpeg = std::process::Command::new("ffmpeg");
        ffmpeg.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "64x48", "-r", "10", "-i"]).arg(&frames);
        if sound {
            ffmpeg.args(["-f", "lavfi", "-i", "anullsrc=r=8000:cl=mono", "-t", "3", "-c:a", "libvorbis"]);
        }
        let made = ffmpeg.args(["-c:v", "libtheora", "-q:v", "10", "-g", "1"]).arg(&out).status();
        let made = made.is_ok_and(|status| status.success());
        skipped(made, "GStreamer here reads no MJPEG in AVI, and no ffmpeg makes Theora").then_some(out)
    }

    /// Whether to go on: `false`, said, where `there` is not (a system or CI image without a
    /// player), unless `TAUT_VIDEO=required`, which fails then.
    fn skipped(there: bool, missing: &str) -> bool {
        let required = std::env::var("TAUT_VIDEO").as_deref() == Ok("required");
        assert!(there || !required, "{missing}");
        if !there {
            eprintln!("{missing}: skipped");
        }
        there
    }
    /// The video made, or none where the system has no player.
    fn player(made: Result<Video, VideoError>) -> Option<Video> {
        if let Err(VideoError::NoPlayer(why)) = &made {
            skipped(false, why);
            return None;
        }
        Some(made.unwrap())
    }
    /// The test file opened without sound: none where there is no player or no file.
    fn opened(sound: bool) -> Option<Video> {
        test_file(sound).and_then(|file| player(Video::open_silent(file)))
    }

    /// A video in a trial window, its frame at the top left at its size; with the controls
    /// under it where `controls`.
    fn trial(video: Video, controls: bool) -> Trial<Video> {
        Trial::new(video, move |video, ui| {
            ui.column().open();
            ui.video(video).show();
            if controls {
                ui.video_controls(video);
            }
            ui.close();
        })
    }
    use crate::testing::Trial;

    /// Lets `time` pass for real, then in the trial: it runs whenever the video asked.
    fn pass(trial: &mut Trial<Video>, time: Duration) {
        std::thread::sleep(time);
        trial.wait(time);
    }
    /// Passes time until `done`, or fails after 10 s saying `what`.
    fn until(trial: &mut Trial<Video>, what: &str, done: impl Fn(&Video) -> bool) {
        // (Run first: the test changed the video from outside, as an app's timer would.)
        trial.run();
        let start = std::time::Instant::now();
        while !done(&trial.app) {
            assert!(start.elapsed() < Duration::from_secs(10), "{what} did not happen: {:?}", trial.app);
            pass(trial, Duration::from_millis(20));
        }
    }
    /// The frame number on the trial's screen.
    fn shown(trial: &Trial<Video>) -> usize {
        number_in(&trial.screen.pixels, trial.screen.width, (0, 0), 1.0)
    }
    /// The frame number the video's position says, at 10 a second.
    fn due(video: &Video) -> f32 {
        video.position().as_secs_f32() * 10.0
    }

    /// Video tests one at a time: their players' threads and timing stay their own.
    fn alone() -> std::sync::MutexGuard<'static, ()> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        ONE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn a_frame_shows_its_number() {
        for n in [0, 1, 37, 255] {
            assert_eq!(number_in(&frame_pixels(n), 64, (0, 0), 1.0), n);
        }
    }

    /// Runs this test again in a child, with `TAUT_VIDEO_WITHOUT` set, through `wrap` (a
    /// program and its arguments before it, if any). Whether the child passed; none where the
    /// wrapper could not run at all.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn again_in_a_child(envs: &[(&str, &str)], wrap: &[&str]) -> Option<bool> {
        let me = std::env::current_exe().unwrap();
        let me = me.to_str().unwrap();
        let mut words: Vec<&str> = wrap.to_vec();
        words.extend([me, "--exact", "video::tests::a_system_without_gstreamer_is_told_so", "--nocapture"]);
        let run = std::process::Command::new(words[0]).args(&words[1..]).envs(envs.iter().copied()).env("TAUT_VIDEO_WITHOUT", "1").output().ok()?;
        let said = String::from_utf8_lossy(&run.stdout).into_owned() + &String::from_utf8_lossy(&run.stderr);
        let started = said.contains("running 1 test");
        eprintln!("{}", said.lines().filter(|line| line.contains("test ") || line.contains("panicked") || line.contains("NoPlayer")).collect::<Vec<_>>().join("\n"));
        started.then(|| run.status.success() && said.contains("1 passed"))
    }

    #[test]
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn a_system_without_gstreamer_is_told_so() {
        if std::env::var_os("TAUT_VIDEO_WITHOUT").is_some() {
            let file = written("without.avi", &avi(2, 10, false));
            let made = Video::open_silent(file);
            eprintln!("{made:?}");
            assert!(matches!(made, Err(VideoError::NoPlayer(_))), "{made:?}");
            return;
        }
        // GStreamer without its plugins: no playbin.
        let empty = std::env::temp_dir().join(format!("taut-video-no-plugins-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        let registry = empty.join("registry.bin");
        let envs = [("GST_PLUGIN_SYSTEM_PATH_1_0", empty.to_str().unwrap()), ("GST_PLUGIN_PATH_1_0", ""), ("GST_REGISTRY_1_0", registry.to_str().unwrap())];
        assert_eq!(again_in_a_child(&envs, &[]), Some(true), "without plugins");
        // No GStreamer at all: its library hidden in a mount namespace of the child's own.
        let library =
            ["/usr/lib/x86_64-linux-gnu/libgstreamer-1.0.so.0", "/usr/lib/libgstreamer-1.0.so.0", "/usr/lib64/libgstreamer-1.0.so.0"].into_iter().find(|path| std::path::Path::new(path).exists());
        let Some(library) = library else { return };
        let hide = format!("mount --bind /dev/null {library} && exec \"$0\" \"$@\"");
        match again_in_a_child(&[], &["unshare", "-rm", "sh", "-c", &hide]) {
            Some(passed) => assert!(passed, "without GStreamer's library"),
            None => eprintln!("unshare could not hide GStreamer here: that half skipped"),
        }
    }

    #[test]
    fn a_missing_file_is_refused() {
        let missing = std::env::temp_dir().join("taut-no-such-video.mp4");
        assert!(matches!(Video::open_silent(missing.as_path()), Err(VideoError::Refused(_))));
    }

    #[test]
    fn text_is_a_path_or_an_address() {
        assert_eq!(VideoSource::from("clip.mp4"), VideoSource::File("clip.mp4".into()));
        assert_eq!(VideoSource::from("https://example.com/a.mp4"), VideoSource::Url("https://example.com/a.mp4".into()));
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn frames_show_at_their_times_and_only_their_box_is_redrawn() {
        let _alone = alone();
        let Some(video) = opened(true) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        assert_eq!((trial.app.size(), shown(&trial)), (Some((64, 48)), 0));
        assert!(trial.app.length().is_some_and(|length| (length.as_secs_f32() - 3.0).abs() < 0.05), "{:?}", trial.app.length());
        trial.app.play();
        trial.run();
        let mut seen = Vec::new();
        while trial.app.playing() && seen.len() < 100 {
            std::thread::sleep(Duration::from_millis(50));
            let now = trial.ui.now + Duration::from_millis(50);
            trial.ui.set_time(now);
            let changed = trial.run();
            let number = shown(&trial);
            // (Its time span, 0.1 s, holds the position, give or take what showing it takes.)
            let (start, at) = (number as f32 / 10.0, trial.app.position().as_secs_f32());
            assert!(at > start - 0.08 && at < start + 0.18, "frame {number} shown at {at} s");
            assert!(changed.is_empty() || changed.x >= 0.0 && changed.right() <= 64.0 && changed.bottom() <= 48.0, "redrawn: {changed:?}");
            seen.push(number);
        }
        assert!(seen.windows(2).all(|two| two[0] <= two[1]) && seen.last() >= Some(&25), "frames shown: {seen:?}");
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn a_paused_video_stays_on_its_frame_and_a_seek_shows_the_frame_there() {
        let _alone = alone();
        let Some(video) = opened(false) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.play();
        until(&mut trial, "a second played", |video| video.position() >= Duration::from_secs(1));
        trial.app.pause();
        pass(&mut trial, Duration::from_millis(200));
        let (held, at) = (shown(&trial), trial.app.position());
        pass(&mut trial, Duration::from_millis(400));
        assert_eq!((shown(&trial), trial.app.position()), (held, at));
        assert!((held as f32 - due(&trial.app)).abs() <= 1.5, "paused at {at:?} on frame {held}");
        for (to, frame) in [(2.0, 20), (0.5, 5), (2.9, 29)] {
            trial.app.seek(Duration::from_secs_f32(to));
            until(&mut trial, "the frame after a seek", |video| !video.awaited);
            pass(&mut trial, Duration::from_millis(100));
            assert_eq!(shown(&trial), frame, "after a seek to {to} s");
        }
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_end_is_told_once_and_a_loop_starts_again() {
        let _alone = alone();
        let Some(video) = opened(true) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.seek(Duration::from_millis(2500));
        trial.app.play();
        until(&mut trial, "the end", |video| !video.playing());
        assert!(trial.app.ended() && !trial.app.ended(), "the end is told once");
        assert_eq!(shown(&trial), 29);
        trial.app.set_looping(true);
        trial.app.play();
        until(&mut trial, "playing from the start", |video| video.position() < Duration::from_millis(500) && video.frames > 1);
        pass(&mut trial, Duration::from_millis(300));
        assert!(trial.app.playing() && !trial.app.ended() && shown(&trial) < 10, "looped: {:?}", trial.app);
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_controls_are_told_to_readers_and_work_by_keyboard() {
        let _alone = alone();
        let Some(video) = opened(false) else { return };
        let mut trial = trial(video, true);
        until(&mut trial, "the first frame", |video| video.size().is_some() && video.length().is_some());
        trial.run();
        for (role, name) in [(Role::Button, "Play"), (Role::Slider, "Position"), (Role::Slider, "Volume")] {
            assert!(trial.has(role, name), "{name}:\n{}", trial.dump());
        }
        let position = trial.find(Role::Slider, "Position").value_range;
        assert_eq!(position, Some([0.0, 0.0, 3.0]));
        trial.press(Role::Button, "Play");
        assert!(trial.app.playing() && trial.has(Role::Button, "Pause"));
        trial.press(Role::Button, "Pause");
        assert!(!trial.app.playing());
        // Tab from the button to the position, End: the last frame.
        trial.key("Tab");
        trial.key("End");
        until(&mut trial, "the frame after End", |video| !video.awaited);
        pass(&mut trial, Duration::from_millis(100));
        assert_eq!(shown(&trial), 29);
        trial.key("Tab");
        trial.key("Space");
        assert!(!trial.app.muted(), "Space on Mute unmutes what opened silent");
        trial.key("Tab");
        trial.key("Left");
        assert!((trial.app.volume() - 0.95).abs() < 1e-4, "volume {}", trial.app.volume());
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn bytes_play_as_the_file_does() {
        let _alone = alone();
        let Some(bytes) = test_file(false).map(|file| std::fs::read(file).unwrap()) else { return };
        let Some(video) = player(Video::open_silent(bytes)) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.seek(Duration::from_millis(1200));
        until(&mut trial, "the frame after a seek", |video| !video.awaited);
        pass(&mut trial, Duration::from_millis(100));
        assert_eq!(shown(&trial), 12);
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn filling_cuts_the_frame_to_its_box() {
        let _alone = alone();
        let Some(mut video) = opened(false) else { return };
        video.set_fill(true);
        let mut trial = Trial::new(video, |video, ui| {
            ui.video(video).size(32.0, 48.0).show();
        });
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.seek(Duration::from_millis(500));
        until(&mut trial, "a frame cut to its box", |video| !video.awaited);
        let picture = trial.app.picture.unwrap();
        assert_eq!(trial.ui.picture_size(picture), Some((32, 48)));
    }

    /// Opens, plays a moment and drops a video, `times` over.
    #[cfg(not(target_vendor = "apple"))]
    fn open_and_close(times: usize) {
        for _ in 0..times {
            let Some(video) = opened(true) else { return };
            let mut trial = trial(video, false);
            until(&mut trial, "the first frame", |video| video.size().is_some());
            trial.app.play();
            pass(&mut trial, Duration::from_millis(100));
            let Trial { app, mut ui, .. } = trial;
            ui.drop_video(app);
        }
    }

    /// The memory this process holds, in bytes.
    #[cfg(not(target_vendor = "apple"))]
    fn memory_held() -> usize {
        #[cfg(windows)]
        {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetCurrentProcess() -> isize;
                fn K32GetProcessMemoryInfo(process: isize, counters: *mut [usize; 10], size: u32) -> i32;
            }
            let mut counters = [0usize; 10];
            counters[0] = std::mem::size_of_val(&counters);
            // SAFETY: the counters are as large as said; this process's handle needs no closing.
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, std::mem::size_of_val(&counters) as u32) };
            counters[8]
        }
        #[cfg(not(windows))]
        {
            let status = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
            status.split(' ').nth(1).and_then(|pages| pages.parse::<usize>().ok()).unwrap_or(0) * 4096
        }
    }

    #[test]
    #[cfg(not(target_vendor = "apple"))]
    fn many_opens_leave_nothing_behind() {
        // (In a process of its own, so no other test's memory is counted.)
        if std::env::var_os("TAUT_VIDEO_LEAK").is_none() {
            let me = std::env::current_exe().unwrap();
            let run =
                std::process::Command::new(me).args(["--exact", "video::tests::many_opens_leave_nothing_behind", "--test-threads=1", "--nocapture"]).env("TAUT_VIDEO_LEAK", "1").output().unwrap();
            let said = String::from_utf8_lossy(&run.stdout).into_owned() + &String::from_utf8_lossy(&run.stderr);
            assert!(run.status.success() && said.contains("1 passed"), "{said}");
            eprintln!("{}", said.lines().filter(|line| line.starts_with("held")).collect::<Vec<_>>().join("\n"));
            return;
        }
        open_and_close(5);
        let (before, threads) = (memory_held(), threads_held());
        open_and_close(40);
        let after = memory_held();
        eprintln!("held {before} bytes, after 40 more {after}");
        assert!(after < before + (6 << 20), "held {before} bytes, after 40 more {after}");
        no_more_threads(threads);
    }

    /// The names of the threads this process runs.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn threads_held() -> Vec<String> {
        let tasks = std::fs::read_dir("/proc/self/task").into_iter().flatten().flatten();
        tasks.filter_map(|task| std::fs::read_to_string(task.path().join("comm")).ok()).map(|name| name.trim().to_owned()).collect()
    }
    #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
    fn threads_held() -> Vec<String> {
        Vec::new()
    }

    /// Waits up to 2 s for the threads to be no more than `before` (the system's own wind
    /// down after a player is dropped), else fails naming them. A few more may stay: a player's
    /// pool may keep idle threads a while (GStreamer's, under load); one per open would be 40.
    #[cfg(not(target_vendor = "apple"))]
    fn no_more_threads(before: Vec<String>) {
        let start = std::time::Instant::now();
        while threads_held().len() > before.len() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let now = threads_held();
        eprintln!("held {} threads, after 40 more opens {}", before.len(), now.len());
        assert!(now.len() <= before.len() + 4, "threads before: {before:?}\nafter: {now:?}");
    }
}
