//! Video and sound files played by the system's own player: Media Foundation on Windows,
//! GStreamer on Linux, AVFoundation on macOS and iOS, the NDK's media codecs on Android, a
//! `<video>` element on the web; each loaded at run time. Feature `video`.
//!
//! A [`Video`] is the app's: it opens paused. Each run, [`Ui::video`] takes the newest frame and
//! shows it in a box; only that box is redrawn per frame. [`Ui::video_captions`] shows its
//! caption over it, and [`Ui::video_controls`] adds play, position, time, captions, sound track,
//! speed, mute and volume, usable by keyboard and screen readers.
//!
//! ```no_run
//! use taut::{Flow, Video, Window};
//!
//! let mut video = Video::open("clip.mp4").expect("a player");
//! video.play();
//! taut::run(Window::default(), move |ui| {
//!     ui.column().pad(8.0).gap(8.0).open();
//!     ui.el().flow(Flow::Stack).grow().open();
//!     ui.video(&mut video).grow().show();
//!     ui.video_captions(&mut video);
//!     ui.close();
//!     ui.video_controls(&mut video);
//!     ui.close();
//! });
//! ```

use crate::accessibility::Role;
use crate::color::Color;
use crate::el::El;
use crate::event::{Key, Mods};
use crate::layout::{Align, Flow, Length};
use crate::text::Font;
use crate::ui::{Icon, Picture, Response, Ui};
pub use crate::video_captions::CaptionsError;
use crate::video_captions::{Caption, Cue, Down, LINE};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[cfg(all(feature = "gpu-draw", target_os = "android"))]
pub(crate) use crate::video_android::FrameTexture;
#[cfg(target_os = "android")]
use crate::video_android::{Player, caption_style};
#[cfg(all(feature = "gpu-draw", target_vendor = "apple"))]
pub(crate) use crate::video_apple::FrameTexture;
#[cfg(target_vendor = "apple")]
use crate::video_apple::{Player, caption_style};
#[cfg(all(feature = "gpu-draw", unix, not(any(target_vendor = "apple", target_os = "android"))))]
pub(crate) use crate::video_gstreamer::FrameTexture;
#[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
use crate::video_gstreamer::{Player, caption_style};
#[cfg(target_family = "wasm")]
use crate::video_web::{Player, caption_style};
#[cfg(all(feature = "gpu-draw", windows))]
pub(crate) use crate::video_windows::FrameTexture;
#[cfg(windows)]
use crate::video_windows::{Player, caption_style};

/// (Tests: videos' frames read back from the GPU so far.)
#[cfg(all(test, any(windows, feature = "gpu-draw"), not(target_family = "wasm")))]
pub(crate) static READ_BACK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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

/// A sound or caption track: its language, as the file or app names it ("en", "eng",
/// "en-US"), and its name; either may be empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    /// Its language.
    pub language: String,
    /// Its name.
    pub name: String,
}

impl Track {
    /// What a menu calls it: its name, else its language, else "Track" and its number from 1.
    fn label(&self, nth: usize) -> String {
        match (self.name.is_empty(), self.language.is_empty()) {
            (false, _) => self.name.clone(),
            (true, false) => self.language.clone(),
            (true, true) => format!("Track {}", nth + 1),
        }
    }
}

/// The tracks a player found in its source, and which sound track plays.
#[derive(Default, Clone)]
pub(crate) struct Tracks {
    pub sound: Vec<Track>,
    pub captions: Vec<Track>,
    pub sound_chosen: Option<usize>,
}

/// How captions look, as the system's caption settings say where it has them: their text and
/// background colours, and their size against the usual.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CaptionStyle {
    pub text: Color,
    pub background: Color,
    pub scale: f32,
}

impl Default for CaptionStyle {
    /// White on black at 80%, the usual size.
    fn default() -> Self {
        CaptionStyle { text: Color::WHITE, background: Color::BLACK.with_alpha(0.8), scale: 1.0 }
    }
}

/// The caption track chosen: the file's or the app's, by its place among them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Captioned {
    File(usize),
    App(usize),
}

/// What the system's player said since it was last asked.
#[derive(Default)]
pub(crate) struct News {
    /// It played to the end.
    pub ended: bool,
    /// It stopped with this error.
    pub error: Option<String>,
    /// The error clears at the next play: a browser waiting for a press.
    pub clears_on_play: bool,
    /// The source's tracks changed since they were read: captions in the video stream found
    /// once they play.
    pub tracks_changed: bool,
    /// A thread of its own still reads the captions in the video stream: tracks may come while
    /// the video stands still.
    pub reading_captions: bool,
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
    /// The player still reads the captions in the video stream (asked again though paused).
    reading_captions: bool,
    /// The time between frames, as seen; and the last frame's time.
    gap: Duration,
    last: Option<Duration>,
    /// Frames shown so far.
    frames: u64,
    /// The box's size at the last layout, for filling.
    room: (f32, f32),
    /// The source's sound tracks once known, and the one playing.
    sound_tracks: Vec<Track>,
    sound_track: Option<usize>,
    /// The source's caption tracks once known, then the app's, with the app's cues.
    caption_tracks: Vec<Track>,
    file_captions: usize,
    tracks_known: bool,
    app_cues: Vec<Vec<Cue>>,
    captioned: Option<Captioned>,
    /// The captions on show as of the last run, their text (a caption a line), whether it is new
    /// since readers were told, and how they look.
    shown: Vec<Caption>,
    caption: Option<String>,
    caption_new: bool,
    style: CaptionStyle,
    speed: f32,
}

impl std::fmt::Debug for Video {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Video")
            .field("playing", &self.playing)
            .field("position", &self.position)
            .field("length", &self.length)
            .field("size", &self.size)
            .field("speed", &self.speed)
            .field("caption", &self.caption)
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
        Ok(Video::playing_on(Player::open(source, sound)?, sound))
    }
    /// A video of this player, paused, with sound or not.
    fn playing_on(player: Player, sound: bool) -> Video {
        Video {
            player,
            picture: None,
            serial: OPENED.fetch_add(1, Ordering::Relaxed),
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
            reading_captions: false,
            gap: Duration::from_millis(16),
            last: None,
            frames: 0,
            room: (0.0, 0.0),
            sound_tracks: Vec::new(),
            sound_track: None,
            caption_tracks: Vec::new(),
            file_captions: 0,
            tracks_known: false,
            app_cues: Vec::new(),
            captioned: None,
            shown: Vec::new(),
            caption: None,
            caption_new: false,
            style: caption_style(),
            speed: 1.0,
        }
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

    /// Plays at this speed, from 0.25 to 2 times (1 is the usual); the sound keeps its pitch
    /// where the system can.
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = if speed.is_nan() { 1.0 } else { speed.clamp(0.25, 2.0) };
        self.player.set_speed(self.speed);
    }
    /// Its speed.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// The source's sound tracks, once the player has read it; empty before, or with none.
    pub fn sound_tracks(&self) -> &[Track] {
        &self.sound_tracks
    }
    /// The sound track playing, by its place in [`sound_tracks`](Self::sound_tracks).
    pub fn sound_track(&self) -> Option<usize> {
        self.sound_track
    }
    /// Plays this sound track (by its place in [`sound_tracks`](Self::sound_tracks)) in place
    /// of the one playing; ignored for one there is not.
    pub fn set_sound_track(&mut self, nth: usize) {
        if nth < self.sound_tracks.len() {
            self.sound_track = Some(nth);
            self.player.choose_sound(nth);
        }
    }

    /// Its caption tracks: the source's own (once the player has read it), then the app's
    /// ([`add_captions`](Self::add_captions)).
    pub fn caption_tracks(&self) -> &[Track] {
        &self.caption_tracks
    }
    /// The caption track shown, by its place in [`caption_tracks`](Self::caption_tracks);
    /// none while captions are off (the default).
    pub fn caption_track(&self) -> Option<usize> {
        match self.captioned? {
            Captioned::File(nth) => Some(nth),
            Captioned::App(nth) => Some(self.file_captions + nth),
        }
    }
    /// Shows this caption track (by its place in [`caption_tracks`](Self::caption_tracks)),
    /// or none: captions off. Ignored for one there is not.
    pub fn set_caption_track(&mut self, nth: Option<usize>) {
        let chosen = match nth {
            None => None,
            Some(nth) if nth < self.file_captions => Some(Captioned::File(nth)),
            Some(nth) if nth < self.caption_tracks.len() => Some(Captioned::App(nth - self.file_captions)),
            Some(_) => return,
        };
        let file = match chosen {
            Some(Captioned::File(nth)) => Some(nth),
            _ => None,
        };
        self.captioned = chosen;
        self.player.choose_captions(file);
        let shown = self.captions_now();
        self.set_captions(shown);
    }
    /// Adds the app's captions, WebVTT, SRT or SSA (ASS) text, as a caption track named `name`
    /// in `language`: its place in [`caption_tracks`](Self::caption_tracks). Each cue keeps its
    /// text, its place (WebVTT's settings and regions, SRT's `{\an8}`, SSA's alignment, margins
    /// and `\pos`) and italics; not its colours or fonts.
    ///
    /// ```no_run
    /// # fn add(video: &mut taut::Video) -> Result<(), taut::CaptionsError> {
    /// let english = video.add_captions(&std::fs::read_to_string("clip.en.vtt").unwrap_or_default(), "en", "English")?;
    /// video.set_caption_track(Some(english));
    /// # Ok(()) }
    /// ```
    pub fn add_captions(&mut self, text: &str, language: &str, name: &str) -> Result<usize, CaptionsError> {
        self.app_cues.push(crate::video_captions::read(text)?);
        self.caption_tracks.push(Track { language: language.into(), name: name.into() });
        Ok(self.caption_tracks.len() - 1)
    }
    /// The caption on show, as of the last run, a line for each where several show; none while
    /// captions are off or between cues.
    pub fn caption(&self) -> Option<&str> {
        self.caption.as_deref()
    }

    /// Asks the player what changed and takes its newest frame, once per moment of the
    /// interface (a rerun shows the same); asks to run again in half the time between frames.
    /// (Asked once a gap, one frame missed doubles the gap seen, and from then on every other
    /// frame is missed: a 30 fps video showed 10 fps.) Paused, it asks again soon for a frame
    /// due, and every 100 ms while the player reads captions.
    fn update(&mut self, ui: &mut Ui) {
        if self.asked != Some(ui.now) {
            self.ask(ui);
        }
        let waits = self.error.is_none() && (self.playing || self.awaited || self.reading_captions);
        if !waits || ui.window_minimized() {
            return;
        }
        ui.wake_after(match (self.playing, self.awaited) {
            (true, _) => (self.gap / 2).div_f32(self.speed),
            (false, true) => Duration::from_millis(10),
            (false, false) => Duration::from_millis(100),
        });
    }
    /// Takes the source's tracks once the player knows them: they come before the app's
    /// caption tracks. A track of the source's shown stays shown where tracks found later
    /// come before it.
    fn take_tracks(&mut self) {
        let Some(found) = self.player.tracks() else { return };
        let chosen = match self.captioned {
            Some(Captioned::File(nth)) => self.caption_tracks.get(nth).map(|track| (track.clone(), self.caption_tracks[..nth].iter().filter(|other| *other == track).count())),
            _ => None,
        };
        let app = self.caption_tracks.split_off(self.file_captions);
        (self.file_captions, self.tracks_known) = (found.captions.len(), true);
        (self.sound_tracks, self.sound_track, self.caption_tracks) = (found.sound, found.sound_chosen, found.captions);
        self.caption_tracks.extend(app);
        let Some((track, earlier)) = chosen else { return };
        let moved = self.caption_tracks[..self.file_captions].iter().enumerate().filter(|(_, other)| **other == track).nth(earlier).map(|(at, _)| at);
        if let Some(at) = moved.filter(|at| self.captioned != Some(Captioned::File(*at))) {
            self.captioned = Some(Captioned::File(at));
            self.player.choose_captions(Some(at));
        }
    }
    /// The captions on show at its position.
    fn captions_now(&mut self) -> Vec<Caption> {
        match self.captioned {
            None => Vec::new(),
            Some(Captioned::File(_)) => self.player.captions(),
            Some(Captioned::App(nth)) => crate::video_captions::captions_at(&self.app_cues[nth], self.position),
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
        (self.tracks_known, self.reading_captions) = (self.tracks_known && !news.tracks_changed, news.reading_captions);
        self.take_frame(ui);
        (self.position, self.length) = (self.player.position(), self.player.length().or(self.length));
        if !self.tracks_known {
            self.take_tracks();
        }
        let shown = self.captions_now();
        self.set_captions(shown);
    }
    /// The captions on show from now: readers are told their text where it is new.
    fn set_captions(&mut self, shown: Vec<Caption>) {
        let texts: Vec<&str> = shown.iter().map(|caption| caption.text.as_str()).filter(|text| !text.is_empty()).collect();
        let caption = (!texts.is_empty()).then(|| texts.join("\n"));
        self.caption_new |= caption.is_some() && caption != self.caption;
        (self.caption, self.shown) = (caption, shown);
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
    /// Takes the player's newest frame, if it has one: left on the GPU where it decodes there,
    /// else as pixels.
    fn take_frame(&mut self, ui: &mut Ui) {
        #[cfg(all(feature = "gpu-draw", not(target_family = "wasm")))]
        if self.player.decodes_on_gpu()
            && let Some((texture, at)) = self.player.frame_texture()
        {
            return self.show_texture(ui, texture, at);
        }
        let mut pixels = Vec::new();
        if let Some((width, at)) = self.player.frame(&mut pixels) {
            self.show(ui, width, pixels, at);
        }
    }
    /// A new frame came, taken at `at`, this big.
    fn came(&mut self, at: Duration, size: (usize, usize)) {
        if let Some(gap) = self.last.and_then(|last| at.checked_sub(last)).filter(|gap| !gap.is_zero()) {
            self.gap = gap.clamp(Duration::from_millis(4), Duration::from_millis(100));
        }
        (self.last, self.awaited, self.frames, self.size) = (Some(at), false, self.frames + 1, Some(size));
    }
    /// Shows a new frame of this width, taken at `at`: cut to its box's shape when filling.
    fn show(&mut self, ui: &mut Ui, width: usize, pixels: Vec<u32>, at: Duration) {
        self.came(at, (width, pixels.len() / width.max(1)));
        let (width, pixels) = match self.fill {
            true => cut_to(width, pixels, self.room),
            false => (width, pixels),
        };
        match self.picture {
            Some(picture) => ui.set_picture(picture, width, pixels),
            None => self.picture = Some(ui.add_picture(width, pixels)),
        }
    }
    /// Shows a new frame left on the GPU, taken at `at`: its middle when filling.
    #[cfg(all(feature = "gpu-draw", not(target_family = "wasm")))]
    fn show_texture(&mut self, ui: &mut Ui, texture: std::sync::Arc<FrameTexture>, at: Duration) {
        let size = (texture.size.0 as usize, texture.size.1 as usize);
        self.came(at, size);
        let crop = if self.fill { middle(size, self.room) } else { [0, 0, size.0, size.1] };
        let frame = FrameOnGpu { texture, crop, read: false };
        match self.picture {
            Some(picture) => ui.set_picture_texture(picture, frame),
            None => self.picture = Some(ui.add_picture_texture(frame)),
        }
    }
}

/// The size of the frame shown in a box `room` wide and high: all of it when filling or before
/// the frame's size is known, else as big as fits with the frame's shape.
fn frame_size(size: Option<(usize, usize)>, room: (f32, f32), fill: bool) -> (f32, f32) {
    let Some((width, height)) = size.filter(|(width, height)| !fill && *width > 0 && *height > 0) else { return room };
    let scale = (room.0 / width as f32).min(room.1 / height as f32);
    (width as f32 * scale, height as f32 * scale)
}

/// How captions look over a frame: the system's caption style, the text size, and the frame's.
struct CaptionLooks {
    style: CaptionStyle,
    size: f32,
    frame: (f32, f32),
}

impl CaptionLooks {
    /// A caption's box around its text, at most `widest` wide.
    fn boxed<'a>(&self, el: El<'a>, widest: f32) -> El<'a> {
        let size = self.size;
        el.pad((size / 6.0, size / 3.0)).radius(size / 6.0).fill(self.style.background).max(widest, f32::MAX)
    }
    fn font(&self, italic: bool) -> Font {
        if italic { Font::new(self.size).italic() } else { Font::new(self.size) }
    }
}

/// The middle of a frame with the shape of a box `room` wide and high: all of it where the box
/// has no size yet.
fn cut_to(width: usize, pixels: Vec<u32>, room: (f32, f32)) -> (usize, Vec<u32>) {
    let [left, top, cut_w, cut_h] = middle((width, pixels.len() / width.max(1)), room);
    if cut_w == width {
        return (width, pixels);
    }
    let rows = pixels.chunks_exact(width).skip(top).take(cut_h);
    (cut_w, rows.flat_map(|row| &row[left..left + cut_w]).copied().collect())
}

/// Where the middle of a frame `width` by `height` with the shape of a box `room` wide and
/// high lies in it: left, top, width, height. All of it where the box has no size yet.
fn middle((width, height): (usize, usize), (wide, high): (f32, f32)) -> [usize; 4] {
    if wide <= 0.0 || high <= 0.0 || height == 0 || width == 0 {
        return [0, 0, width, height];
    }
    let shape = wide / high;
    let (cut_w, cut_h) = match (width as f32 / height as f32) > shape {
        true => (((height as f32 * shape).round() as usize).clamp(1, width), height),
        false => (width, ((width as f32 / shape).round() as usize).clamp(1, height)),
    };
    [(width - cut_w) / 2, (height - cut_h) / 2, cut_w, cut_h]
}

/// A video's frame left on the GPU, shown by a picture: its texture, the part shown (left,
/// top, width, height), and whether the picture's pixels hold it yet (read back only for the
/// CPU).
#[cfg(all(feature = "gpu-draw", not(target_family = "wasm")))]
pub(crate) struct FrameOnGpu {
    pub texture: std::sync::Arc<FrameTexture>,
    pub crop: [usize; 4],
    pub read: bool,
}

#[cfg(all(feature = "gpu-draw", not(target_family = "wasm")))]
impl FrameOnGpu {
    /// Reads the part shown back into `into`: whether it could.
    pub(crate) fn read_into(&self, into: &mut Vec<u32>) -> bool {
        let [left, top, width, height] = self.crop;
        let size = (self.texture.size.0 as usize, self.texture.size.1 as usize);
        if (left, top, width, height) == (0, 0, size.0, size.1) {
            return self.texture.read(into);
        }
        let mut whole = Vec::new();
        if !self.texture.read(&mut whole) {
            return false;
        }
        let rows = whole.chunks_exact(size.0).skip(top).take(height);
        into.clear();
        into.extend(rows.flat_map(|row| &row[left..left + width]));
        true
    }
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
/// A screen with two lines of text.
const CAPTIONS: Icon = Icon::line("M1.5 3.5h13v9h-13zM4 8.5h5M10.5 8.5H12M4 10.5h2M7.5 10.5H12");
/// A speech bubble.
const SOUND_TRACKS: Icon = Icon::line("M2.5 3h11v7.5h-6l-3 2.5v-2.5h-2z");
/// A dial.
const SPEED: Icon = Icon::line("M2.5 11.5a5.5 5.5 0 1 1 11 0M8 11.5l2.5-3.5");
/// The speeds the controls offer.
const SPEEDS: [(f32, &str); 8] = [(0.25, "0.25×"), (0.5, "0.5×"), (0.75, "0.75×"), (1.0, "Normal"), (1.25, "1.25×"), (1.5, "1.5×"), (1.75, "1.75×"), (2.0, "2×")];

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
        let (picture, size) = (video.picture, video.size.unwrap_or((0, 0)));
        // (Sensing, so its place is kept for filling and for its captions' places.)
        let el = self.el().key(key).fill(Color::BLACK).sense();
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
        let chosen = self.track_menus(video);
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
        row.changed = toggled || moved || chosen || muting || loud;
        self.again |= row.changed;
        row
    }
    /// The video's captions on show, over its frame, styled as the system's caption settings say
    /// where it has them: each where its source puts it (rows and windows of captions in the
    /// stream, WebVTT's settings, SRT's `{\an8}`, SSA's alignment and `\pos`), the rest in a box
    /// at the bottom middle; empty while none shows. Told to screen readers, and said by them as
    /// they change. Put it after [`video`](Self::video) in a stack the video fills, so it lies
    /// over the frame (see [the module](crate::Video)).
    pub fn video_captions(&mut self, video: &mut Video) -> Response {
        let room = self.visible_rect(self.id(("taut video", video.serial)));
        let Some(text) = video.caption.clone() else { return self.el().size(0.0, 0.0).place((Align::Center, Align::End)).show() };
        if std::mem::take(&mut video.caption_new) {
            self.announce(&text);
        }
        let frame = frame_size(video.size, (room.w, room.h), video.fill);
        let looks = CaptionLooks { style: video.style, size: (frame.1 / 18.0).max(self.theme.font.size) * video.style.scale, frame };
        let (placed, usual): (Vec<&Caption>, Vec<&Caption>) = video.shown.iter().partition(|caption| caption.place.is_some() && room.w > 0.0);
        if placed.is_empty() {
            return self.usual_caption(&text, usual.iter().all(|caption| caption.italic), &looks);
        }
        let response = self.el().flow(Flow::Stack).place(Align::Center).size(frame.0, frame.1).open();
        for caption in placed {
            self.placed_caption(caption, &looks);
        }
        if !usual.is_empty() {
            let text = usual.iter().map(|caption| caption.text.as_str()).collect::<Vec<_>>().join("\n");
            self.usual_caption(&text, usual.iter().all(|caption| caption.italic), &looks);
        }
        self.close();
        response
    }
    /// Captions at the usual place: the bottom middle, as wide as the frame leaves room for once
    /// it was laid out.
    fn usual_caption(&mut self, text: &str, italic: bool, looks: &CaptionLooks) -> Response {
        let size = looks.size;
        let widest = if looks.frame.0 > 0.0 { (looks.frame.0 - size).max(size) } else { f32::MAX };
        let el = self.el().place((Align::Center, Align::End)).offset(0.0, -size / 2.0);
        looks.boxed(el, widest).text(text, looks.font(italic), looks.style.text).text_align(Align::Center).wrap().show()
    }
    /// A caption where its source puts it on the frame: the side of its box it names at its
    /// place, no wider than its place and the frame leave room for.
    fn placed_caption(&mut self, caption: &Caption, looks: &CaptionLooks) {
        let Some(place) = caption.place else { return };
        let ((width, height), size) = (looks.frame, looks.size);
        let line = LINE * height * looks.style.scale;
        let (across, x) = match place.across {
            Align::End => (Align::End, place.x * width - width),
            Align::Center => (Align::Center, place.x * width - width / 2.0),
            _ => (Align::Start, place.x * width),
        };
        let (down, y) = match place.down {
            Down::At(at, Align::End) => (Align::End, at * height - height),
            Down::At(at, Align::Center) => (Align::Center, at * height - height / 2.0),
            Down::At(at, _) => (Align::Start, at * height),
            Down::Line(from_top @ 0..) => (Align::Start, from_top as f32 * line),
            Down::Line(from_bottom) => (Align::End, (from_bottom + 1) as f32 * line - size / 2.0),
        };
        let room = match place.across {
            Align::End => place.x,
            Align::Center => 2.0 * place.x.min(1.0 - place.x),
            _ => 1.0 - place.x,
        };
        let widest = (place.width.min(room) * width - size / 1.5).max(size);
        let el = self.el().place((across, down)).offset(x, y);
        looks.boxed(el, widest).text(&caption.text, looks.font(caption.italic), looks.style.text).text_align(place.lines).wrap().show();
    }
    /// Menus for the caption track, the sound track (where there are several) and the speed:
    /// whether one was chosen.
    fn track_menus(&mut self, video: &mut Video) -> bool {
        let captions = self.caption_menu(video);
        let sound = self.sound_menu(video);
        let speed = self.speed_menu(video);
        captions || sound || speed
    }
    fn caption_menu(&mut self, video: &mut Video) -> bool {
        if video.caption_tracks.is_empty() {
            return false;
        }
        let (mut chosen, labels) = (video.caption_track(), labels(&video.caption_tracks));
        let options = labels.iter().enumerate().map(|(nth, label)| (Some(nth), label.as_str()));
        let changed = self.menu_of(CAPTIONS, "Captions", &mut chosen, &[(None, "Off")], options);
        if changed {
            video.set_caption_track(chosen);
        }
        changed
    }
    fn sound_menu(&mut self, video: &mut Video) -> bool {
        if video.sound_tracks.len() < 2 {
            return false;
        }
        let (mut chosen, labels) = (video.sound_track, labels(&video.sound_tracks));
        let options = labels.iter().enumerate().map(|(nth, label)| (Some(nth), label.as_str()));
        let changed = self.menu_of(SOUND_TRACKS, "Sound track", &mut chosen, &[], options);
        if let (true, Some(nth)) = (changed, chosen) {
            video.set_sound_track(nth);
        }
        changed
    }
    fn speed_menu(&mut self, video: &mut Video) -> bool {
        let mut speed = video.speed;
        let changed = self.menu_of(SPEED, "Speed", &mut speed, &[], SPEEDS.into_iter());
        if changed {
            video.set_speed(speed);
        }
        changed
    }
    /// A button named `name` opening a menu of `first` then `rest`, each a value and its label,
    /// the one equal to `value` marked: whether one was chosen (into `value`).
    fn menu_of<'a, T: PartialEq + Copy>(&mut self, icon: Icon, name: &str, value: &mut T, first: &[(T, &'a str)], rest: impl Iterator<Item = (T, &'a str)>) -> bool {
        let button = self.icon_button(icon, name);
        if button.clicked {
            self.open_popup(button.id, button.rect);
        }
        if !self.menu(button.id) {
            return false;
        }
        let was = *value;
        for (option, label) in first.iter().copied().chain(rest) {
            self.menu_radio(value, option, label);
        }
        self.close();
        *value != was
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

/// What a menu calls each track.
fn labels(tracks: &[Track]) -> Vec<String> {
    tracks.iter().enumerate().map(|(nth, track)| track.label(nth)).collect()
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
pub(crate) mod tests {
    use super::*;

    /// (Each system uses some of them.)
    #[allow(dead_code)]
    pub(crate) mod files {
        include!("../tests/video/files.rs");
    }
    use files::*;

    /// Frames 0 to 29 at 10 a second: a lossless H.264 MP4 where the system plays H.264; on
    /// Linux an MJPEG AVI where GStreamer reads those, else Theora in Ogg made by `ffmpeg`.
    /// `sound` adds a silent track.
    pub(crate) fn test_file(sound: bool) -> Option<PathBuf> {
        let name = if sound { "with-sound" } else { "silent" };
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        if !crate::video_gstreamer::has_element(c"avidemux") || !crate::video_gstreamer::has_element(c"jpegdec") {
            return theora(name, 30, if sound { &[SILENT] } else { &[] });
        }
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        return Some(written(&format!("{name}.avi"), &avi(30, 10, sound)));
        #[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
        Some(written(&format!("{name}.mp4"), &mp4(30, 10, sound)))
    }
    /// A silent sound track.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    const SILENT: Sound = Sound { language: "und", name: "Sound", tone: 0 };

    /// The languages file's frames: ten seconds. (A player switching sound tracks waits for the
    /// next buffer of the track chosen; in a file of a few seconds, its buffers have all gone.)
    const LONG: usize = 100;

    /// Frames as `test_file`'s, ten seconds of them, with two sound tracks (English at 400 Hz,
    /// German at 1000 Hz) and two caption tracks (English, French), as this system's player reads
    /// them; and whether it reads the captions. Matroska on Windows (Media Foundation reads no
    /// captions in MP4), MP4 elsewhere (with JPEG frames on Linux, where GStreamer reads those);
    /// else, on Linux, Ogg by `ffmpeg`, without captions (no plugin there reads them in Ogg).
    fn languages_file() -> Option<(PathBuf, bool)> {
        #[cfg(windows)]
        return Some((written("languages.mkv", &matroska(LONG, 10, &TWO_LANGUAGES, &TWO_CAPTIONS)), true));
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        if !crate::video_gstreamer::has_element(c"qtdemux") || !crate::video_gstreamer::has_element(c"jpegdec") {
            return theora("languages", LONG, &TWO_LANGUAGES).map(|file| (file, false));
        }
        #[cfg(not(windows))]
        Some((written("languages.mp4", &movie(LONG, 10, cfg!(target_os = "linux"), &TWO_LANGUAGES, &TWO_CAPTIONS)), true))
    }

    /// The same frames as Theora in Ogg (every frame a key frame), by `ffmpeg`, with a Vorbis
    /// track for each sound (its tone as a sine, or silence), named for its language.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn theora(name: &str, count: usize, sounds: &[Sound]) -> Option<PathBuf> {
        let raw: Vec<u8> = (0..count).flat_map(frame_pixels).flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8]).collect();
        let frames = written(&format!("{name}.rgb"), &raw);
        let out = frames.with_extension("ogv");
        let mut ffmpeg = std::process::Command::new("ffmpeg");
        ffmpeg.args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "64x48", "-r", "10", "-i"]).arg(&frames);
        for sound in sounds {
            let source = match sound.tone {
                0 => "anullsrc=r=8000:cl=mono".to_owned(),
                hertz => format!("sine=frequency={hertz}:sample_rate=8000"),
            };
            ffmpeg.args(["-f", "lavfi", "-t", &(count / 10).to_string(), "-i", &source]);
        }
        ffmpeg.args(["-map", "0"]);
        for (nth, sound) in sounds.iter().enumerate() {
            let tag = format!("-metadata:s:a:{nth}");
            ffmpeg.args(["-map", &(nth + 1).to_string(), &tag, &format!("language={}", sound.language), &tag, &format!("title={}", sound.name)]);
        }
        let made = ffmpeg.args(["-c:a", "libvorbis", "-c:v", "libtheora", "-q:v", "10", "-g", "1"]).arg(&out).status();
        let made = made.is_ok_and(|status| status.success());
        skipped(made, "GStreamer here reads no MJPEG, and no ffmpeg makes Theora").then_some(out)
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
        until_within(trial, what, Duration::from_secs(10), done);
    }
    /// The same within `limit`.
    fn until_within(trial: &mut Trial<Video>, what: &str, limit: Duration, done: impl Fn(&Video) -> bool) {
        // (Run first: the test changed the video from outside, as an app's timer would.)
        trial.run();
        let start = std::time::Instant::now();
        while !done(&trial.app) {
            assert!(start.elapsed() < limit, "{what} did not happen: {:?}", trial.app);
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
            // (Never ahead of the position. A busy computer's player hands frames late, as any
            // player does: that they keep coming is what the end holds.)
            let (start, at) = (number as f32 / 10.0, trial.app.position().as_secs_f32());
            assert!(at > start - 0.08, "frame {number} shown at {at} s");
            assert!(changed.is_empty() || changed.x >= 0.0 && changed.right() <= 64.0 && changed.bottom() <= 48.0, "redrawn: {changed:?}");
            seen.push(number);
        }
        assert!(seen.windows(2).all(|two| two[0] <= two[1]) && seen.last() >= Some(&25), "frames shown: {seen:?}");
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn a_playing_video_asks_again_sooner_than_its_frame_time() {
        let _alone = alone();
        let Some(video) = opened(false) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.play();
        until(&mut trial, "frames", |video| video.frames_shown() >= 3);
        // (Frames come every 100 ms: asked once a gap seen, a frame missed would be every other.)
        assert!(trial.ui.wake_in().is_some_and(|wait| wait < Duration::from_millis(100)), "asks again in {:?}", trial.ui.wake_in());
    }

    #[test]
    #[cfg(target_os = "android")]
    fn the_decoders_output_format_is_read_when_it_changes_not_for_every_frame() {
        use std::sync::atomic::Ordering::Relaxed;
        let _alone = alone();
        let Some(video) = opened(true) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        let before = crate::video_android::FORMAT_READS.load(Relaxed);
        trial.app.play();
        until(&mut trial, "the end", |video| !video.playing());
        let read = crate::video_android::FORMAT_READS.load(Relaxed) - before;
        assert!(shown(&trial) >= 25 && read <= 1, "the format read {read} times for {} frames", shown(&trial));
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
        // Past Speed (this file has no caption tracks, and one sound track at most) to Mute.
        trial.key("Tab");
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

    /// Opens, plays a moment and drops a video, `times` over. (The first frame may take 30 s:
    /// with five busy threads a core, Media Foundation's first opens took 8 s here.)
    #[cfg(not(target_vendor = "apple"))]
    fn open_and_close(times: usize) {
        for _ in 0..times {
            let Some(video) = opened(true) else { return };
            let mut trial = trial(video, false);
            until_within(&mut trial, "the first frame", Duration::from_secs(30), |video| video.size().is_some());
            trial.app.play();
            pass(&mut trial, Duration::from_millis(100));
            let Trial { app, mut ui, .. } = trial;
            ui.drop_video(app);
        }
    }

    /// The memory this process holds now, in bytes: on Windows its private bytes, on Linux and
    /// Android its resident ones. (Not the most it ever held: a passing high while 40 videos
    /// play is no leak.)
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
            // SAFETY: `PROCESS_MEMORY_COUNTERS_EX`, as large as said; this process's handle needs no
            // closing.
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, std::mem::size_of_val(&counters) as u32) };
            // `PrivateUsage`, its last field.
            counters[9]
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
        // (Warmed up first, until 20 more opens hold under 2 MB more, ten times 20 at most: the
        // GPU's driver and Media Foundation keep some 10 MB of their own over the first few
        // dozen videos decoded on the GPU, more of them on a busy computer, then no more.)
        let mut held = memory_held();
        for _ in 0..10 {
            open_and_close(20);
            let (was, now) = (held, memory_held());
            held = now;
            if now < was + (2 << 20) {
                break;
            }
        }
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

    /// The languages file opened so that what it plays can be heard here, and no speaker plays
    /// it: with sound, muted, on Windows (it reaches the effect that listens) and Android (what
    /// goes to the speaker is kept before the volume); silent and not muted on Linux (it goes to a
    /// quiet sink, which keeps it as played).
    fn opened_to_hear(file: &Path) -> Option<Video> {
        #[cfg(windows)]
        let made = Player::listening(file.into()).map(|player| Video::playing_on(player, true));
        #[cfg(target_os = "android")]
        let made = Video::open(file);
        #[cfg(not(any(windows, target_os = "android")))]
        let made = Video::open_silent(file);
        let muted = cfg!(any(windows, target_os = "android"));
        player(made).map(|mut video| {
            video.set_muted(muted);
            video
        })
    }

    /// The sound the video's player handed on last, where this system's player lets it be heard
    /// here: its first channel, and its rate.
    fn heard_samples(video: &mut Video) -> Option<(Vec<f32>, u32)> {
        #[cfg(any(windows, target_os = "android", all(unix, not(target_vendor = "apple"))))]
        return video.player.heard();
        #[cfg(not(any(windows, target_os = "android", all(unix, not(target_vendor = "apple")))))]
        {
            let _ = video;
            None
        }
    }
    /// The tone the video plays now, in Hz, where this system's player lets it be heard here.
    fn heard(video: &mut Video) -> Option<f32> {
        heard_samples(video).map(|(samples, rate)| hertz_of(&samples, rate))
    }

    /// The first two letters of each track's language.
    fn languages(tracks: &[Track]) -> Vec<String> {
        tracks.iter().map(|track| track.language.chars().take(2).collect()).collect()
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_sources_sound_tracks_are_listed_and_the_one_chosen_is_heard() {
        let _alone = alone();
        let Some(video) = languages_file().and_then(|(file, _)| opened_to_hear(&file)) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the sound tracks", |video| video.sound_tracks().len() == 2);
        // (In the file's order, which some demuxers do not keep.)
        let languages = languages(trial.app.sound_tracks());
        let place = |language: &str| languages.iter().position(|of| of == language).unwrap_or_else(|| panic!("no {language} in {languages:?}"));
        let (english, german) = (place("en"), place("de"));
        assert!(trial.app.sound_track().is_some());
        trial.app.set_looping(true);
        trial.app.play();
        for (nth, tone) in [(german, 1000.0), (english, 400.0), (german, 1000.0)] {
            trial.app.set_sound_track(nth);
            assert_eq!(trial.app.sound_track(), Some(nth));
            trial.app.set_sound_track(7);
            assert_eq!(trial.app.sound_track(), Some(nth), "a track there is not is ignored");
            let start = std::time::Instant::now();
            pass(&mut trial, Duration::from_millis(600));
            while heard(&mut trial.app).is_none() && start.elapsed() < Duration::from_secs(3) {
                pass(&mut trial, Duration::from_millis(50));
            }
            let Some(mut hertz) = heard(&mut trial.app) else { return eprintln!("this player's sound can't be heard here: the tones skipped") };
            while (hertz - tone).abs() > tone * 0.1 && start.elapsed() < Duration::from_secs(4) {
                pass(&mut trial, Duration::from_millis(50));
                hertz = heard(&mut trial.app).unwrap_or(0.0);
            }
            assert!((hertz - tone).abs() <= tone * 0.1, "track {nth}: heard {hertz} Hz, not {tone}");
        }
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_sources_captions_show_at_their_times() {
        let _alone = alone();
        let Some((file, true)) = languages_file() else { return eprintln!("no captions in a file this player reads here: skipped") };
        let Some(video) = player(Video::open_silent(file)) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the caption tracks", |video| video.caption_tracks().len() >= 2);
        let french = languages(trial.app.caption_tracks()).iter().position(|language| language == "fr").expect("French captions");
        trial.app.set_caption_track(Some(french));
        assert_eq!(trial.app.caption_track(), Some(french));
        trial.app.play();
        trial.run();
        let mut seen = Vec::new();
        while trial.app.playing() && trial.app.position() < Duration::from_millis(3200) {
            pass(&mut trial, Duration::from_millis(30));
            seen.push((trial.app.position(), trial.app.caption().map(str::to_owned)));
        }
        // (From the first caption on: a player may read them a moment after it opened.)
        let shown = seen.iter().skip_while(|(_, caption)| caption.is_none());
        for (at, caption) in shown {
            let tenths = at.as_secs_f32() * 10.0;
            // (Away from a cue's edges: the time read and the caption on show may be a moment apart.)
            let near_an_edge = FRENCH.iter().any(|(from, to, _)| (tenths - *from as f32).abs() < 1.5 || (tenths - *to as f32).abs() < 1.5);
            let due = FRENCH.iter().find(|(from, to, _)| (*from as f32) <= tenths && tenths < *to as f32).map(|cue| cue.2);
            assert!(near_an_edge || caption.as_deref() == due, "at {at:?}: {caption:?}, not {due:?}\n{seen:?}");
        }
        for (_, _, text) in FRENCH {
            assert!(seen.iter().any(|(_, caption)| caption.as_deref() == Some(*text)), "{text} never shown: {seen:?}");
        }
        trial.app.set_caption_track(None);
        assert_eq!((trial.app.caption_track(), trial.app.caption()), (None, None));
    }

    /// WebVTT captions: the second second's has two lines.
    const NOTES: &str = "WEBVTT\n\n00:00.000 --> 00:01.000\nFirst\n\n00:01.000 --> 00:02.000\nSecond, <i>two</i>\nlines\n\n00:02.500 --> 00:03.000\nThird";

    /// The video in a stack with its captions over it, in a box 320×240.
    fn captioned(video: &mut Video, ui: &mut Ui) {
        ui.el().flow(crate::layout::Flow::Stack).size(320.0, 240.0).open();
        ui.video(video).grow().show();
        ui.video_captions(video);
        ui.close();
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_apps_captions_show_over_the_video_and_are_told_as_they_change() {
        let _alone = alone();
        let Some(mut video) = opened(false) else { return };
        assert_eq!(video.add_captions("no captions", "en", "Notes"), Err(CaptionsError::NoCues));
        let notes = video.add_captions(NOTES, "en", "Notes").unwrap();
        assert_eq!(video.caption_tracks()[notes], Track { language: "en".into(), name: "Notes".into() });
        let mut trial = Trial::new(video, captioned);
        until(&mut trial, "the first frame and the tracks", |video| video.size().is_some() && video.tracks_known);
        let notes = trial.app.caption_tracks().len() - 1;
        trial.app.set_caption_track(Some(notes));
        for (at, shown) in [(1500, Some("Second, two\nlines")), (2200, None), (500, Some("First"))] {
            trial.app.seek(Duration::from_millis(at));
            until(&mut trial, "the frame after a seek", |video| !video.awaited);
            assert_eq!(trial.app.caption(), shown, "at {at} ms");
            let told = trial.access_nodes().iter().find(|node| node.role == Role::Text && node.name.contains("Second")).map(|node| node.name.clone());
            assert_eq!(told.is_some(), at == 1500, "told: {told:?}\n{}", trial.dump());
        }
        let caption = trial.find(Role::Text, "First").rect;
        assert!(caption.bottom() <= 240.0 && caption.bottom() > 200.0 && (caption.x + caption.w / 2.0 - 160.0).abs() < 1.0, "the caption sits at the bottom middle: {caption:?}");
        // Said as it changes: a run of its own, as a window's, whose announcements are kept.
        let Trial { app: mut video, mut ui, .. } = trial;
        video.seek(Duration::from_millis(2700));
        let mut said = Vec::new();
        let start = std::time::Instant::now();
        while !said.iter().any(|words: &String| words == "Third") && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
            let now = ui.now + Duration::from_millis(10);
            ui.set_time(now);
            ui.run(&mut |ui| captioned(&mut video, ui));
            said.extend(ui.announced().iter().cloned());
        }
        assert_eq!(said, ["Third"]);
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn captions_turned_on_during_a_cue_say_it() {
        let _alone = alone();
        let Some(mut video) = opened(false) else { return };
        video.add_captions(NOTES, "en", "Notes").unwrap();
        let mut trial = Trial::new(video, captioned);
        until(&mut trial, "the first frame and the tracks", |video| video.size().is_some() && video.tracks_known);
        trial.app.seek(Duration::from_millis(500));
        until(&mut trial, "the frame after a seek", |video| !video.awaited);
        let Trial { app: mut video, mut ui, .. } = trial;
        video.set_caption_track(Some(video.caption_tracks().len() - 1));
        ui.run(&mut |ui| captioned(&mut video, ui));
        assert_eq!(ui.announced(), ["First"]);
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn a_faster_or_slower_speed_moves_the_frames_that_much_faster_or_slower() {
        let _alone = alone();
        for speed in [2.0, 0.5] {
            let Some(mut video) = opened(false) else { return };
            video.set_speed(speed);
            video.set_speed(f32::NAN);
            assert_eq!(video.speed(), 1.0);
            video.set_speed(9.0);
            assert_eq!(video.speed(), 2.0);
            video.set_speed(speed);
            let mut trial = trial(video, false);
            until(&mut trial, "the first frame", |video| video.size().is_some());
            trial.app.play();
            until(&mut trial, "playing", |video| video.position() >= Duration::from_millis(200));
            // (The player's own time, read now: the video's is of its last run, which comes
            // only when a frame is due.)
            // (A busy computer's player hands its frames late, as any player does: so the frame
            // shown is held never to be ahead of the video's time, and the frames to move as fast
            // as that time, not to be on time.)
            let (start, from, first) = (std::time::Instant::now(), trial.app.player.position(), shown(&trial));
            while start.elapsed() < Duration::from_millis(800) {
                pass(&mut trial, Duration::from_millis(20));
                assert!(shown(&trial) as f32 <= due(&trial.app) + 1.0, "at {:?} frame {}, ahead", trial.app.position(), shown(&trial));
            }
            let seconds = start.elapsed().as_secs_f32();
            let rate = (trial.app.player.position() - from).as_secs_f32() / seconds;
            assert!((rate - speed).abs() <= speed * 0.2, "at {speed}× the time moved {rate} s a second");
            let (frames, due_frames) = (shown(&trial) as f32 - first as f32, seconds * 10.0 * speed);
            assert!((frames - due_frames).abs() <= 1.5 + due_frames * 0.1, "at {speed}×, {frames} frames came in {seconds} s");
        }
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn a_speed_set_after_a_seek_keeps_the_seek() {
        let _alone = alone();
        let Some(mut video) = opened(false) else { return };
        video.seek(Duration::from_millis(1200));
        video.set_speed(2.0);
        let mut trial = trial(video, false);
        let start = std::time::Instant::now();
        while shown(&trial) != 12 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
            let now = trial.ui.now + Duration::from_millis(20);
            trial.ui.set_time(now);
            trial.run();
        }
        assert_eq!(shown(&trial), 12, "{:?}", trial.app);
    }

    /// What a video plays, heard here. Windows: what it mixes for this process, the video
    /// played at −130 dB (inaudible), since Media Foundation changes the speed after the
    /// effect that hears it. Elsewhere: what the player hands its sound device (`heard`).
    struct Ear {
        #[cfg(windows)]
        capture: crate::video_windows::played::Capture,
        played: Vec<f32>,
        rate: u32,
    }

    impl Ear {
        /// The video opened to be heard, and an ear on it; none where there is no player, or
        /// this system's sound can't be heard here.
        fn open(file: &Path) -> Option<(Video, Ear)> {
            #[cfg(windows)]
            {
                let capture = crate::video_windows::played::Capture::start()?;
                let mut video = player(Video::open(file))?;
                video.set_volume(1e-6);
                Some((video, Ear { capture, played: Vec::new(), rate: crate::video_windows::played::RATE }))
            }
            #[cfg(not(windows))]
            opened_to_hear(file).map(|video| (video, Ear { played: Vec::new(), rate: 0 }))
        }
        /// Forgets what it heard.
        fn clear(&mut self) {
            #[cfg(windows)]
            self.capture.take();
            self.played.clear();
        }
        /// Takes what was played since last asked.
        fn listen(&mut self, video: &mut Video) {
            #[cfg(windows)]
            {
                let _ = video;
                self.played.extend(self.capture.take());
            }
            #[cfg(not(windows))]
            if let Some((samples, rate)) = heard_samples(video) {
                (self.played, self.rate) = (samples, rate);
            }
        }
        /// The tone of the last half second heard, in Hz, where it sounded: a player starved on
        /// a busy computer leaves gaps of silence, which are not its pitch. None where all was
        /// silent.
        fn tone(&self) -> Option<f32> {
            let last = &self.played[self.played.len().saturating_sub(self.rate as usize / 2)..];
            let sounding: Vec<f32> = last.iter().copied().filter(|sample| *sample != 0.0).collect();
            (sounding.len() > 200).then(|| hertz_of(&sounding, self.rate))
        }
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_pitch_stays_at_other_speeds() {
        let _alone = alone();
        #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
        if !skipped(crate::video_gstreamer::has_element(c"scaletempo"), "GStreamer here has no scaletempo (plugins-good): the pitch moves") {
            return;
        }
        let Some((file, _)) = languages_file() else { return };
        for speed in [0.5, 1.5, 2.0] {
            let Some((mut video, mut ear)) = Ear::open(&file) else { return eprintln!("this player's sound can't be heard here: skipped") };
            video.set_looping(true);
            video.set_speed(speed);
            let mut trial = trial(video, false);
            let ready = |video: &Video| (video.size().is_some() && video.sound_tracks().len() == 2) || video.error().is_some();
            until(&mut trial, "the first frame and the sound tracks", ready);
            if let Some(why) = trial.app.error() {
                return eprintln!("{why} (no sound device?): the tones skipped");
            }
            // (English, at 400 Hz: some demuxers list the tracks out of the file's order.)
            let english = languages(trial.app.sound_tracks()).iter().position(|language| language == "en").expect("an English track");
            trial.app.set_sound_track(english);
            trial.app.play();
            pass(&mut trial, Duration::from_millis(500));
            ear.clear();
            let (start, from) = (std::time::Instant::now(), trial.app.player.position());
            while start.elapsed() < Duration::from_secs(1) {
                pass(&mut trial, Duration::from_millis(20));
                ear.listen(&mut trial.app);
            }
            let rate = (trial.app.player.position() - from).as_secs_f32() / start.elapsed().as_secs_f32();
            assert!((rate - speed).abs() <= speed * 0.2, "at {speed}× the time moved {rate} s a second");
            let Some(hertz) = ear.tone() else { return eprintln!("nothing was heard (no sound device?): the tones skipped") };
            eprintln!("at {speed}× the 400 Hz tone plays at {hertz} Hz");
            assert!((hertz - 400.0).abs() <= 40.0, "at {speed}× the 400 Hz tone plays at {hertz} Hz");
        }
    }

    /// Whether GStreamer here reads `coding`'s frames in MP4 and hands over their captions;
    /// said where not.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn reads_in_stream(coding: Coding) -> bool {
        use crate::video_gstreamer::has_element;
        let (parser, decoders) = match coding {
            Coding::Hevc => (c"h265parse", [c"avdec_h265", c"libde265dec"]),
            _ => (c"h264parse", [c"avdec_h264", c"openh264dec"]),
        };
        let reads = has_element(c"qtdemux") && has_element(parser) && decoders.iter().any(|decoder| has_element(decoder));
        skipped(reads, &format!("GStreamer here reads no such video in MP4 (qtdemux, {parser:?})"))
    }
    /// Every other system's player reads both.
    #[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
    fn reads_in_stream(_: Coding) -> bool {
        true
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn cea608_and_cea708_in_h264_and_hevc_streams_are_caption_tracks_shown_at_their_times() {
        let _alone = alone();
        for (coding, name) in [(Coding::H264, "in-stream.mp4"), (Coding::Hevc, "in-stream-hevc.mp4")] {
            if !reads_in_stream(coding) {
                continue;
            }
            let file = written(name, &movie_captioned_in_stream(30, coding, &IN_STREAM, Carried::InSei));
            let Some(video) = player(Video::open_silent(file)) else { return };
            let mut trial = trial(video, false);
            until(&mut trial, "the first frame", |video| video.size().is_some() || video.error().is_some());
            if let Some(why) = trial.app.error() {
                assert!(coding == Coding::Hevc && cfg!(windows), "{name}: {why}");
                eprintln!("{name}: {why} (no HEVC decoder?): skipped");
                continue;
            }
            // (The frames as made: the HEVC writer's too.)
            assert_eq!(shown(&trial), 0, "{name}");
            trial.app.play();
            // (Found once their first data is read.)
            until(&mut trial, "two caption tracks", |video| video.caption_tracks().len() >= 2);
            let names: Vec<&str> = trial.app.caption_tracks().iter().map(|track| track.name.as_str()).collect();
            assert_eq!(names, ["CC1", "Service 1"], "{name}");
            for (at, track, caption) in IN_STREAM_SHOWN {
                until(&mut trial, "the time", |video| video.position() >= Duration::from_secs_f32(at));
                trial.app.set_caption_track(trial.app.caption_tracks().iter().position(|found| found.name == track));
                trial.run();
                assert_eq!(trial.app.caption(), caption, "{name}, {track} at {:?}", trial.app.position());
            }
        }
    }

    /// The bounds (left, top, right, bottom) of the box of the caption track `name` in the
    /// window's image: the pixels of its background (opaque magenta, for the test).
    fn caption_box(trial: &mut Trial<Video>, name: &str) -> Option<[usize; 4]> {
        let track = trial.app.caption_tracks().iter().position(|track| track.name == name).expect("the track");
        trial.app.style = CaptionStyle { text: Color::WHITE, background: Color::rgb(0xFF00FF), scale: 1.0 };
        trial.app.set_caption_track(Some(track));
        trial.run();
        let width = trial.screen.width;
        let boxed = trial.screen.pixels.iter().enumerate().filter(|(_, pixel)| **pixel & 0xFF_FFFF == 0xFF00FF).map(|(at, _)| (at % width, at / width));
        boxed.fold(None, |bounds, (x, y)| {
            let [left, top, right, bottom] = bounds.unwrap_or([x, y, x + 1, y + 1]);
            Some([left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1)])
        })
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn a_placed_caption_is_drawn_where_its_file_puts_it() {
        let _alone = alone();
        let Some(video) = opened(false) else { return };
        let mut trial = Trial::new(video, captioned);
        until(&mut trial, "the first frame and the tracks", |video| video.size().is_some() && !video.awaited && video.tracks_known);
        trial.app.add_captions("WEBVTT\n\n00:00.000 --> 00:05.000 align:start position:10% line:10%\nTop left", "en", "Top left").unwrap();
        trial.app.add_captions(&format!("{SSA_HEADER}Dialogue: 0,0:00:00.00,0:00:05.00,Default,,0,0,0,,{{\\an5\\pos(320,240)}}Middle\r\n"), "en", "Middle").unwrap();
        trial.app.add_captions("WEBVTT\n\n00:00.000 --> 00:05.000\nUsual", "en", "Usual").unwrap();
        // (A 4:3 frame filling the box of 320×240: a tenth in is 32 across and 24 down.)
        let [left, top, ..] = caption_box(&mut trial, "Top left").expect("the caption drawn");
        assert!(left.abs_diff(32) <= 1 && top.abs_diff(24) <= 1, "WebVTT's caption starts at {left}, {top}, not 32, 24");
        let [left, top, right, bottom] = caption_box(&mut trial, "Middle").expect("the caption drawn");
        let middle = ((left + right) / 2, (top + bottom) / 2);
        assert!(middle.0.abs_diff(160) <= 1 && middle.1.abs_diff(120) <= 1, "SSA's caption is centred at {middle:?}, not 160, 120");
        let [left, _, right, bottom] = caption_box(&mut trial, "Usual").expect("the caption drawn");
        assert!(bottom > 200 && ((left + right) / 2).abs_diff(160) <= 1, "the usual caption sits at the bottom middle: {left}..{right}, to {bottom}");
        assert_eq!(trial.find(Role::Text, "Usual").name, "Usual");
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_other_cea608_channels_in_the_stream_are_tracks_placed_at_their_rows() {
        let _alone = alone();
        if !reads_in_stream(Coding::H264) {
            return;
        }
        let file = written("other-channels.mp4", &movie_captioned_in_stream(30, Coding::H264, &OTHER_CHANNELS, Carried::InSei));
        let Some(video) = player(Video::open_silent(file)) else { return };
        let mut trial = trial(video, false);
        trial.app.play();
        until(&mut trial, "three caption tracks", |video| video.caption_tracks().len() >= 3);
        let names: Vec<&str> = trial.app.caption_tracks().iter().map(|track| track.name.as_str()).collect();
        assert_eq!(names, ["CC2", "CC3", "CC4"]);
        // (Each from the start: a slow machine finds the last track after the first's caption.)
        for (track, caption) in [("CC2", "Second channel"), ("CC3", "Third channel"), ("CC4", "Fourth channel")] {
            trial.app.set_caption_track(trial.app.caption_tracks().iter().position(|found| found.name == track));
            trial.app.seek(Duration::ZERO);
            trial.app.play();
            until(&mut trial, "the caption", |video| video.caption() == Some(caption));
            // (Row 15 of the caption screen, the middle 80% of the frame.)
            let place = trial.app.shown[0].place.expect("a place");
            assert_eq!(place.down, Down::At(0.1 + 0.8 * 14.0 / 15.0, Align::Start), "{track}");
        }
    }

    /// Whether GStreamer here reads H.264 in Matroska (and so its SSA captions, places and
    /// all); said where not.
    #[cfg(all(unix, not(any(target_vendor = "apple", target_os = "android"))))]
    fn reads_placed_ssa() -> bool {
        use crate::video_gstreamer::has_element;
        skipped(has_element(c"matroskademux") && [c"avdec_h264", c"openh264dec"].iter().any(|decoder| has_element(decoder)), "GStreamer here reads no H.264 in Matroska")
    }
    /// Media Foundation reads them; AVFoundation and Android's extractor read no SSA.
    #[cfg(not(all(unix, not(any(target_vendor = "apple", target_os = "android")))))]
    fn reads_placed_ssa() -> bool {
        cfg!(windows)
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn ssa_captions_in_matroska_are_placed_where_their_script_says() {
        let _alone = alone();
        if !reads_placed_ssa() {
            return;
        }
        let file = written("placed.mkv", &matroska(30, 10, &[], &PLAIN_AND_PLACED));
        // (Each track chosen before it plays: GStreamer may not switch in a file this short.)
        let place_of = |language: &str, caption: &str| {
            let video = player(Video::open_silent(file.clone()))?;
            let mut trial = trial(video, false);
            until(&mut trial, "the caption tracks", |video| video.caption_tracks().len() >= 2);
            let track = languages(trial.app.caption_tracks()).iter().position(|found| found == language);
            trial.app.set_caption_track(track);
            trial.app.play();
            until(&mut trial, "the first caption", |video| video.caption() == Some(caption));
            Some(trial.app.shown[0].place)
        };
        // (`\an7\pos(64,48)` in 640 by 480: its top left a tenth in.)
        let Some(Some(place)) = place_of("fr", "Français un") else { return };
        let Down::At(down, Align::Start) = place.down else { panic!("{place:?}") };
        assert!((place.x - 0.1).abs() < 0.02 && place.across == Align::Start && (down - 0.1).abs() < 0.02, "{place:?}");
        assert_eq!(place_of("en", "English one"), Some(None));
    }

    #[test]
    #[cfg(windows)]
    fn caption_tracks_in_mp4_files_are_read_by_taut() {
        let _alone = alone();
        let file = written("mp4-captions.mp4", &movie(30, 10, false, &[], &PLAIN_AND_PLACED));
        let Some(video) = player(Video::open_silent(file)) else { return };
        let mut trial = trial(video, false);
        until(&mut trial, "the caption tracks", |video| video.caption_tracks().len() >= 2);
        assert_eq!(trial.app.caption_tracks(), [Track { language: "eng".into(), name: "English".into() }, Track { language: "fra".into(), name: String::new() }]);
        trial.app.set_caption_track(Some(1));
        trial.app.play();
        until(&mut trial, "the last French caption", |video| video.caption() == Some("Français fin"));
        assert_eq!(trial.app.shown[0].place.map(|place| (place.across, place.down)), Some((Align::Start, Down::At(0.1, Align::Start))));
    }

    #[test]
    #[cfg(windows)]
    fn a_quicktime_closed_caption_track_is_a_track_for_each_cea608_channel_in_it() {
        let _alone = alone();
        let file = written("channels.mov", &movie_captioned_in_stream(30, Coding::H264, &OTHER_CHANNELS, Carried::InCaptionTrack));
        let Some(video) = player(Video::open_silent(file)) else { return };
        let mut trial = trial(video, false);
        trial.app.play();
        until(&mut trial, "three caption tracks", |video| video.caption_tracks().len() >= 3);
        let names: Vec<&str> = trial.app.caption_tracks().iter().map(|track| track.name.as_str()).collect();
        assert_eq!(names, ["CC2", "CC3", "CC4"]);
        until(&mut trial, "the time", |video| video.position() >= Duration::from_millis(1300));
        trial.app.set_caption_track(Some(1));
        trial.run();
        assert_eq!(trial.app.caption(), Some("Third channel"));
    }

    #[test]
    #[cfg(windows)]
    fn an_address_is_downloaded_once_for_the_player_and_its_captions() {
        let _alone = alone();
        for (bytes, track, at, caption) in
            [(movie_captioned_in_stream(30, Coding::H264, &IN_STREAM, Carried::InSei), "CC1", 0.7, "Hello there"), (movie(30, 10, false, &[], &PLAIN_AND_PLACED), "English", 1.5, "English two")]
        {
            let (address, connections, sent) = crate::video_windows_download::tests::serve(bytes.clone());
            let Some(video) = player(Video::open_silent(address.as_str())) else { return };
            let mut trial = trial(video, false);
            trial.app.play();
            until(&mut trial, "the caption track", |video| video.caption_tracks().iter().any(|found| found.name == track));
            trial.app.set_caption_track(trial.app.caption_tracks().iter().position(|found| found.name == track));
            until(&mut trial, "the time", |video| video.position() >= Duration::from_secs_f32(at));
            trial.run();
            assert_eq!(trial.app.caption(), Some(caption), "{track} at {:?}", trial.app.position());
            until(&mut trial, "the end", |video| video.ended || video.error().is_some());
            let (sent, connections) = (sent.load(Ordering::Acquire), connections.load(Ordering::Acquire));
            eprintln!("{track}: {sent} bytes sent for a file of {}, in {connections} connections", bytes.len());
            assert!(sent <= bytes.len() + (64 << 10), "{track}: {sent} bytes sent for a file of {}", bytes.len());
        }
    }

    #[test]
    #[cfg_attr(target_vendor = "apple", ignore = "AVFoundation plays only on the main thread: tests/video_apple.rs")]
    fn the_controls_choose_captions_sound_track_and_speed() {
        let _alone = alone();
        let Some(video) = languages_file().and_then(|(file, _)| player(Video::open_silent(file))) else { return };
        let mut trial = trial(video, true);
        // (And the first frame: the controls move down when it comes.)
        until(&mut trial, "the tracks and the first frame", |video| video.sound_tracks().len() == 2 && video.size().is_some());
        trial.app.add_captions(NOTES, "en", "Notes").unwrap();
        trial.run();
        trial.press(Role::Button, "Captions");
        assert!(trial.has(Role::MenuItemRadio, "Off"), "{}", trial.dump());
        trial.press(Role::MenuItemRadio, "Notes");
        assert_eq!(trial.app.caption_track(), Some(trial.app.caption_tracks().len() - 1));
        let german = trial.app.sound_tracks()[1].label(1);
        trial.press(Role::Button, "Sound track");
        trial.press(Role::MenuItemRadio, &german);
        assert_eq!(trial.app.sound_track(), Some(1));
        trial.press(Role::Button, "Speed");
        trial.press(Role::MenuItemRadio, "1.5×");
        assert_eq!(trial.app.speed(), 1.5);
        trial.press(Role::Button, "Captions");
        trial.press(Role::MenuItemRadio, "Off");
        assert_eq!(trial.app.caption_track(), None);
    }

    /// The frames a player shows at these times, paused there in turn: their pixels; the
    /// player's error where it gave one before its first frame.
    #[cfg(windows)]
    fn frames_at(player: &mut Player, times: [f32; 3]) -> Result<Vec<Vec<u32>>, String> {
        let mut pixels = Vec::new();
        let start = std::time::Instant::now();
        let mut shown = loop {
            assert!(start.elapsed() < Duration::from_secs(10), "no first frame");
            std::thread::sleep(Duration::from_millis(10));
            if let Some(why) = player.news().error {
                return Err(why);
            }
            if let Some((_, at)) = player.frame(&mut pixels) {
                break at;
            }
        };
        let mut frames = Vec::new();
        for at in times.map(Duration::from_secs_f32) {
            if shown.abs_diff(at) >= Duration::from_millis(60) {
                player.seek(at);
            }
            while shown.abs_diff(at) >= Duration::from_millis(60) {
                assert!(start.elapsed() < Duration::from_secs(10), "no frame at {at:?}");
                std::thread::sleep(Duration::from_millis(10));
                player.news();
                shown = player.frame(&mut pixels).map_or(shown, |(_, shown)| shown);
            }
            frames.push(pixels.clone());
        }
        Ok(frames)
    }

    /// The time this process has had the GPU's video decoder, in 100 ns: the PDH counter
    /// `\GPU Engine(pid_<id>_*engtype_VideoDecode)\Running Time`, summed over its engines; none
    /// where it has used no such engine (or there is none: no GPU).
    #[cfg(windows)]
    fn video_decoder_time() -> Option<i64> {
        #[repr(C)]
        struct Item {
            name: *mut u16,
            status: u32,
            value: i64,
        }
        #[link(name = "pdh")]
        unsafe extern "system" {
            fn PdhOpenQueryW(source: *const u16, user: usize, query: *mut isize) -> u32;
            fn PdhAddEnglishCounterW(query: isize, path: *const u16, user: usize, counter: *mut isize) -> u32;
            fn PdhCollectQueryData(query: isize) -> u32;
            fn PdhGetFormattedCounterArrayW(counter: isize, format: u32, size: *mut u32, count: *mut u32, items: *mut Item) -> u32;
            fn PdhCloseQuery(query: isize) -> u32;
        }
        let path = crate::windows::wide(&format!(r"\GPU Engine(pid_{}_*engtype_VideoDecode)\Running Time", std::process::id()));
        let (mut query, mut counter, mut size, mut count) = (0, 0, 0u32, 0u32);
        // SAFETY: a query of one counter, read twice (the first sizes the answer: 0x400 is
        // PDH_FMT_LARGE), and closed.
        unsafe {
            PdhOpenQueryW(std::ptr::null(), 0, &mut query);
            PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter);
            PdhCollectQueryData(query);
            PdhGetFormattedCounterArrayW(counter, 0x400, &mut size, &mut count, std::ptr::null_mut());
            let mut items: Vec<Item> = Vec::with_capacity(size as usize / std::mem::size_of::<Item>() + 1);
            let read = PdhGetFormattedCounterArrayW(counter, 0x400, &mut size, &mut count, items.as_mut_ptr());
            items.set_len(if read == 0 { count as usize } else { 0 });
            let total = items.iter().filter(|item| item.status == 0).map(|item| item.value).sum();
            PdhCloseQuery(query);
            (!items.is_empty()).then_some(total)
        }
    }

    #[test]
    #[cfg(windows)]
    fn frames_decoded_on_the_gpu_match_those_decoded_in_software_and_the_gpu_decodes_them() {
        let _alone = alone();
        let Some(file) = test_file(false) else { return };
        let times = [0.0, 1.2, 2.9];
        let software = frames_at(&mut Player::in_software(file.as_path().into()).unwrap(), times).unwrap();
        let before = video_decoder_time().unwrap_or(0);
        let gpu = frames_at(&mut player(Video::open_silent(file.as_path())).unwrap().player, times).unwrap();
        let used = video_decoder_time().map(|after| after - before);
        for ((at, gpu), software) in times.into_iter().zip(gpu).zip(software) {
            let apart = gpu.iter().zip(&software).map(|(a, b)| a.to_le_bytes().iter().zip(b.to_le_bytes()).map(|(a, b)| a.abs_diff(b)).max().unwrap_or(0)).max();
            assert_eq!(gpu.len(), software.len());
            assert!(apart <= Some(2), "at {at} s the GPU's frame is {apart:?} levels from software's");
            assert_eq!(number_in(&gpu, 64, (0, 0), 1.0), (at * 10.0).round() as usize);
        }
        let Some(used) = used else { return eprintln!("no GPU video decoder here: frames matched, the decoder skipped") };
        eprintln!("the GPU's video decoder ran {} µs for this process", used / 10);
        assert!(used > 0, "the GPU's video decoder never ran");
    }

    #[test]
    #[cfg(windows)]
    fn jpeg_frames_play_at_their_times_through_direct3d() {
        let _alone = alone();
        let jpeg = written("jpeg.mp4", &movie(30, 10, true, &[], &[]));
        // (Into a bitmap, Windows' own MJPEG decoder refuses them on some machines and takes
        // them on others: said, not checked.)
        let into_a_bitmap = frames_at(&mut Player::in_software(jpeg.as_path().into()).unwrap(), [0.0, 1.5, 2.9]).map(|_| "taken");
        let frames = match frames_at(&mut player(Video::open_silent(jpeg.as_path())).unwrap().player, [0.0, 1.5, 2.9]) {
            Ok(frames) => frames,
            Err(why) => return eprintln!("no GPU here takes JPEG frames ({why}): skipped"),
        };
        let numbers: Vec<usize> = frames.iter().map(|frame| number_in(frame, 64, (0, 0), 1.0)).collect();
        eprintln!("into a bitmap: {into_a_bitmap:?}");
        assert_eq!(numbers, [0, 15, 29]);
    }

    #[test]
    #[cfg(windows)]
    fn hevc_vp9_and_av1_play_where_the_gpu_or_an_extension_decodes_them() {
        let _alone = alone();
        let Some(folder) = std::env::var_os("TAUT_VIDEO_FILES").map(PathBuf::from) else {
            return eprintln!("TAUT_VIDEO_FILES names no folder of hevc.mp4, vp9.webm, av1.mp4 (made by ffmpeg): skipped");
        };
        for name in ["hevc.mp4", "vp9.webm", "av1.mp4"] {
            let mut video = player(Video::open_silent(folder.join(name))).unwrap();
            let before = video_decoder_time().unwrap_or(0);
            let start = std::time::Instant::now();
            let mut pixels = Vec::new();
            let mut error = None;
            while start.elapsed() < Duration::from_secs(5) && error.is_none() && video.player.frame(&mut pixels).is_none() {
                std::thread::sleep(Duration::from_millis(20));
                error = video.player.news().error;
            }
            let used = video_decoder_time().map_or(0, |after| after - before);
            let shown = (pixels.len(), error);
            eprintln!("{name}: {} pixels, error {:?}, the GPU's video decoder ran {} µs", shown.0, shown.1, used / 10);
            assert!(shown.0 == 640 * 480 || shown.1.is_some(), "{name}: neither a frame nor an error");
        }
    }
}
