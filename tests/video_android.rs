//! Videos on Android: the unit tests of `src/video.rs` (`video::tests`) built for an x86-64
//! emulator, pushed with `adb` and run there as a command-line program (the player needs no
//! app). Needs the NDK (`ANDROID_NDK_HOME`, else the newest in `$ANDROID_HOME/ndk`) and an
//! emulator that `adb` reaches; says why and passes without them, unless `TAUT_VIDEO=required`.
mod on_android;

#[test]
fn the_video_checks_pass_on_an_android_emulator() {
    let (Some(adb), Some(linker)) = (on_android::adb(), on_android::linker()) else {
        let required = std::env::var("TAUT_VIDEO").as_deref() == Ok("required");
        assert!(!required, "no emulator adb reaches, or no NDK");
        return eprintln!("no emulator adb reaches, or no NDK: skipped");
    };
    let checks = on_android::built(&linker, &["--lib"], "video", "video-android");
    let (said, passed) = on_android::ran(&adb, &checks, "taut-video-checks", "TAUT_VIDEO=required", "video::tests --test-threads=1 --nocapture");
    println!("{said}");
    assert!(passed, "the checks on the emulator failed");
}
