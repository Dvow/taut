//! Videos on a Mac, on the main thread: AVFoundation plays only while that thread's run loop
//! turns, and libtest runs no test there. (Elsewhere the unit tests of `src/video.rs` do this.)

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    checks::all();
    println!("video on macOS: every check passed");
}

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod files {
    include!("video/files.rs");
}

#[cfg(target_os = "macos")]
mod checks {
    use super::files::*;
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use taut::testing::Trial;
    use taut::{Role, Video};

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
        trial.run();
        let start = Instant::now();
        while !done(&trial.app) {
            assert!(start.elapsed() < Duration::from_secs(15), "{what} did not happen: {:?}", trial.app);
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
        the_end_and_a_loop(&with_sound);
        println!("the end, and a loop");
        the_controls(&silent);
        println!("the controls");
        let mut bytes = trial(Video::open_silent(std::fs::read(&silent).unwrap()).unwrap(), false);
        until(&mut bytes, "the first frame", |video| video.size().is_some());
        assert_eq!(seek(&mut bytes, Duration::from_millis(1200)), 12);
        println!("bytes");
        many_opens(&with_sound);
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

    /// Opens, plays and drops videos: what the process holds stays put.
    fn many_opens(file: &std::path::Path) {
        let held = || {
            let said = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
            String::from_utf8_lossy(&said.stdout).trim().parse::<usize>().unwrap_or(0) * 1024
        };
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
