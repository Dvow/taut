//! Videos on a Mac and on iOS, on the main thread: AVFoundation plays only while that thread's
//! run loop turns, and libtest runs no test there. (Elsewhere the unit tests of `src/video.rs`
//! do this.) One file, two programs:
//!
//! - On a Mac it runs the checks; with `TAUT_IOS_SIMULATOR=1` it also builds itself for the
//!   iOS simulator as an app (`examples/ios_bundle.rs`), boots the newest iPhone simulator and
//!   runs it there.
//! - On iOS it is that app: the checks run inside UIKit's run loop (from the first frame of
//!   `taut::run`), then it says how they went and quits.

#[cfg(not(target_vendor = "apple"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    checks::all();
    println!("video on macOS: every check passed");
    simulator::run();
}

#[cfg(target_os = "ios")]
fn main() {
    let mut checked = false;
    taut::run(taut::Window::default(), move |_| {
        if !std::mem::replace(&mut checked, true) {
            checks::all();
            println!("video on iOS: every check passed");
            std::process::exit(0);
        }
    });
}

#[cfg(target_vendor = "apple")]
#[allow(dead_code)]
mod files {
    include!("video/files.rs");
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../examples/ios_bundle.rs"]
mod ios_bundle;

#[cfg(target_vendor = "apple")]
mod checks {
    use super::files::*;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use taut::testing::Trial;
    use taut::{Role, Track, Video};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, once: u8) -> i32;
    }

    /// Lets the main run loop turn this long.
    fn turn(time: Duration) {
        // SAFETY: the main thread's own run loop, in its default mode.
        unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, time.as_secs_f64(), 0) };
    }

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
    fn pass(trial: &mut Trial<Video>, time: Duration) {
        turn(time);
        trial.wait(time);
    }
    fn until(trial: &mut Trial<Video>, what: &str, done: impl Fn(&Video) -> bool) {
        until_within(trial, what, Duration::from_secs(15), done);
    }
    fn until_within(trial: &mut Trial<Video>, what: &str, limit: Duration, done: impl Fn(&Video) -> bool) {
        trial.run();
        let start = Instant::now();
        while !done(&trial.app) {
            assert!(start.elapsed() < limit, "{what} did not happen: {:?}", trial.app);
            pass(trial, Duration::from_millis(20));
        }
    }
    fn shown(trial: &Trial<Video>) -> usize {
        number_in(&trial.screen.pixels, trial.screen.width, (0, 0), 1.0)
    }
    /// Seeks, and waits for the frame there.
    fn seek(trial: &mut Trial<Video>, to: Duration) -> usize {
        let before = trial.app.frames_shown();
        trial.app.seek(to);
        until(trial, "the frame after a seek", |video| video.frames_shown() > before);
        pass(trial, Duration::from_millis(100));
        shown(trial)
    }

    pub fn all() {
        let silent = written("silent.mp4", &mp4(30, 10, false));
        let with_sound = written("with-sound.mp4", &mp4(30, 10, true));
        frames_show_at_their_times(&with_sound);
        println!("frames show at their times");
        a_paused_video_holds_and_seeks(&silent);
        println!("pause and seek");
        a_seek_before_the_first_frame(&silent);
        println!("a seek before the first frame");
        the_end_and_a_loop(&with_sound);
        println!("the end, and a loop");
        the_controls(&silent);
        println!("the controls");
        let mut bytes = trial(Video::open_silent(std::fs::read(&silent).unwrap()).unwrap(), false);
        until(&mut bytes, "the first frame", |video| video.size().is_some());
        assert_eq!(seek(&mut bytes, Duration::from_millis(1200)), 12);
        println!("bytes");
        tracks_captions_and_speed();
        println!("tracks, captions and speed");
        captions_in_the_video_stream();
        println!("captions in the video stream");
        for _ in 0..std::env::var("TAUT_VIDEO_REPEAT").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0) {
            captions_in_the_video_stream();
            println!("captions in the video stream, again");
        }
        other_cea608_channels();
        println!("CEA-608's other channels");
        many_opens(&with_sound);
    }

    /// CEA-608's other channels, in the SEI and in a QuickTime closed-caption track: a track
    /// each, shown at their times.
    fn other_cea608_channels() {
        for (carried, name) in [(Carried::InSei, "other-channels.mp4"), (Carried::InCaptionTrack, "other-channels.mov")] {
            let file = written(name, &movie_captioned_in_stream(30, Coding::H264, &OTHER_CHANNELS, carried));
            let mut trial = trial(Video::open_silent(file).unwrap(), false);
            until(&mut trial, "the first frame", |video| video.size().is_some());
            trial.app.play();
            // (The simulator's reader is slow: see `captions_in_the_video_stream`.)
            let reader = Duration::from_secs(60);
            until_within(&mut trial, "three caption tracks", reader, |video| video.caption_tracks().len() >= 3);
            let names: Vec<&str> = trial.app.caption_tracks().iter().map(|track| track.name.as_str()).collect();
            assert_eq!(names, ["CC2", "CC3", "CC4"], "{name}");
            trial.app.pause();
            for (at, track, caption) in [(900, "CC2", "Second channel"), (1300, "CC3", "Third channel"), (2400, "CC4", "Fourth channel")] {
                trial.app.set_caption_track(trial.app.caption_tracks().iter().position(|found| found.name == track));
                let there = Duration::from_millis(at);
                trial.app.seek(there);
                until_within(&mut trial, "the caption", reader, |video| video.position().abs_diff(there) < Duration::from_millis(50) && video.caption() == Some(caption));
            }
        }
    }

    /// The first two letters of each track's language.
    fn languages(tracks: &[Track]) -> Vec<String> {
        tracks.iter().map(|track| track.language.chars().take(2).collect()).collect()
    }

    fn tracks_captions_and_speed() {
        let file = written("languages.mp4", &movie(30, 10, false, &TWO_LANGUAGES, &TWO_CAPTIONS));
        let mut trial = trial(Video::open_silent(file).unwrap(), false);
        until(&mut trial, "the tracks and the first frame", |video| video.sound_tracks().len() == 2 && video.size().is_some());
        assert_eq!((languages(trial.app.sound_tracks()), languages(trial.app.caption_tracks())), (vec!["en".to_string(), "de".into()], vec!["en".to_string(), "fr".into()]));
        trial.app.set_sound_track(1);
        assert_eq!(trial.app.sound_track(), Some(1));
        trial.app.set_caption_track(Some(1));
        for (to, text) in [(500, Some("Français un")), (1500, Some("Français deux")), (2200, None), (2700, Some("Français fin"))] {
            seek(&mut trial, Duration::from_millis(to));
            assert_eq!(trial.app.caption(), text, "at {to} ms");
        }
        trial.app.set_speed(2.0);
        seek(&mut trial, Duration::ZERO);
        trial.app.play();
        until(&mut trial, "playing", |video| video.position() >= Duration::from_millis(200));
        let (start, from) = (Instant::now(), trial.app.position());
        pass(&mut trial, Duration::from_millis(600));
        let rate = (trial.app.position() - from).as_secs_f32() / start.elapsed().as_secs_f32();
        assert!((rate - 2.0).abs() < 0.4, "at 2× the time moved {rate} s a second");
    }

    /// CEA-608 and CEA-708 in H.264's and HEVC's SEI (read by taut), and CEA-608 in a QuickTime
    /// closed-caption track: tracks shown at their times; the HEVC file's frames as made.
    fn captions_in_the_video_stream() {
        let files = [(Coding::H264, Carried::InSei, "in-stream.mp4"), (Coding::Hevc, Carried::InSei, "in-stream-hevc.mp4"), (Coding::H264, Carried::InCaptionTrack, "closed-captions.mov")];
        for (coding, carried, name) in files {
            let file = written(name, &movie_captioned_in_stream(30, coding, &IN_STREAM, carried));
            let mut trial = trial(Video::open_silent(file).unwrap(), false);
            until(&mut trial, "the first frame", |video| video.size().is_some());
            assert_eq!(shown(&trial), 0, "{name}");
            trial.app.play();
            // (The simulator's reader takes seconds to start and once stalled 25 s mid-file
            // (`copyNextSampleBuffer`): the tracks may come once the captions played, or the
            // video ended. CC1's second caption, once read, shows at 2.5 s and has ended at
            // 2.65 s once all checked below was read; then from the start.)
            let reader = Duration::from_secs(60);
            let wanted: &[&str] = if carried == Carried::InSei { &["CC1", "Service 1"] } else { &["CC1"] };
            until_within(&mut trial, "the caption tracks", reader, |video| video.caption_tracks().len() >= wanted.len());
            let names: Vec<&str> = trial.app.caption_tracks().iter().map(|track| track.name.as_str()).collect();
            assert_eq!(names, wanted, "{name}");
            trial.app.pause();
            trial.app.set_caption_track(Some(0));
            for (at, shown) in [(2500, Some("Second line")), (2650, None)] {
                let there = Duration::from_millis(at);
                trial.app.seek(there);
                until_within(&mut trial, "the captions read", reader, |video| video.position().abs_diff(there) < Duration::from_millis(50) && video.caption() == shown);
            }
            trial.app.seek(Duration::ZERO);
            trial.app.play();
            until(&mut trial, "the seek back", |video| video.position() < Duration::from_millis(500));
            for (at, track, caption) in IN_STREAM_SHOWN.into_iter().filter(|(_, track, _)| wanted.contains(track)) {
                until(&mut trial, "the time", |video| video.position() >= Duration::from_secs_f32(at));
                trial.app.set_caption_track(trial.app.caption_tracks().iter().position(|found| found.name == track));
                trial.run();
                assert_eq!(trial.app.caption(), caption, "{name}, {track} at {:?}", trial.app.position());
            }
        }
    }

    fn frames_show_at_their_times(file: &std::path::Path) {
        let mut trial = trial(Video::open_silent(file).unwrap(), false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        assert_eq!((trial.app.size(), shown(&trial)), (Some((64, 48)), 0));
        assert!(trial.app.length().is_some_and(|length| (length.as_secs_f32() - 3.0).abs() < 0.05), "{:?}", trial.app.length());
        trial.app.play();
        trial.run();
        let mut seen = Vec::new();
        while trial.app.playing() && seen.len() < 100 {
            pass(&mut trial, Duration::from_millis(50));
            let (number, at) = (shown(&trial), trial.app.position().as_secs_f32());
            let start = number as f32 / 10.0;
            assert!(at > start - 0.08 && at < start + 0.18, "frame {number} shown at {at} s");
            seen.push(number);
        }
        assert!(seen.windows(2).all(|two| two[0] <= two[1]) && seen.last() >= Some(&25), "frames shown: {seen:?}");
    }

    fn a_paused_video_holds_and_seeks(file: &std::path::Path) {
        let mut trial = trial(Video::open_silent(file).unwrap(), false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.play();
        until(&mut trial, "a second played", |video| video.position() >= Duration::from_secs(1));
        trial.app.pause();
        pass(&mut trial, Duration::from_millis(200));
        let (held, at) = (shown(&trial), trial.app.position());
        pass(&mut trial, Duration::from_millis(400));
        assert_eq!((shown(&trial), trial.app.position()), (held, at));
        for (to, frame) in [(2.0, 20), (0.5, 5), (2.9, 29)] {
            assert_eq!(seek(&mut trial, Duration::from_secs_f32(to)), frame, "after a seek to {to} s");
        }
    }

    /// A seek asked before the video is ready: the first frame shown is the one there, none on
    /// the way to it.
    fn a_seek_before_the_first_frame(file: &std::path::Path) {
        let mut video = Video::open_silent(file).unwrap();
        video.seek(Duration::from_millis(1200));
        let mut trial = trial(video, false);
        until(&mut trial, "the first frame", |video| video.frames_shown() > 0);
        assert_eq!(shown(&trial), 12, "the first frame shown");
    }

    fn the_end_and_a_loop(file: &std::path::Path) {
        let mut trial = trial(Video::open_silent(file).unwrap(), false);
        until(&mut trial, "the first frame", |video| video.size().is_some());
        trial.app.seek(Duration::from_millis(2500));
        trial.app.play();
        until(&mut trial, "the end", |video| !video.playing());
        assert!(trial.app.ended() && !trial.app.ended(), "the end is told once");
        assert_eq!(shown(&trial), 29);
        trial.app.set_looping(true);
        trial.app.play();
        until(&mut trial, "playing from the start", |video| video.position() < Duration::from_millis(500));
        pass(&mut trial, Duration::from_millis(300));
        assert!(trial.app.playing() && !trial.app.ended() && shown(&trial) < 10, "looped: {:?}", trial.app);
    }

    fn the_controls(file: &std::path::Path) {
        let mut trial = trial(Video::open_silent(file).unwrap(), true);
        until(&mut trial, "the first frame", |video| video.size().is_some() && video.length().is_some());
        trial.run();
        for (role, name) in [(Role::Button, "Play"), (Role::Slider, "Position"), (Role::Slider, "Volume")] {
            assert!(trial.has(role, name), "{name}:\n{}", trial.dump());
        }
        trial.press(Role::Button, "Play");
        assert!(trial.app.playing() && trial.has(Role::Button, "Pause"));
        trial.press(Role::Button, "Pause");
        let before = trial.app.frames_shown();
        trial.key("Tab");
        trial.key("End");
        until(&mut trial, "the frame after End", |video| video.frames_shown() > before);
        pass(&mut trial, Duration::from_millis(100));
        assert_eq!(shown(&trial), 29);
    }

    /// The memory the process holds (Mach's resident size).
    fn held() -> usize {
        /// `mach_task_basic_info`: sizes, then times and the rest.
        #[repr(C)]
        #[derive(Default)]
        struct BasicInfo {
            virtual_size: u64,
            resident_size: u64,
            resident_most: u64,
            rest: [i32; 6],
        }
        unsafe extern "C" {
            static mach_task_self_: u32;
            fn task_info(task: u32, flavor: i32, info: *mut BasicInfo, count: *mut u32) -> i32;
        }
        let (mut info, mut count) = (BasicInfo::default(), (size_of::<BasicInfo>() / 4) as u32);
        // SAFETY: this task's own port; the info is as large as `count` says. 20 is
        // `MACH_TASK_BASIC_INFO`.
        unsafe { task_info(mach_task_self_, 20, &mut info, &mut count) };
        info.resident_size as usize
    }

    /// Opens, plays and drops videos: what the process holds stays put.
    fn many_opens(file: &std::path::Path) {
        let open_and_close = |times: usize| {
            for _ in 0..times {
                let mut trial = trial(Video::open_silent(file).unwrap(), false);
                until(&mut trial, "the first frame", |video| video.size().is_some());
                trial.app.play();
                pass(&mut trial, Duration::from_millis(100));
                let Trial { app, mut ui, .. } = trial;
                ui.drop_video(app);
            }
        };
        open_and_close(5);
        let before = held();
        open_and_close(30);
        let after = held();
        println!("many opens: held {before} bytes, after 30 more {after}");
        assert!(after < before + (10 << 20), "held {before} bytes, after 30 more {after}");
    }
}

/// The checks as an app in the newest iPhone simulator, with `TAUT_IOS_SIMULATOR=1`.
#[cfg(target_os = "macos")]
mod simulator {
    use super::ios_bundle;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    const ID: &str = "dev.taut.video-check";

    pub fn run() {
        if std::env::var_os("TAUT_IOS_SIMULATOR").is_none() {
            return println!("video on iOS runs with TAUT_IOS_SIMULATOR=1 (it boots a simulator)");
        }
        match checked() {
            Ok(said) => println!("{said}"),
            Err(why) => panic!("video on iOS:\n{why}"),
        }
    }

    /// Where the app is built: a folder of its own, so this test's own build is not waited for.
    fn built() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("video-ios")
    }

    /// This file built for the simulator, as a signed app.
    fn app() -> Result<PathBuf, String> {
        let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cargo.args(["build", "--test", "video_apple", "--features", "video", "--target", "aarch64-apple-ios-sim", "--message-format=json"]).env("CARGO_TARGET_DIR", built());
        let said = ios_bundle::ran(&mut cargo)?;
        let line = said.lines().find(|line| line.contains("\"executable\":\"") && line.contains("\"name\":\"video_apple\""));
        let program = line.and_then(|line| line.split("\"executable\":\"").nth(1)?.split('"').next()).ok_or("no test program built")?;
        let app = ios_bundle::bundle(Path::new(program), "video_check", ID, false, &built().join("ios")).map_err(|error| error.to_string())?;
        ios_bundle::sign_for_simulator(&app)?;
        Ok(app)
    }

    /// Runs the app on the newest iPhone simulator and waits for its verdict: what it said, or
    /// why it failed. A simulator booted here is shut down after: the steps after this one
    /// time frames on the Mac.
    fn checked() -> Result<String, String> {
        let app = app()?;
        let device = ios_bundle::newest_simulator("iPhone")?;
        let booted = ios_bundle::boot(&device)?;
        let verdict = verdict(&device, &app);
        let shut = if booted { ios_bundle::shut_down(&device) } else { Ok(()) };
        // (A failed check is said before a failed shutdown.)
        verdict.and_then(|said| shut.map(|()| said))
    }

    /// Installs and runs the app on `device`, and waits for its verdict.
    fn verdict(device: &str, app: &Path) -> Result<String, String> {
        ios_bundle::ran(Command::new("xcrun").args(["simctl", "install", device]).arg(app))?;
        let (out, err) = (built().join("video_check.out"), built().join("video_check.err"));
        let _ = (std::fs::remove_file(&out), std::fs::remove_file(&err));
        let logs = [format!("--stdout={}", out.display()), format!("--stderr={}", err.display())];
        ios_bundle::ran(Command::new("xcrun").args(["simctl", "launch", "--terminate-running-process"]).args(&logs).args([device, ID]))?;
        let since = Instant::now();
        loop {
            let said = std::fs::read_to_string(&out).unwrap_or_default();
            if said.contains("every check passed") {
                return Ok(said);
            }
            let failed = std::fs::read_to_string(&err).unwrap_or_default();
            if failed.contains("panicked") || since.elapsed() > Duration::from_secs(1200) {
                return Err(format!("{said}\n{failed}"));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
