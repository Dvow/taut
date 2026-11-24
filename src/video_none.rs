//! Video where taut has no player yet (iOS, the web): opening says so.

use crate::video::{News, VideoError, VideoSource};
use std::time::Duration;

/// No player. Never made: `open` refuses.
pub(crate) struct Player(());

impl Player {
    /// Refuses: this system has no player taut can use yet.
    pub(crate) fn open(source: VideoSource, sound: bool) -> Result<Player, VideoError> {
        let _ = (source, sound, Player(()));
        Err(VideoError::NoPlayer("taut has no video player on this system yet".into()))
    }
    pub(crate) fn play(&mut self) {}
    pub(crate) fn pause(&mut self) {}
    pub(crate) fn seek(&mut self, to: Duration) {
        let _ = to;
    }
    pub(crate) fn position(&mut self) -> Duration {
        Duration::ZERO
    }
    pub(crate) fn length(&mut self) -> Option<Duration> {
        None
    }
    pub(crate) fn set_volume(&mut self, volume: f32) {
        let _ = volume;
    }
    pub(crate) fn set_muted(&mut self, muted: bool) {
        let _ = muted;
    }
    pub(crate) fn news(&mut self) -> News {
        News { ended: false, error: None }
    }
    pub(crate) fn frame(&mut self, into: &mut Vec<u32>) -> Option<(usize, Duration)> {
        let _ = into;
        None
    }
}
