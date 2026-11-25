//! Videos on Android: the unit tests of `src/video.rs` (`video::tests`) built for an x86-64
//! emulator, pushed with `adb` and run there as a command-line program (the player needs no
//! app). Needs the NDK (`ANDROID_NDK_HOME`, else the newest in `$ANDROID_HOME/ndk`) and an
//! emulator that `adb` reaches; says why and passes without them, unless `TAUT_VIDEO=required`.
use std::path::{Path, PathBuf};
use std::process::Command;

const TARGET: &str = "x86_64-linux-android";
/// Where the checks go on the emulator.
const THERE: &str = "/data/local/tmp/taut-video-checks";

#[test]
fn the_video_checks_pass_on_an_android_emulator() {
    let (Some(adb), Some(linker)) = (adb(), linker()) else {
        let required = std::env::var("TAUT_VIDEO").as_deref() == Ok("required");
        assert!(!required, "no emulator adb reaches, or no NDK");
        return eprintln!("no emulator adb reaches, or no NDK: skipped");
    };
    let checks = built(&linker);
    let pushed = Command::new(&adb).arg("push").arg(&checks).arg(THERE).output().expect("adb runs");
    assert!(pushed.status.success(), "adb push: {}", String::from_utf8_lossy(&pushed.stderr));
    let run = format!("cd /data/local/tmp && TAUT_VIDEO=required {THERE} video::tests --test-threads=1 --nocapture 2>&1; echo \"exit $?\"");
    let ran = Command::new(&adb).args(["shell", &run]).output().expect("adb runs");
    let said = String::from_utf8_lossy(&ran.stdout);
    println!("{said}");
    assert!(said.contains("test result: ok.") && !said.contains(" 0 passed"), "the checks on the emulator failed");
}

/// `adb`, from the SDK or the path, where it reaches a device.
fn adb() -> Option<PathBuf> {
    let tools = std::env::var_os("ANDROID_HOME").map(|home| PathBuf::from(home).join("platform-tools").join("adb"));
    let adb = tools.filter(|path| path.exists()).unwrap_or_else(|| "adb".into());
    let state = Command::new(&adb).arg("get-state").output().ok()?;
    (state.status.success() && String::from_utf8_lossy(&state.stdout).trim() == "device").then_some(adb)
}

/// The NDK's linker for the emulator's target, at the library's oldest Android (11).
fn linker() -> Option<PathBuf> {
    let ndk = std::env::var_os("ANDROID_NDK_HOME").map(PathBuf::from).or_else(|| {
        let ndks = PathBuf::from(std::env::var_os("ANDROID_HOME")?).join("ndk");
        std::fs::read_dir(ndks).ok()?.flatten().map(|entry| entry.path()).max()
    })?;
    let prebuilt = std::fs::read_dir(ndk.join("toolchains").join("llvm").join("prebuilt")).ok()?.flatten().next()?.path();
    let name = if cfg!(windows) { format!("{TARGET}30-clang.cmd") } else { format!("{TARGET}30-clang") };
    Some(prebuilt.join("bin").join(name)).filter(|linker| linker.exists())
}

/// The library's unit tests built for the emulator, with the feature `video`.
fn built(linker: &Path) -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("video-android");
    let variable = format!("CARGO_TARGET_{}_LINKER", TARGET.to_uppercase().replace('-', "_"));
    let mut build = Command::new(cargo);
    build.current_dir(env!("CARGO_MANIFEST_DIR")).args(["test", "--no-run", "--lib", "--features", "video", "--target", TARGET, "--message-format=json"]);
    let made = build.env("CARGO_TARGET_DIR", &target).env(variable, linker).output().expect("cargo runs");
    assert!(made.status.success(), "the checks build for Android:\n{}", String::from_utf8_lossy(&made.stderr));
    let said = String::from_utf8_lossy(&made.stdout);
    let line = said.lines().find(|line| line.contains("\"test\":true") && line.contains("\"executable\":\""));
    let program = line.and_then(|line| line.split("\"executable\":\"").nth(1)?.split('"').next()).expect("a test program built");
    PathBuf::from(program.replace("\\\\", "\\"))
}
