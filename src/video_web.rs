//! Videos on the web: a `<video>` element the loader makes (`web.js`), each new frame drawn into
//! a canvas of the video's size and read back (`drawImage`, `getImageData`). Bytes play from a
//! Blob address; a path is an address relative to the page, or a picked file's bytes. Browsers
//! play sound only after a press on the page: a play refused for that is an error that clears
//! at the next play.

use crate::video::{News, VideoError, VideoSource, kind_of};
use crate::web_host::took;
use std::time::Duration;

#[link(wasm_import_module = "taut")]
unsafe extern "C" {
    fn video_open(source: *const u8, length: usize, kind: *const u8, kind_length: usize, bytes: u32, sound: u32) -> u32;
    fn video_close(id: u32);
    fn video_play(id: u32);
    fn video_pause(id: u32);
    fn video_seek(id: u32, seconds: f64);
    fn video_sound(id: u32, volume: f32, muted: u32);
    fn video_state(id: u32, state: *mut f64) -> usize;
    fn video_frame(id: u32, frame: *mut f64) -> usize;
}

/// A video element of the loader's, by its number.
pub(crate) struct Player {
    id: u32,
    position: Duration,
    length: Option<Duration>,
    volume: f32,
    muted: bool,
}

impl Player {
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let (text, bytes) = match source {
            VideoSource::Url(url) => (url.into_bytes(), false),
            VideoSource::File(path) => match crate::web_host::picked(&path) {
                Some(bytes) => (bytes, true),
                None => (path.to_string_lossy().replace('\\', "/").into_bytes(), false),
            },
            VideoSource::Bytes(bytes) => (bytes, true),
        };
        let kind = if bytes { mime(kind_of(&text)) } else { "" };
        // SAFETY: the loader reads both, copying the bytes into a Blob.
        let id = unsafe { video_open(text.as_ptr(), text.len(), kind.as_ptr(), kind.len(), bytes as u32, sound as u32) };
        match id {
            0 => Err(VideoError::NoPlayer("no page to play in (no document)".into())),
            id => Ok(Player { id, position: Duration::ZERO, length: None, volume: 1.0, muted: !sound }),
        }
    }

    pub(crate) fn play(&mut self) {
        // SAFETY: a plain request.
        unsafe { video_play(self.id) };
    }
    pub(crate) fn pause(&mut self) {
        // SAFETY: a plain request.
        unsafe { video_pause(self.id) };
    }
    pub(crate) fn seek(&mut self, to: Duration) {
        self.position = to;
        // SAFETY: a plain request.
        unsafe { video_seek(self.id, to.as_secs_f64()) };
    }
    pub(crate) fn position(&mut self) -> Duration {
        self.position
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        self.length
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        self.volume = volume;
        // SAFETY: a plain request.
        unsafe { video_sound(self.id, self.volume, self.muted as u32) };
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        // SAFETY: a plain request.
        unsafe { video_sound(self.id, self.volume, self.muted as u32) };
    }

    /// The element's time, length, end and error, as of now.
    pub(crate) fn news(&mut self) -> News {
        let mut state = [0f64; 4];
        // SAFETY: the loader writes four numbers, and answers the error's length.
        let told = unsafe { video_state(self.id, state.as_mut_ptr()) };
        let [at, length, ended, error_kind] = state;
        self.position = Duration::from_secs_f64(at.max(0.0));
        self.length = (length.is_finite() && length >= 0.0).then(|| Duration::from_secs_f64(length));
        let error = (error_kind != 0.0).then(|| String::from_utf8_lossy(&took(told)).into_owned());
        News { ended: ended != 0.0, error, clears_on_play: error_kind == 2.0 }
    }

    /// The frame on show, where it was not given yet: its width and time, its pixels in `into`.
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let mut said = [0f64; 3];
        // SAFETY: the loader writes three numbers, and answers the pixels' length.
        let length = unsafe { video_frame(self.id, said.as_mut_ptr()) };
        if length == 0 {
            return None;
        }
        let bytes = took(length);
        into.clear();
        into.extend(bytes.as_chunks::<4>().0.iter().map(|p| u32::from_le_bytes([p[2], p[1], p[0], 0xFF])));
        Some((said[0] as usize, Duration::from_secs_f64(said[2].max(0.0))))
    }
}

/// A Blob's type for a file kind.
fn mime(kind: &str) -> &'static str {
    match kind {
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "wav" => "audio/wav",
        "mkv" => "video/webm",
        "ogg" => "video/ogg",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "wmv" => "video/x-ms-wmv",
        _ => "video/mp4",
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        // SAFETY: the loader stops the element and frees its Blob.
        unsafe { video_close(self.id) };
    }
}
