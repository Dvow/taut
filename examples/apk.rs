//! Packs an example as an Android app (an APK) with the SDK's own tools, signed with a debug
//! key, ready for `adb install`:
//!
//!   cargo run --example apk -- gallery --target x86_64-linux-android --features touch
//!
//! Needs `ANDROID_HOME` (with `build-tools` and a platform), the NDK (`ANDROID_NDK_HOME`, else
//! the newest in `$ANDROID_HOME/ndk`) and a JDK (`keytool`, and Java for `apksigner`). The
//! steps, each a program the SDK has:
//!
//! 1. The example is built as a shared library, stripped: a crate of its own in
//!    `target/apk/<name>`, whose library is the example's file, linked by the NDK's `clang`. (On Android an app is a
//!    library the system's `NativeActivity` loads; the example's `android_main` starts it.)
//! 2. `aapt2 link` makes the package from a manifest: `NativeActivity`, the library's name,
//!    no Java (`hasCode="false"`), every configuration change kept in the running app.
//! 3. `aapt add` puts the library in it, as `lib/<abi>/lib<name>.so`.
//! 4. `zipalign`, then `apksigner` with `target/apk/debug.keystore` (made once by `keytool`).
//!
//! Another file than an example's is packed the same, named by its stem: `apk -- tests/android.rs`.
//!
//! The APK is `target/apk/<name>.apk` (or `--out`). Targets: `aarch64-linux-android`
//! (phones), `x86_64-linux-android` (the emulator); the default is the first.
use std::path::{Path, PathBuf};
use std::process::Command;

/// Android 11: the oldest system the library runs on (window insets by type).
const MIN_SDK: u32 = 30;
const TARGET_SDK: u32 = 35;

/// What was asked on the command line.
struct Asked {
    example: String,
    /// The file to pack, where not `examples/<example>.rs`.
    source: Option<PathBuf>,
    target: String,
    features: Vec<String>,
    out: Option<PathBuf>,
}

fn main() {
    let asked = asked(std::env::args().skip(1).collect());
    let sdk = PathBuf::from(std::env::var_os("ANDROID_HOME").or_else(|| std::env::var_os("ANDROID_SDK_ROOT")).unwrap_or_else(|| fail("ANDROID_HOME is not set")));
    let ndk = std::env::var_os("ANDROID_NDK_HOME").map(PathBuf::from).or_else(|| newest(&sdk.join("ndk"))).unwrap_or_else(|| fail("no NDK: set ANDROID_NDK_HOME"));
    let tools = newest(&sdk.join("build-tools")).unwrap_or_else(|| fail("no build-tools in ANDROID_HOME"));
    let platform = newest(&sdk.join("platforms")).unwrap_or_else(|| fail("no platform in ANDROID_HOME")).join("android.jar");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let work = root.join("target").join("apk").join(&asked.example);
    let library = build(&asked, &root, &work, &ndk);
    let abi = if asked.target.starts_with("x86_64") { "x86_64" } else { "arm64-v8a" };
    std::fs::write(work.join("AndroidManifest.xml"), manifest(&asked.example)).unwrap_or_else(|_| fail("cannot write the manifest"));
    let unsigned = work.join("unaligned.apk");
    let jar = platform.to_string_lossy().into_owned();
    let (min, most) = (MIN_SDK.to_string(), TARGET_SDK.to_string());
    run(&tool(&tools, "aapt2"), &["link", "-o", &text(&unsigned), "--manifest", &text(&work.join("AndroidManifest.xml")), "-I", &jar, "--min-sdk-version", &min, "--target-sdk-version", &most], None);
    let inside = format!("lib/{abi}/lib{}.so", asked.example);
    std::fs::create_dir_all(work.join("lib").join(abi)).unwrap_or_else(|_| fail("cannot make the lib folder"));
    std::fs::copy(&library, work.join(&inside)).unwrap_or_else(|_| fail("cannot copy the library"));
    run(&tool(&tools, "aapt"), &["add", "-f", &text(&unsigned), &inside], Some(&work));
    let aligned = work.join("aligned.apk");
    run(&tool(&tools, "zipalign"), &["-f", "-p", "4", &text(&unsigned), &text(&aligned)], None);
    let keys = keystore(&root.join("target").join("apk"));
    let out = asked.out.clone().unwrap_or_else(|| root.join("target").join("apk").join(format!("{}.apk", asked.example)));
    let signing = ["sign", "--ks", &text(&keys), "--ks-pass", "pass:android", "--key-pass", "pass:android", "--out", &text(&out), &text(&aligned)];
    run(&tool(&tools, "apksigner"), &signing, None);
    println!("{}", out.display());
}

/// The command line: the example's name, `--target`, `--features`, `--out`.
fn asked(args: Vec<String>) -> Asked {
    let mut asked = Asked { example: String::new(), source: None, target: "aarch64-linux-android".into(), features: Vec::new(), out: None };
    let mut words = args.into_iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--target" => asked.target = words.next().unwrap_or_default(),
            "--features" => asked.features.extend(words.next().unwrap_or_default().split(',').filter(|one| !one.is_empty()).map(String::from)),
            "--out" => asked.out = words.next().map(PathBuf::from),
            _ if word.ends_with(".rs") => {
                asked.example = Path::new(&word).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
                asked.source = Some(PathBuf::from(word));
            }
            _ => asked.example = word,
        }
    }
    if asked.example.is_empty() {
        fail("say which example: cargo run --example apk -- gallery [--target x86_64-linux-android] [--features touch] [--out app.apk]");
    }
    asked
}

/// Builds the example as a shared library for the target: its path.
fn build(asked: &Asked, root: &Path, work: &Path, ndk: &Path) -> PathBuf {
    std::fs::create_dir_all(work).unwrap_or_else(|_| fail("cannot make target/apk"));
    let features: String = ["right-to-left", "brahmic", "touch", "emoji", "fallback-fonts", "cff", "color-profiles", "woff2", "translations", "gpu", "gpu-draw", "video"]
        .iter()
        .map(|name| format!("{name} = [\"taut/{name}\"]\n"))
        .collect();
    let source = asked.source.as_ref().map_or_else(|| root.join("examples").join(format!("{}.rs", asked.example)), |source| root.join(source));
    let crate_file = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[lib]\ncrate-type = [\"cdylib\"]\npath = {source:?}\n\n[dependencies]\ntaut = {{ path = {root:?} }}\n\n[features]\n{features}\n[profile.release]\nstrip = true\n\n[workspace]\n",
        name = asked.example,
        source = text(&source).replace('\\', "/"),
        root = text(root).replace('\\', "/"),
    );
    std::fs::write(work.join("Cargo.toml"), crate_file).unwrap_or_else(|_| fail("cannot write the crate"));
    let prebuilt = only_folder(&ndk.join("toolchains").join("llvm").join("prebuilt")).unwrap_or_else(|| fail("no NDK toolchain"));
    let linker = tool(&prebuilt.join("bin"), &format!("{}{MIN_SDK}-clang", asked.target));
    let targets = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    let variable = format!("CARGO_TARGET_{}_LINKER", asked.target.to_uppercase().replace('-', "_"));
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cargo.args(["build", "--release", "--lib", "--target", &asked.target, "--manifest-path", &text(&work.join("Cargo.toml")), "--target-dir", &text(&targets)]);
    cargo.args(["--features", &asked.features.join(",")]).env(variable, &linker);
    if !cargo.status().is_ok_and(|ended| ended.success()) {
        fail("the build failed");
    }
    targets.join(&asked.target).join("release").join(format!("lib{}.so", asked.example))
}

/// The app's manifest: one `NativeActivity`, loading the example's library.
fn manifest(name: &str) -> String {
    let changes = "orientation|screenSize|screenLayout|smallestScreenSize|keyboard|keyboardHidden|navigation|density|uiMode|fontScale|locale|layoutDirection";
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="taut.example.{name}" android:versionCode="1" android:versionName="1.0">
    <application android:label="{name}" android:hasCode="false" android:extractNativeLibs="true">
        <activity android:name="android.app.NativeActivity" android:exported="true" android:configChanges="{changes}"
            android:windowSoftInputMode="adjustResize" android:theme="@android:style/Theme.DeviceDefault.NoActionBar">
            <meta-data android:name="android.app.lib_name" android:value="{name}" />
            <intent-filter>
                <action android:name="android.intent.action.MAIN" />
                <category android:name="android.intent.category.LAUNCHER" />
            </intent-filter>
        </activity>
    </application>
</manifest>
"#
    )
}

/// The debug key: `debug.keystore` in `folder`, made by `keytool` the first time.
fn keystore(folder: &Path) -> PathBuf {
    let keys = folder.join("debug.keystore");
    if keys.exists() {
        return keys;
    }
    let java = std::env::var_os("JAVA_HOME").map(|home| PathBuf::from(home).join("bin"));
    let keytool = java.map_or_else(|| PathBuf::from("keytool"), |bin| tool(&bin, "keytool"));
    let made = ["-genkeypair", "-keystore", &text(&keys), "-alias", "androiddebugkey", "-storepass", "android", "-keypass", "android", "-keyalg", "RSA", "-validity", "10000"];
    run(&keytool, &[&made[..], &["-dname", "CN=Android Debug,O=Android,C=US"]].concat(), None);
    keys
}

/// A tool in a folder, as this system names it (`aapt2.exe`, `apksigner.bat`, `clang.cmd`).
fn tool(folder: &Path, name: &str) -> PathBuf {
    let ends: &[&str] = if cfg!(windows) { &[".exe", ".bat", ".cmd"] } else { &[""] };
    let named = ends.iter().map(|end| folder.join(format!("{name}{end}"))).find(|path| path.is_file());
    named.unwrap_or_else(|| fail(&format!("no {name} in {}", folder.display())))
}

/// Runs a tool; stops if it fails.
fn run(program: &Path, args: &[&str], within: Option<&Path>) {
    let mut command = Command::new(program);
    command.args(args);
    within.into_iter().for_each(|folder| _ = command.current_dir(folder));
    if !command.status().is_ok_and(|ended| ended.success()) {
        fail(&format!("{} failed", program.display()));
    }
}

/// The newest folder in a folder (by name: versions sort so).
fn newest(folder: &Path) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(folder).ok()?.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    found.sort_by_key(|path| path.file_name().map(|name| name.to_string_lossy().split(['.', '-']).map(|part| part.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>()));
    found.pop()
}

/// The one folder in a folder (the NDK's host toolchain).
fn only_folder(folder: &Path) -> Option<PathBuf> {
    std::fs::read_dir(folder).ok()?.filter_map(Result::ok).map(|entry| entry.path()).find(|path| path.is_dir())
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn fail(why: &str) -> ! {
    eprintln!("apk: {why}");
    std::process::exit(1)
}
