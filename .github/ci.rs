//! What the CI jobs run besides cargo, on GitHub's Ubuntu runners. Each job builds it first:
//! `rustc -O --edition 2024 .github/ci.rs -o ~/ci`.
//!
//!   ci apt <package...>
//!     Installs Ubuntu packages, keeping the downloaded files in ~/debs for the next run (the job
//!     caches that folder): some runners' mirror sends packages at 60 KB/s, and Weston alone is
//!     90 MB. apt takes a kept file only where its index lists that very file.
//!
//!   ci desk <weston-x11-gl|weston-gl|weston-x11|weston> <display> <command...>
//!     Runs a command under a Weston of its own. x11: Weston in a window on an Xvfb of its own
//!     (display <display>), whose screen the tests read and drive (TAUT_XVFB); else headless.
//!     gl: the GL renderer (takes dmabufs); else pixman (doesn't). The command gets
//!     WAYLAND_DISPLAY and TAUT_DESK; a "not tested" in its output fails the step.

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread::sleep;
use std::time::Duration;

const ARCHIVES: &str = "/var/cache/apt/archives";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.split_first() {
        Some((what, packages)) if what == "apt" && !packages.is_empty() => apt(packages),
        Some((what, rest)) if what == "desk" && rest.len() > 2 => desk(&rest[0], &rest[1], &rest[2..]),
        _ => Err("usage: ci apt <package...> | ci desk <kind> <display> <command...>".into()),
    };
    match done {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

type Done = Result<bool, String>;

fn apt(packages: &[String]) -> Done {
    let kept = home().join("debs");
    fs::create_dir_all(&kept).map_err(|e| format!("{}: {e}", kept.display()))?;
    let debs = debs_in(&kept);
    if !debs.is_empty() {
        sudo(&[&["cp"][..], &debs.iter().map(String::as_str).collect::<Vec<_>>(), &[ARCHIVES]].concat())?;
    }
    sudo(&["apt-get", "update"])?;
    let install = ["apt-get", "install", "-y", "--no-install-recommends", "-o", "APT::Keep-Downloaded-Packages=true"];
    sudo(&[&install[..], &packages.iter().map(String::as_str).collect::<Vec<_>>()].concat())?;
    // Kept for the next run: what was downloaded or taken, less what the index no longer lists.
    sudo(&["apt-get", "autoclean", "-y", "-qq"])?;
    fs::remove_dir_all(&kept).map_err(|e| format!("{}: {e}", kept.display()))?;
    fs::create_dir(&kept).map_err(|e| format!("{}: {e}", kept.display()))?;
    let mut bytes = 0;
    let debs = debs_in(Path::new(ARCHIVES));
    for deb in &debs {
        let name = Path::new(deb).file_name().unwrap_or_default();
        bytes += fs::copy(deb, kept.join(name)).map_err(|e| format!("{deb}: {e}"))?;
    }
    println!("kept for the next run: {} packages, {} MB", debs.len(), bytes / 1_000_000);
    Ok(true)
}

fn debs_in(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let paths = entries.filter_map(|entry| Some(entry.ok()?.path()));
    paths.filter(|path| path.extension().is_some_and(|e| e == "deb")).map(|path| path.display().to_string()).collect()
}

fn sudo(args: &[&str]) -> Result<(), String> {
    let status = Command::new("sudo").args(args).status().map_err(|e| format!("sudo: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("sudo {} ended with {status}", args.join(" "))) }
}

fn desk(kind: &str, display: &str, command: &[String]) -> Done {
    let runtime = std::env::temp_dir().join(format!("taut-desk-{}", std::process::id()));
    fs::create_dir_all(&runtime).map_err(|e| format!("{}: {e}", runtime.display()))?;
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let renderer = if kind.ends_with("-gl") { "gl" } else { "pixman" };
    let on_x = kind.starts_with("weston-x11");
    let x_display = format!(":{display}");
    let env = [("XDG_RUNTIME_DIR", runtime.to_str().unwrap_or_default()), ("GSETTINGS_BACKEND", "memory"), ("TAUT_SCREEN_SCALE", "1"), ("TAUT_DESK", if on_x { "weston-x11" } else { "weston" })];
    let started = |program: &str, args: &[&str], log: &str, extra: &[(&str, &str)]| {
        let log = fs::File::create(runtime.join(log)).map_err(|e| e.to_string())?;
        let mut command = Command::new(program);
        command.args(args).envs(env).envs(extra.iter().copied()).stdout(log.try_clone().map_err(|e| e.to_string())?);
        command.env_remove("WAYLAND_DISPLAY").env_remove("DISPLAY").env_remove("WAYLAND_SOCKET").stderr(log);
        command.spawn().map_err(|e| format!("{program}: {e}"))
    };

    let mut servers = Servers { weston: None, xvfb: None };
    if on_x {
        servers.xvfb = Some(started("Xvfb", &[&x_display, "-screen", "0", "900x600x24", "-nolisten", "tcp"], "xvfb.log", &[])?);
        wait_for(&PathBuf::from(format!("/tmp/.X11-unix/X{display}")), 50);
    }
    let backend = if on_x { "--backend=x11" } else { "--backend=headless" };
    let renderer_arg = format!("--renderer={renderer}");
    let weston_args = [backend, &renderer_arg, "--width=900", "--height=600", "--socket=taut-wl", "--debug", "--no-config"];
    let x_env: &[(&str, &str)] = if on_x { &[("DISPLAY", &x_display)] } else { &[] };
    servers.weston = Some(started("weston", &weston_args, "desk.log", x_env)?);
    wait_for(&runtime.join("taut-wl"), 100);
    // (Weston's desktop fades in from black for a moment.)
    sleep(Duration::from_secs(2));

    println!("--- {kind} ({renderer})");
    let test_env: &[(&str, &str)] = if on_x { &[("WAYLAND_DISPLAY", "taut-wl"), ("TAUT_XVFB", &x_display)] } else { &[("WAYLAND_DISPLAY", "taut-wl")] };
    let ran = run_shown(command, &env, test_env);
    drop(servers);
    let (passed, untested) = ran?;
    println!("--- weston said:");
    let log = fs::read_to_string(runtime.join("desk.log")).unwrap_or_default();
    let said = ["dmabuf", "renderer", "error", "fail"];
    let telling = log.lines().filter(|line| said.iter().any(|word| line.to_lowercase().contains(word)));
    telling.take(20).for_each(|line| println!("{line}"));
    let _ = fs::remove_dir_all(&runtime);
    if untested {
        println!("::error::a test was not tested under {kind}");
    }
    Ok(passed && !untested)
}

/// Runs `command`, its output shown as it comes. Whether it passed, and whether it said "not
/// tested".
fn run_shown(command: &[String], env: &[(&str, &str)], extra: &[(&str, &str)]) -> Result<(bool, bool), String> {
    let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    // The command (with the writers) goes at the end of this statement: the reader ends with the child.
    let mut child = Command::new(&command[0])
        .args(&command[1..])
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .envs(env.iter().copied())
        .envs(extra.iter().copied())
        .stdout(writer.try_clone().map_err(|e| e.to_string())?)
        .stderr(writer)
        .spawn()
        .map_err(|e| format!("{}: {e}", command[0]))?;
    let mut untested = false;
    for line in BufReader::new(reader).lines().map_while(Result::ok) {
        untested |= line.contains("not tested");
        println!("{line}");
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok((status.success(), untested))
}

/// Asks `child` to end (SIGTERM, as a shell's `kill`), then waits for it.
fn stop(mut child: Child) {
    let _ = Command::new("kill").arg(child.id().to_string()).stderr(Stdio::null()).status();
    let _ = child.wait();
}

/// Waits up to `tenths` tenths of a second for `path` to appear.
fn wait_for(path: &Path, tenths: u32) {
    for _ in 0..tenths {
        if path.exists() {
            return;
        }
        sleep(Duration::from_millis(100));
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// A Weston and the Xvfb it shows in, ended in that order when dropped, however the step ends: a
/// Weston in an X window whose server goes first ends in a fatal X error.
struct Servers {
    weston: Option<Child>,
    xvfb: Option<Child>,
}

impl Drop for Servers {
    fn drop(&mut self) {
        for child in [self.weston.take(), self.xvfb.take()].into_iter().flatten() {
            stop(child);
        }
    }
}
