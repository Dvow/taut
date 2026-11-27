//! The emulator from here, through `adb`: commands sent again whenever the connection dropped,
//! files copied whole; and checks built for an x86-64 emulator, pushed and run there as
//! command-line programs. The checks need the NDK (`ANDROID_NDK_HOME`, else the newest in
//! `$ANDROID_HOME/ndk`); everything needs an emulator that `adb` reaches.
#![allow(dead_code)] // each file of tests uses its own part of this
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

pub const TARGET: &str = "x86_64-linux-android";
/// Where the checks go on the emulator.
const THERE: &str = "/data/local/tmp";
/// How long the emulator may stay out of reach before a check gives up on it.
const OUT_OF_REACH: Duration = Duration::from_secs(120);

/// The SDK's `adb` (`$ANDROID_HOME/platform-tools`, `adb.exe` on Windows), else the path's.
pub fn program() -> PathBuf {
    let tools = std::env::var_os("ANDROID_HOME").map(|home| PathBuf::from(home).join("platform-tools").join("adb"));
    tools.filter(|path| path.exists() || path.with_extension("exe").exists()).unwrap_or_else(|| "adb".into())
}

/// `adb`, where it reaches a device.
pub fn adb() -> Option<PathBuf> {
    let adb = program();
    let state = Command::new(&adb).arg("get-state").output().ok()?;
    (state.status.success() && String::from_utf8_lossy(&state.stdout).trim() == "device").then_some(adb)
}

/// `adb` with `args`, run until it reaches the emulator. The emulator's connection drops now
/// and then (on a busy runner, after a large copy) and comes back: a command that never
/// reached it is sent again once it is back.
pub fn run(adb: &Path, args: &[&str]) -> Output {
    let since = Instant::now();
    loop {
        let done = Command::new(adb).args(args).output().expect("adb runs");
        let errors = String::from_utf8_lossy(&done.stderr);
        if !never_reached(&errors) {
            return done;
        }
        assert!(since.elapsed() < OUT_OF_REACH, "the emulator stayed out of reach: adb {args:?}: {errors}");
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// `adb` with `args`, as `run`: what it printed, and its errors.
pub fn said(adb: &Path, args: &[&str]) -> (String, String) {
    let done = run(adb, args);
    (String::from_utf8_lossy(&done.stdout).into_owned(), String::from_utf8_lossy(&done.stderr).into_owned())
}

/// Whether `adb`'s errors say it never reached the emulator, so the command did nothing there.
pub fn never_reached(errors: &str) -> bool {
    // (After "error: " from most commands, "adb: " or "adb.exe: " from installs.)
    let said = errors.lines().next().and_then(|line| line.split_once(": ")).map_or("", |(_, said)| said.trim_end());
    said == "device offline" || said == "no devices/emulators found" || (said.starts_with("device '") && said.ends_with("' not found"))
}

/// The emulator's file `path`, whole, by `pull`, again until it comes so. (What `exec-out`
/// prints can end early and still exit 0.)
pub fn read(adb: &Path, path: &str) -> Vec<u8> {
    let copy = std::env::temp_dir().join(format!("taut-adb-{}", std::process::id()));
    let since = Instant::now();
    while !said(adb, &["pull", path, &copy.to_string_lossy()]).1.contains("1 file pulled") {
        assert!(since.elapsed() < OUT_OF_REACH, "{path} was not copied");
        std::thread::sleep(Duration::from_secs(1));
    }
    let bytes = std::fs::read(&copy).unwrap_or_default();
    let _ = std::fs::remove_file(copy);
    bytes
}

/// The NDK's linker for the emulator's target, at the library's oldest Android (11).
pub fn linker() -> Option<PathBuf> {
    let ndk = std::env::var_os("ANDROID_NDK_HOME").map(PathBuf::from).or_else(|| {
        let ndks = PathBuf::from(std::env::var_os("ANDROID_HOME")?).join("ndk");
        std::fs::read_dir(ndks).ok()?.flatten().map(|entry| entry.path()).max()
    })?;
    let prebuilt = std::fs::read_dir(ndk.join("toolchains").join("llvm").join("prebuilt")).ok()?.flatten().next()?.path();
    let name = if cfg!(windows) { format!("{TARGET}30-clang.cmd") } else { format!("{TARGET}30-clang") };
    Some(prebuilt.join("bin").join(name)).filter(|linker| linker.exists())
}

/// A test program of the library built for the emulator, with `features`: the library's own
/// tests (`which` is `["--lib"]`) or a file of tests (`["--test", name]`). Built into a folder of
/// `name`'s own, so this test's build is not waited for.
pub fn built(linker: &Path, which: &[&str], features: &str, name: &str) -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let variable = format!("CARGO_TARGET_{}_LINKER", TARGET.to_uppercase().replace('-', "_"));
    let mut build = Command::new(cargo);
    build.current_dir(env!("CARGO_MANIFEST_DIR")).args(["test", "--no-run"]).args(which).args(["--features", features, "--target", TARGET, "--message-format=json"]);
    let made = build.env("CARGO_TARGET_DIR", &target).env(variable, linker).output().expect("cargo runs");
    assert!(made.status.success(), "the checks build for Android:\n{}", String::from_utf8_lossy(&made.stderr));
    let said = String::from_utf8_lossy(&made.stdout);
    let line = said.lines().find(|line| line.contains("\"test\":true") && line.contains("\"executable\":\""));
    let program = line.and_then(|line| line.split("\"executable\":\"").nth(1)?.split('"').next()).expect("a test program built");
    PathBuf::from(program.replace("\\\\", "\\"))
}

/// Pushes `program` to the emulator as `name` and runs it there with `arguments` and `variables`
/// (`NAME=value`, space apart); what it said, and whether all passed.
pub fn ran(adb: &Path, program: &Path, name: &str, variables: &str, arguments: &str) -> (String, bool) {
    let pushed = run(adb, &["push", &program.to_string_lossy(), &format!("{THERE}/{name}")]);
    assert!(pushed.status.success(), "adb push: {}", String::from_utf8_lossy(&pushed.stderr));
    let run = format!("cd {THERE} && chmod +x {name} && {variables} ./{name} {arguments} 2>&1; echo \"exit $?\"");
    let said = said(adb, &["shell", &run]).0;
    let passed = said.contains("test result: ok.") && !said.contains(" 0 passed") && said.trim_end().ends_with("exit 0");
    (said, passed)
}

#[test]
fn adb_errors_from_a_dropped_emulator_are_told_apart() {
    let lost = ["error: device offline\n", "error: no devices/emulators found\n", "error: device 'emulator-5554' not found\n", "adb.exe: device 'emulator-5554' not found\r\n"];
    for lost in lost {
        assert!(never_reached(lost), "{lost}");
    }
    for reached in ["", "All files should be loaded. Notifying the device.\n", "cat: /sdcard/x: No such file or directory\n", "error: closed\n"] {
        assert!(!never_reached(reached), "{reached}");
    }
}
