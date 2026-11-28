//! The captions in a video stream read by a thread of their own from the stream's samples as
//! stored (no decoder), a little ahead of where the video plays: for players that play a source
//! but hand over none of its captions (Media Foundation's engine, AVFoundation's player).

use crate::video_sei::{Codec, StreamCaptions};
use crate::video_stream_captions::ClosedCaptions;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
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
    /// What the current reading covers: where it started, and the latest time it read (`MAX`
    /// once it read all).
    read: (Duration, Duration),
    /// The thread reads: started, not waiting for the video, not done.
    reading: bool,
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
        let state =
            Pacing { position: Duration::ZERO, seek_to: None, stop: false, captions: StreamCaptions::default(), recent: VecDeque::new(), read: (Duration::ZERO, Duration::ZERO), reading: false };
        ReadAhead { state: Mutex::new(state), wake: Condvar::new() }
    }

    /// Starts the reading thread, `read` reading with this; reading until it returns or waits.
    pub(crate) fn spawn(self: &Arc<Self>, read: impl FnOnce(&ReadAhead) + Send + 'static) {
        self.lock().reading = true;
        let shared = Arc::clone(self);
        std::thread::spawn(move || {
            read(&shared);
            shared.lock().reading = false;
        });
    }

    /// Whether the thread reads now: tracks may come though the video stands still.
    pub(crate) fn reading(&self) -> bool {
        self.lock().reading
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

    /// It went to `to`: reading starts again there, unless the captions there were read or soon
    /// will be (they stay: a player that seeks where it stands, as AVFoundation's does once ready,
    /// loses none, and a reader slow to start is not started twice).
    pub(crate) fn seek(&self, to: Duration) {
        let mut state = self.lock();
        let (from, reached) = state.read;
        let read_again = to < from || to > reached.saturating_add(LEAD);
        (state.position, state.seek_to) = (to, read_again.then_some(to));
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
                (state.recent, state.read, state.reading) = (VecDeque::new(), (to, to), true);
                return Step::Seek(to);
            }
            if read_to <= state.position.saturating_add(LEAD) {
                state.reading = true;
                return Step::Read;
            }
            state.reading = false;
            state = self.wake.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Takes in an access unit of `codec` the thread read, shown at `at`, in decode order, and
    /// decoded at `decode_time` where the source says: the captions shown before that are read
    /// now (no unit after it is decoded, so shown, sooner). Without it, they are read once
    /// `REORDER` units came after them.
    pub(crate) fn read(&self, access_unit: &[u8], codec: Codec, at: Duration, decode_time: Option<Duration>) {
        let mut state = self.lock();
        state.captions.fed(access_unit, codec, at);
        state.read.1 = state.read.1.max(at);
        let shown_from = decode_time.or_else(|| earliest_of_last(&mut state.recent, at));
        if let Some(time) = shown_from {
            state.captions.decoded(time.saturating_sub(Duration::from_nanos(1)));
        }
    }

    /// The thread read all there is: what waits is read.
    pub(crate) fn ended(&self) {
        let mut state = self.lock();
        state.captions.decoded(Duration::MAX);
        state.read.1 = Duration::MAX;
    }
}

/// With no decode times: the earliest time of the last `REORDER` units read (`at` the latest),
/// once more than that many came; no unit after them is shown before it.
fn earliest_of_last(recent: &mut VecDeque<Duration>, at: Duration) -> Option<Duration> {
    recent.push_back(at);
    if recent.len() <= REORDER {
        return None;
    }
    recent.pop_front();
    recent.iter().min().copied()
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
            read_ahead.read(&units[frame], Codec::H264, at(frame as u64), None);
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
    fn units_with_decode_times_give_their_captions_once_nothing_can_come_before_them() {
        // (B-frames as above; each unit decoded a frame's time after the one before.)
        let units: Vec<Vec<u8>> = cc_data(&IN_STREAM, 30).iter().map(|triplets| length_prefixed(caption_sei(triplets, false))).collect();
        let decode_order: Vec<usize> = std::iter::once(0).chain((1..30).step_by(3).flat_map(|first| [first + 2, first, first + 1])).filter(|frame| *frame < 30).collect();
        let read_ahead = ReadAhead::new();
        for (decoded, &frame) in decode_order.iter().enumerate() {
            read_ahead.read(&units[frame], Codec::H264, at(frame as u64), Some(at(decoded.saturating_sub(1) as u64)));
            // (CC1's first caption ends at 1.2 s: told once a unit decoded after it came, not 16
            // units later.)
            if decoded == 14 {
                assert_eq!(read_ahead.with_tracks(|tracks| (tracks.text_at(0, at(7)), tracks.text_at(0, at(12)))), (Some("Hello there".into()), None));
            }
        }
        let mut in_order = StreamCaptions::default();
        for (frame, unit) in (0..).zip(&units) {
            in_order.fed(unit, Codec::H264, at(frame));
            in_order.decoded(at(frame));
        }
        let texts = |tracks: &mut ClosedCaptions| (0..2).flat_map(|nth| (0..28).map(move |tenth| (nth, tenth))).map(|(nth, tenth)| tracks.text_at(nth, at(tenth))).collect::<Vec<_>>();
        assert_eq!(read_ahead.with_tracks(texts), texts(&mut in_order.tracks));
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
        read_ahead.seek(at(70));
        assert_eq!(steps.recv().unwrap(), "seek 7s");
        thread.join().unwrap();
        read_ahead.stop();
        assert!(matches!(read_ahead.next(Duration::MAX), Step::Stop));
    }

    #[test]
    fn a_seek_within_what_was_read_keeps_its_captions_and_one_out_of_it_reads_again() {
        let units: Vec<Vec<u8>> = cc_data(&IN_STREAM, 30).iter().map(|triplets| length_prefixed(caption_sei(triplets, false))).collect();
        let read_ahead = ReadAhead::new();
        assert!(matches!(read_ahead.next(Duration::ZERO), Step::Read));
        for (frame, unit) in (0..).zip(&units) {
            read_ahead.read(unit, Codec::H264, at(frame), None);
        }
        read_ahead.ended();
        let hello = |read_ahead: &ReadAhead| read_ahead.with_tracks(|tracks| tracks.text_at(0, at(7)));
        // (Where it stands, as a player once ready; back, within what was read.)
        for to in [Duration::ZERO, at(20), at(3)] {
            read_ahead.seek(to);
            assert!(read_ahead.lock().seek_to.is_none(), "a seek to {to:?} read again");
            assert_eq!(hello(&read_ahead).as_deref(), Some("Hello there"), "after a seek to {to:?}");
        }
        // (Past what was read and the lead: read again from there.)
        let read_ahead = ReadAhead::new();
        read_ahead.read(&units[0], Codec::H264, at(0), None);
        read_ahead.seek(at(70));
        assert!(matches!(read_ahead.next(Duration::ZERO), Step::Seek(to) if to == at(70)));
    }

    #[test]
    fn the_thread_is_told_reading_until_it_waits_for_the_video() {
        let read_ahead = std::sync::Arc::new(ReadAhead::new());
        assert!(!read_ahead.reading());
        let ((told, heard), (go, went)) = (std::sync::mpsc::channel(), std::sync::mpsc::channel());
        read_ahead.spawn(move |read_ahead| {
            assert!(matches!(read_ahead.next(Duration::ZERO), Step::Read));
            told.send(read_ahead.reading()).unwrap();
            went.recv().unwrap();
            // (Past the lead: it waits, until stopped.)
            assert!(matches!(read_ahead.next(LEAD + at(10)), Step::Stop));
        });
        assert!(heard.recv().unwrap(), "not reading while it reads");
        go.send(()).unwrap();
        let start = std::time::Instant::now();
        while read_ahead.reading() {
            assert!(start.elapsed() < Duration::from_secs(5), "still reading while it waits");
            std::thread::yield_now();
        }
        read_ahead.stop();
    }

    #[test]
    fn a_seek_to_the_latest_time_is_followed() {
        let read_ahead = ReadAhead::new();
        read_ahead.seek(Duration::MAX);
        assert!(matches!(read_ahead.next(Duration::ZERO), Step::Seek(Duration::MAX)));
        assert!(matches!(read_ahead.next(Duration::MAX), Step::Read));
    }
}
