//! Checks built for an x86-64 Android emulator, pushed with `adb` and run there as command-line
//! programs. Needs the NDK (`ANDROID_NDK_HOME`, else the newest in `$ANDROID_HOME/ndk`) and an
//! emulator that `adb` reaches.
use std::path::{Path, PathBuf};
use std::process::Command;

pub const TARGET: &str = "x86_64-linux-android";
/// Where the checks go on the emulator.
const THERE: &str = "/data/local/tmp";

/// `adb`, from the SDK or the path, where it reaches a device.
pub fn adb() -> Option<PathBuf> {
    let tools = std::env::var_os("ANDROID_HOME").map(|home| PathBuf::from(home).join("platform-tools").join("adb"));
    let adb = tools.filter(|path| path.exists()).unwrap_or_else(|| "adb".into());
    let state = Command::new(&adb).arg("get-state").output().ok()?;
    (state.status.success() && String::from_utf8_lossy(&state.stdout).trim() == "device").then_some(adb)
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
    let pushed = Command::new(adb).arg("push").arg(program).arg(format!("{THERE}/{name}")).output().expect("adb runs");
    assert!(pushed.status.success(), "adb push: {}", String::from_utf8_lossy(&pushed.stderr));
    let run = format!("cd {THERE} && chmod +x {name} && {variables} ./{name} {arguments} 2>&1; echo \"exit $?\"");
    let done = Command::new(adb).args(["shell", &run]).output().expect("adb runs");
    let said = String::from_utf8_lossy(&done.stdout).into_owned();
    let passed = said.contains("test result: ok.") && !said.contains(" 0 passed") && said.trim_end().ends_with("exit 0");
    (said, passed)
}
