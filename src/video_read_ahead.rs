//! The captions in a video stream read by a thread of their own from the stream's samples as
//! stored (no decoder), a little ahead of where the video plays: for players that play a source
//! but hand over none of its captions (Media Foundation's engine, AVFoundation's player).

use crate::video_sei::{Codec, StreamCaptions};
use crate::video_stream_captions::ClosedCaptions;
use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// How far past where a video plays its captions are read.
const LEAD: Duration = Duration::from_secs(5);
/// How many access units deep a decoder may reorder frames (H.264's and HEVC's most).
const REORDER: usize = 16;

/// What the player's side and the reading thread share.
pub(crate) struct ReadAhead {
    state: Mutex<Pacing>,
    wake: Condvar,
}

struct Pacing {
    position: Duration,
    seek_to: Option<Duration>,
    stop: bool,
    captions: StreamCaptions,
    /// The times of the last units read, in decode order.
    recent: VecDeque<Duration>,
}

/// What the reading thread does next.
pub(crate) enum Step {
    Read,
    /// Reads from the key frame before this time.
    Seek(Duration),
    Stop,
}

impl ReadAhead {
    pub(crate) fn new() -> ReadAhead {
        let state = Pacing { position: Duration::ZERO, seek_to: None, stop: false, captions: StreamCaptions::default(), recent: VecDeque::new() };
        ReadAhead { state: Mutex::new(state), wake: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, Pacing> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What `read` makes of the tracks read so far.
    pub(crate) fn with_tracks<R>(&self, read: impl FnOnce(&mut ClosedCaptions) -> R) -> R {
        read(&mut self.lock().captions.tracks)
    }

    /// It plays at `position`: reading goes on to `LEAD` past it.
    pub(crate) fn played_to(&self, position: Duration) {
        self.lock().position = position;
        self.wake.notify_all();
    }

    /// It went to `to`: reading starts again there.
    pub(crate) fn seek(&self, to: Duration) {
        let mut state = self.lock();
        (state.position, state.seek_to) = (to, Some(to));
        drop(state);
        self.wake.notify_all();
    }

    /// The player is gone: the thread ends.
    pub(crate) fn stop(&self) {
        self.lock().stop = true;
        self.wake.notify_all();
    }

    /// The reading thread's next step, having read up to `read_to`: it waits while that is
    /// `LEAD` past where it plays.
    pub(crate) fn next(&self, read_to: Duration) -> Step {
        let mut state = self.lock();
        loop {
            if state.stop {
                return Step::Stop;
            }
            if let Some(to) = state.seek_to.take() {
                state.captions.sought(to);
                state.recent.clear();
                return Step::Seek(to);
            }
            if read_to <= state.position.saturating_add(LEAD) {
                return Step::Read;
            }
            state = self.wake.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Takes in an access unit of `codec` the thread read, shown at `at`, in decode order: its
    /// captions are read once `REORDER` units came after it (none of those can be shown before
    /// it).
    pub(crate) fn read(&self, access_unit: &[u8], codec: Codec, at: Duration) {
        let mut state = self.lock();
        state.captions.fed(access_unit, codec, at);
        state.recent.push_back(at);
        if state.recent.len() > REORDER {
            state.recent.pop_front();
            let earliest = state.recent.iter().min().copied().unwrap_or(at);
            state.captions.decoded(earliest.saturating_sub(Duration::from_nanos(1)));
        }
    }

    /// The thread read all there is: what waits is read.
    pub(crate) fn ended(&self) {
        self.lock().captions.decoded(Duration::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    mod files {
        include!("../tests/video/files.rs");
    }
    use files::{IN_STREAM, caption_sei, cc_data, length_prefixed};

    fn at(tenths: u64) -> Duration {
        Duration::from_millis(tenths * 100)
    }

    #[test]
    fn units_read_in_decode_order_give_their_captions_in_display_order() {
        // (Each I or P frame read before the two B-frames shown ahead of it.)
        let units: Vec<Vec<u8>> = cc_data(&IN_STREAM, 30).iter().map(|triplets| length_prefixed(caption_sei(triplets, false))).collect();
        let decode_order = std::iter::once(0).chain((1..30).step_by(3).flat_map(|first| [first + 2, first, first + 1])).filter(|frame| *frame < 30);
        let read_ahead = ReadAhead::new();
        for frame in decode_order {
            read_ahead.read(&units[frame], Codec::H264, at(frame as u64));
        }
        read_ahead.ended();
        let mut in_order = StreamCaptions::default();
        for (frame, unit) in (0..).zip(&units) {
            in_order.fed(unit, Codec::H264, at(frame));
            in_order.decoded(at(frame));
        }
        let texts = |tracks: &mut ClosedCaptions| (0..2).flat_map(|nth| (0..30).map(move |tenth| (nth, tenth))).map(|(nth, tenth)| tracks.text_at(nth, at(tenth))).collect::<Vec<_>>();
        assert_eq!(read_ahead.with_tracks(texts), texts(&mut in_order.tracks));
        assert!(read_ahead.with_tracks(|tracks| tracks.text_at(1, at(10))).is_some());
    }

    #[test]
    fn the_reading_thread_waits_until_the_video_plays_near_and_follows_a_seek() {
        let read_ahead = std::sync::Arc::new(ReadAhead::new());
        let (sender, steps) = std::sync::mpsc::channel();
        let thread = {
            let read_ahead = std::sync::Arc::clone(&read_ahead);
            std::thread::spawn(move || {
                for read_to in [Duration::ZERO, LEAD + at(1), LEAD + at(5)] {
                    let step = read_ahead.next(read_to);
                    sender
                        .send(match step {
                            Step::Read => "read".to_owned(),
                            Step::Seek(to) => format!("seek {to:?}"),
                            Step::Stop => "stop".into(),
                        })
                        .unwrap();
                }
            })
        };
        assert_eq!(steps.recv().unwrap(), "read");
        // (Past the lead: it waits for the video.)
        assert!(steps.recv_timeout(Duration::from_millis(200)).is_err());
        read_ahead.played_to(at(1));
        assert_eq!(steps.recv().unwrap(), "read");
        read_ahead.seek(at(40));
        assert_eq!(steps.recv().unwrap(), "seek 4s");
        thread.join().unwrap();
        read_ahead.stop();
        assert!(matches!(read_ahead.next(Duration::MAX), Step::Stop));
    }

    #[test]
    fn a_seek_to_the_latest_time_is_followed() {
        let read_ahead = ReadAhead::new();
        read_ahead.seek(Duration::MAX);
        assert!(matches!(read_ahead.next(Duration::ZERO), Step::Seek(Duration::MAX)));
        assert!(matches!(read_ahead.next(Duration::MAX), Step::Read));
    }
}
