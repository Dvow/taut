//! Builds an example as an iOS app (`.app` bundle) on a Mac with Xcode, with no Xcode project,
//! and runs it in the simulator:
//!
//! ```text
//! cargo run --example ios_bundle -- gallery --run
//! ```
//!
//! It builds the example for the simulator (`aarch64-apple-ios-sim`) with the feature `touch`,
//! puts it in `target/ios/<example>.app` beside an `Info.plist`, signs it for the simulator,
//! and with `--run` boots the newest iPhone simulator, installs the app there and starts it.
//! `--release` builds it optimized; `--features a,b` adds features.
//!
//! For a device: `--device --identity "Apple Development: ..." --profile app.mobileprovision`
//! builds for `aarch64-apple-ios` and signs with that identity, the profile embedded and its
//! entitlements (an Apple developer account makes both; the profile names the app's id, which
//! `--id` sets, `dev.taut.<example>` by default). Install it with Xcode's Devices window or
//! `xcrun devicectl device install app`.
//!
//! What the bundle needs, beside the program, is all in `Info.plist` (see `info_plist`): one
//! scene, every orientation, a launch screen (without one iOS shows the app letterboxed, at an
//! old phone's size), and pointer events from a trackpad or mouse.
use std::path::{Path, PathBuf};
use std::process::Command;

/// What to build and how.
#[derive(Debug, Default)]
pub struct Asked {
    /// The example's name.
    pub example: String,
    /// The bundle's id.
    pub id: String,
    /// For a device (else the simulator).
    pub device: bool,
    /// Optimized.
    pub release: bool,
    /// Features beside `touch`.
    pub features: String,
    /// Run it in the simulator.
    pub run: bool,
    /// Signing identity and provisioning profile, for a device.
    pub identity: Option<String>,
    pub profile: Option<PathBuf>,
}

/// The Rust target: the device's or the simulator's (Apple silicon).
pub fn target(device: bool) -> &'static str {
    if device { "aarch64-apple-ios" } else { "aarch64-apple-ios-sim" }
}

/// The bundle's `Info.plist`: program `name`, id `id`, for a device or the simulator.
pub fn info_plist(name: &str, id: &str, device: bool) -> String {
    let platform = if device { "iPhoneOS" } else { "iPhoneSimulator" };
    let orientations = "<array><string>UIInterfaceOrientationPortrait</string><string>UIInterfaceOrientationPortraitUpsideDown</string>\
        <string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string></array>";
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>{name}</string>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleName</key><string>{name}</string>
  <key>CFBundleDisplayName</key><string>{name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleSupportedPlatforms</key><array><string>{platform}</string></array>
  <key>LSRequiresIPhoneOS</key><true/>
  <key>MinimumOSVersion</key><string>14.0</string>
  <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
  <key>UILaunchScreen</key><dict/>
  <key>UIApplicationSupportsIndirectInputEvents</key><true/>
  <key>UIApplicationSceneManifest</key><dict><key>UIApplicationSupportsMultipleScenes</key><false/></dict>
  <key>UISupportedInterfaceOrientations</key>{orientations}
  <key>UISupportedInterfaceOrientations~ipad</key>{orientations}
</dict>
</plist>
"#
    )
}

/// Puts a built program in a bundle `<out>/<name>.app` with its `Info.plist`, and gives back the
/// bundle's path.
pub fn bundle(program: &Path, name: &str, id: &str, device: bool, out: &Path) -> std::io::Result<PathBuf> {
    let app = out.join(format!("{name}.app"));
    _ = std::fs::remove_dir_all(&app);
    std::fs::create_dir_all(&app)?;
    std::fs::copy(program, app.join(name))?;
    std::fs::write(app.join("Info.plist"), info_plist(name, id, device))?;
    Ok(app)
}

/// Runs a program, failing with its words if it fails.
pub fn ran(command: &mut Command) -> Result<String, String> {
    let done = command.output().map_err(|error| format!("{command:?}: {error}"))?;
    let said = String::from_utf8_lossy(&done.stdout).into_owned();
    if done.status.success() { Ok(said) } else { Err(format!("{command:?}: {}{said}", String::from_utf8_lossy(&done.stderr))) }
}

/// Signs a bundle for the simulator (with no identity: `-`).
pub fn sign_for_simulator(app: &Path) -> Result<(), String> {
    ran(Command::new("codesign").args(["--force", "--sign", "-"]).arg(app)).map(drop)
}

/// Signs a bundle for a device: the profile embedded, its entitlements claimed.
pub fn sign_for_device(app: &Path, identity: &str, profile: &Path) -> Result<(), String> {
    std::fs::copy(profile, app.join("embedded.mobileprovision")).map_err(|error| error.to_string())?;
    let decoded = ran(Command::new("security").args(["cms", "-D", "-i"]).arg(profile))?;
    let plist = app.with_extension("profile.plist");
    std::fs::write(&plist, decoded).map_err(|error| error.to_string())?;
    let entitlements = ran(Command::new("/usr/libexec/PlistBuddy").args(["-x", "-c", "Print :Entitlements"]).arg(&plist))?;
    let claimed = app.with_extension("entitlements");
    std::fs::write(&claimed, entitlements).map_err(|error| error.to_string())?;
    ran(Command::new("codesign").args(["--force", "--sign", identity, "--entitlements"]).arg(&claimed).arg(app)).map(drop)
}

/// The newest iPhone simulator: its id. (The last iOS runtime listed is the newest.)
pub fn newest_iphone() -> Result<String, String> {
    let listed = ran(Command::new("xcrun").args(["simctl", "list", "devices", "available"]))?;
    // Each runtime's first iPhone, the last runtime's kept.
    let (mut found, mut section): (Option<String>, Option<String>) = (None, None);
    for line in listed.lines() {
        if line.starts_with("--") {
            found = section.take().filter(|first| !first.is_empty()).or(found);
            section = line.contains("iOS").then(String::new);
            continue;
        }
        let id = line.trim().strip_prefix("iPhone").and_then(|rest| rest.split(['(', ')']).map(str::trim).find(|piece| piece.len() == 36 && piece.matches('-').count() == 4));
        if let (Some(id), Some(first)) = (id, section.as_mut().filter(|first| first.is_empty())) {
            *first = id.to_owned();
        }
    }
    let found = section.filter(|first| !first.is_empty()).or(found);
    found.ok_or_else(|| format!("no iPhone simulator in:\n{listed}"))
}

/// Boots a simulator, unless it is booted, and waits until it is ready.
pub fn boot(device: &str) -> Result<(), String> {
    _ = ran(Command::new("xcrun").args(["simctl", "boot", device]));
    ran(Command::new("xcrun").args(["simctl", "bootstatus", device, "-b"])).map(drop)
}

/// Builds the example as asked into a target folder, and gives back the bundle (in its `ios`).
pub fn build(asked: &Asked, built: &Path) -> Result<PathBuf, String> {
    let target = target(asked.device);
    let features = if asked.features.is_empty() { "touch".to_string() } else { format!("touch,{}", asked.features) };
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.args(["build", "--example", &asked.example, "--target", target, "--features", &features]).env("CARGO_TARGET_DIR", built);
    if asked.release {
        cargo.arg("--release");
    }
    let status = cargo.status().map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("the build failed".into());
    }
    let program = built.join(target).join(if asked.release { "release" } else { "debug" }).join("examples").join(&asked.example);
    let app = bundle(&program, &asked.example, &asked.id, asked.device, &built.join("ios")).map_err(|error| error.to_string())?;
    match (&asked.identity, &asked.profile, asked.device) {
        (Some(identity), Some(profile), true) => sign_for_device(&app, identity, profile)?,
        (.., true) => return Err("a device build needs --identity and --profile".into()),
        _ => sign_for_simulator(&app)?,
    }
    Ok(app)
}

/// Installs a bundle in the newest iPhone simulator and starts it, its output here.
pub fn run_in_simulator(app: &Path, id: &str) -> Result<(), String> {
    let device = newest_iphone()?;
    boot(&device)?;
    _ = Command::new("open").args(["-a", "Simulator"]).status();
    ran(Command::new("xcrun").args(["simctl", "install", &device]).arg(app))?;
    let status = Command::new("xcrun").args(["simctl", "launch", "--console-pty", "--terminate-running-process", &device, id]).status();
    status.map_err(|error| error.to_string()).map(drop)
}

/// What was asked on the command line.
fn asked() -> Result<Asked, String> {
    let mut asked = Asked::default();
    let mut words = std::env::args().skip(1);
    while let Some(word) = words.next() {
        match word.as_str() {
            "--device" => asked.device = true,
            "--release" => asked.release = true,
            "--run" => asked.run = true,
            "--features" => asked.features = words.next().unwrap_or_default(),
            "--id" => asked.id = words.next().unwrap_or_default(),
            "--identity" => asked.identity = words.next(),
            "--profile" => asked.profile = words.next().map(PathBuf::from),
            _ if !word.starts_with('-') && asked.example.is_empty() => asked.example = word,
            _ => return Err(format!("unknown: {word}")),
        }
    }
    if asked.example.is_empty() {
        return Err("name an example: cargo run --example ios_bundle -- gallery --run".into());
    }
    if asked.id.is_empty() {
        asked.id = format!("dev.taut.{}", asked.example.replace('_', "-"));
    }
    Ok(asked)
}

fn main() {
    if !cfg!(target_os = "macos") {
        return eprintln!("An iOS app is built on a Mac with Xcode.");
    }
    let done = asked().and_then(|asked| {
        let built = std::env::var_os("CARGO_TARGET_DIR").map_or(Path::new(env!("CARGO_MANIFEST_DIR")).join("target"), PathBuf::from);
        let app = build(&asked, &built)?;
        println!("{}", app.display());
        if asked.run && !asked.device { run_in_simulator(&app, &asked.id) } else { Ok(()) }
    });
    if let Err(error) = done {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
