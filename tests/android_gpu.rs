//! The GPU on Android, on an emulator that `adb` reaches, with the NDK at hand:
//!
//! - The library's GPU checks, built for the emulator and run there (`gpu_android`, `gpu_draw`,
//!   `vk`), with and without `gpu-draw`; and the layers' (`tests/renderers.rs`: Vulkan, OpenGL ES).
//! - With `TAUT_ANDROID_GPU_APK` naming the gallery packed with `gpu-draw` (`cargo run --example
//!   apk -- gallery --features gpu-draw,touch`): the gallery shown by the CPU, then by Vulkan and
//!   OpenGL ES (`debug.taut.renderer`), each only showing or also drawing every part
//!   (`debug.taut.draw`). Each one's screen (first, after a tap, turned, after the home screen
//!   and back) is held against the CPU's: the same pixels where the GPU only shows them, within a
//!   few levels where it draws them.
//!
//! Without an emulator it says so and passes, unless `TAUT_ANDROID_GPU=required`. `TAUT_PICTURES`
//! names a folder for the screens and the log.
#[cfg(not(target_os = "android"))]
mod on_android;

#[cfg(target_os = "android")]
fn main() {}

#[cfg(not(target_os = "android"))]
fn main() {
    host::main();
}

#[cfg(not(target_os = "android"))]
mod host {
    use super::on_android;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{Duration, Instant};

    const PACKAGE: &str = "taut.example.gallery";
    /// Each way of showing the gallery: the renderer and drawing asked, what should show it, and
    /// how far its pixels may be from the CPU's.
    const WAYS: [(&str, &str, &str, u32); 5] =
        [("cpu", "cpu", "Cpu", 0), ("vulkan", "cpu", "Vulkan", 0), ("vulkan", "gpu", "Vulkan", 4), ("opengl", "cpu", "OpenGL", 0), ("opengl", "gpu", "OpenGL", 4)];
    /// The screens taken of each.
    const SCREENS: [&str; 4] = ["first", "ticked", "turned", "back"];

    pub fn main() {
        let required = std::env::var("TAUT_ANDROID_GPU").as_deref() == Ok("required");
        let (Some(adb), Some(linker)) = (on_android::adb(), on_android::linker()) else {
            assert!(!required, "no emulator adb reaches, or no NDK");
            return println!("no emulator adb reaches, or no NDK: the GPU checks on Android are skipped");
        };
        let out = std::env::var_os("TAUT_PICTURES").map_or_else(|| std::env::temp_dir().join("taut-android-gpu"), PathBuf::from);
        std::fs::create_dir_all(&out).unwrap();
        ask(&adb, "", "");
        let mut failed = Vec::new();
        for (features, filters) in [("gpu", "gpu_android"), ("gpu-draw", "gpu_android gpu_draw vk::")] {
            let program = on_android::built(&linker, &["--lib"], features, &format!("gpu-android-{features}"));
            let (said, passed) = on_android::ran(&adb, &program, "taut-gpu-checks", "", &format!("{filters} --test-threads=1 --nocapture"));
            println!("the library's checks with {features}:\n{said}");
            failed.extend((!passed).then(|| format!("the library's checks with {features} failed")));
        }
        let program = on_android::built(&linker, &["--test", "renderers"], "gpu-draw", "gpu-android-renderers");
        let (said, passed) = on_android::ran(&adb, &program, "taut-renderers", "", "on_vulkan on_opengl_es --test-threads=1 --nocapture");
        println!("the layers' checks:\n{said}");
        failed.extend((!passed).then(|| "the layers' checks failed".to_string()));
        match std::env::var_os("TAUT_ANDROID_GPU_APK") {
            Some(apk) => failed.extend(gallery(&adb, Path::new(&apk), &out)),
            None => println!("no TAUT_ANDROID_GPU_APK: the gallery is not checked"),
        }
        ask(&adb, "", "");
        finish(&adb, &out, failed);
    }

    fn run(adb: &Path, args: &[&str]) -> String {
        Command::new(adb).args(args).output().map(|done| String::from_utf8_lossy(&done.stdout).into_owned()).unwrap_or_default()
    }

    fn shell(adb: &Path, command: &str) -> String {
        run(adb, &["shell", command])
    }

    fn settle(seconds: u64) {
        std::thread::sleep(Duration::from_secs(seconds));
    }

    /// Asks every app for a renderer and a way of drawing (none: the app's own choice).
    fn ask(adb: &Path, renderer: &str, draw: &str) {
        shell(adb, &format!("setprop debug.taut.renderer '{renderer}'; setprop debug.taut.draw '{draw}'"));
    }

    /// The screen inside the window's insets (as the app last logged them), as `0xAARRGGBB`, and
    /// its width; the whole screen kept in `out` as `name`.
    fn screen(adb: &Path, out: &Path, name: &str) -> (usize, Vec<u32>) {
        let png = Command::new(adb).args(["exec-out", "screencap", "-p"]).output().map(|done| done.stdout).unwrap_or_default();
        let _ = std::fs::write(out.join(format!("{name}.png")), &png);
        let (width, pixels) = taut::png::decode(&png).unwrap_or_default();
        let log = run(adb, &["logcat", "-d", "-s", "taut:I"]);
        let line = log.lines().rev().find_map(|line| line.split("window ").nth(1)).unwrap_or_default();
        let numbers: Vec<usize> = line.split_whitespace().filter_map(|word| word.parse().ok()).collect();
        let [_, height, left, top, right, bottom] = numbers.get(..6).and_then(|numbers| numbers.try_into().ok()).unwrap_or([0; 6]);
        let inner = width.saturating_sub(left + right);
        let rows = (top..height.saturating_sub(bottom)).filter_map(|row| pixels.get(row * width + left..row * width + left + inner));
        (inner, rows.flatten().copied().collect())
    }

    /// The centre of the gallery's tick box, as `uiautomator` reads the window.
    fn tick_box(adb: &Path) -> Option<[i32; 2]> {
        shell(adb, "uiautomator dump /sdcard/taut-gpu.xml");
        let xml = run(adb, &["exec-out", "cat", "/sdcard/taut-gpu.xml"]);
        let node = xml.split("<node ").find(|node| node.contains("class=\"android.widget.CheckBox\""))?;
        let bounds = node.split("bounds=\"").nth(1)?.split('"').next()?;
        let corners: Vec<i32> = bounds.split(|c: char| !c.is_ascii_digit()).filter_map(|part| part.parse().ok()).collect();
        (corners.len() == 4).then(|| [(corners[0] + corners[2]) / 2, (corners[1] + corners[3]) / 2])
    }

    /// The gallery started afresh, once the app says `shown` shows it; then its screens, or what
    /// went wrong. `name` names the pictures.
    fn screens(adb: &Path, out: &Path, name: &str, shown: &str) -> Result<Vec<(usize, Vec<u32>)>, String> {
        shell(adb, &format!("am force-stop {PACKAGE}"));
        run(adb, &["logcat", "-c"]);
        shell(adb, "settings put system accelerometer_rotation 0; settings put system user_rotation 0");
        shell(adb, &format!("am start -W -n {PACKAGE}/taut.Activity"));
        let since = Instant::now();
        while !run(adb, &["logcat", "-d", "-s", "taut:I"]).contains(&format!("shown by {shown}")) && shown != "Cpu" {
            if since.elapsed() > Duration::from_secs(60) {
                return Err(format!("{name}: never shown by {shown}"));
            }
            settle(1);
        }
        settle(4);
        let mut taken = vec![screen(adb, out, &format!("{name}-first"))];
        let [x, y] = tick_box(adb).ok_or(format!("{name}: no tick box told"))?;
        shell(adb, &format!("input tap {x} {y}"));
        settle(2);
        taken.push(screen(adb, out, &format!("{name}-ticked")));
        shell(adb, "settings put system user_rotation 1");
        settle(5);
        taken.push(screen(adb, out, &format!("{name}-turned")));
        shell(adb, "settings put system user_rotation 0");
        settle(4);
        shell(adb, "input keyevent KEYCODE_HOME");
        settle(3);
        shell(adb, &format!("am start -W -n {PACKAGE}/taut.Activity"));
        settle(4);
        taken.push(screen(adb, out, &format!("{name}-back")));
        let said = run(adb, &["logcat", "-d", "-s", "taut:I"]);
        if shown != "Cpu" && said.contains("shown by Cpu") {
            return Err(format!("{name}: the CPU showed the gallery after {shown} had:\n{said}"));
        }
        Ok(taken)
    }

    /// The gallery shown each way, its screens against the CPU's; what differs more than it may.
    fn gallery(adb: &Path, apk: &Path, out: &Path) -> Vec<String> {
        // (A gallery signed with another key would not be replaced.)
        run(adb, &["uninstall", PACKAGE]);
        run(adb, &["install", &apk.to_string_lossy()]);
        shell(adb, "settings put global hide_error_dialogs 1");
        let mut failed = Vec::new();
        let mut cpu = Vec::new();
        for (renderer, draw, shown, most) in WAYS {
            let name = format!("{renderer}-{draw}");
            ask(adb, renderer, draw);
            let taken = match screens(adb, out, &name, shown) {
                Ok(taken) => taken,
                Err(error) => {
                    failed.push(error);
                    continue;
                }
            };
            if renderer == "cpu" {
                cpu = taken;
                continue;
            }
            for ((screen, (width, pixels)), (cpu_width, cpu_pixels)) in SCREENS.iter().zip(&taken).zip(&cpu) {
                let off = |(a, b): (&u32, &u32)| [16, 8, 0].map(|shift| (a >> shift & 255).abs_diff(b >> shift & 255)).into_iter().max().unwrap_or(0);
                let worst = pixels.iter().zip(cpu_pixels).map(off).max().unwrap_or(u32::MAX);
                let differ = pixels.iter().zip(cpu_pixels).filter(|pair| off(*pair) > 0).count();
                println!("{name}, {screen}: {differ} of {} pixels differ from the CPU's, by {worst} at most", pixels.len());
                if (width, pixels.len()) != (cpu_width, cpu_pixels.len()) || pixels.is_empty() || worst > most {
                    failed.push(format!("{name}, {screen}: {differ} pixels differ from the CPU's, by {worst} (at most {most} may)"));
                }
            }
        }
        shell(adb, &format!("am force-stop {PACKAGE}"));
        failed
    }

    /// Keeps the log, and fails with what failed.
    fn finish(adb: &Path, out: &Path, failed: Vec<String>) {
        let log = run(adb, &["logcat", "-d"]);
        let _ = std::fs::write(out.join("logcat.txt"), &log);
        if log.contains("Fatal signal") || log.contains("FATAL EXCEPTION") {
            eprintln!("an app crashed: see logcat.txt");
            std::process::exit(1);
        }
        if !failed.is_empty() {
            eprintln!("FAILED:\n{}", failed.join("\n"));
            std::process::exit(1);
        }
        println!("all GPU checks on Android passed");
    }
}
