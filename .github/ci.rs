//! What the CI jobs run besides cargo. Each job builds it first:
//! `rustc -O --edition 2024 .github/ci.rs --out-dir ~` (`~/ci`, or `~/ci.exe` on Windows).
//!
//!   ci test <command...>
//!     Runs a test command, its output shown as it comes, and exits with its code. When it fails,
//!     also prints an `::error` annotation for each failed test (its panic message), each test
//!     program that crashed (why, and its last lines), or else the output's last lines: unlike the
//!     job's log, its annotations can be read without a login
//!     (`api.github.com/repos/<owner>/<repo>/check-runs/<job id>/annotations`). On any runner.
//!
//! On Ubuntu only:
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
//!
//!   ci locales <folder> <name...>
//!     Compiles glibc's locales `<name>.UTF-8` (en_US, de_DE, ...) into <folder>, for programs run
//!     with LOCPATH=<folder>; no root is needed.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::sleep;
use std::time::{Duration, Instant};

const ARCHIVES: &str = "/var/cache/apt/archives";

/// The most error annotations GitHub keeps from one step.
const MOST_ANNOTATIONS: usize = 10;
/// The most lines of output one annotation quotes.
const QUOTED_LINES: usize = 12;
/// The longest line an annotation quotes, in characters.
const QUOTED_WIDTH: usize = 300;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.split_first() {
        Some((what, command)) if what == "test" && !command.is_empty() => return test(command),
        Some((what, packages)) if what == "apt" && !packages.is_empty() => apt(packages),
        Some((what, rest)) if what == "desk" && rest.len() > 2 => desk(&rest[0], &rest[1], &rest[2..]),
        Some((what, rest)) if what == "locales" && rest.len() > 1 => locales(Path::new(&rest[0]), &rest[1..]),
        _ => Err("usage: ci test <command...> | ci apt <package...> | ci desk <kind> <display> <command...> | ci locales <folder> <name...>".into()),
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

/// Whether the step passed, or why it could not run.
type Done = Result<bool, String>;

/// `ci apt`: installs `packages`, keeping what was downloaded (see the top).
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

/// The package files in `folder`.
fn debs_in(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else { return Vec::new() };
    let paths = entries.filter_map(|entry| Some(entry.ok()?.path()));
    paths.filter(|path| path.extension().is_some_and(|e| e == "deb")).map(|path| path.display().to_string()).collect()
}

/// `ci locales`: compiles each of `names` into `folder` (see the top).
fn locales(folder: &Path, names: &[String]) -> Done {
    fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    for name in names {
        let into = folder.join(format!("{name}.UTF-8"));
        run("localedef", &["-i", name, "-f", "UTF-8", &into.to_string_lossy()])?;
    }
    Ok(true)
}

/// Runs `args` as root.
fn sudo(args: &[&str]) -> Result<(), String> {
    run("sudo", args)
}

/// Runs `program` with `args`; an error unless it succeeds.
fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program).args(args).status().map_err(|e| format!("{program}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{program} {} ended with {status}", args.join(" "))) }
}

/// `ci desk`: runs `command` under a Weston of its own (see the top).
fn desk(kind: &str, display: &str, command: &[String]) -> Done {
    let runtime = std::env::temp_dir().join(format!("taut-desk-{}", std::process::id()));
    fs::create_dir_all(&runtime).map_err(|e| format!("{}: {e}", runtime.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let renderer = if kind.ends_with("-gl") { "gl" } else { "pixman" };
    let on_x = kind.starts_with("weston-x11");
    let x_display = format!(":{display}");
    let env = [("XDG_RUNTIME_DIR", runtime.to_str().unwrap_or_default()), ("GSETTINGS_BACKEND", "memory"), ("TAUT_SCREEN_SCALE", "1"), ("TAUT_DESK", if on_x { "weston-x11" } else { "weston" })];
    let started = |program: &str, args: &[&str], log: &str, extra: &[(&str, &str)]| {
        let log = fs::File::create(runtime.join(log)).map_err(|e| e.to_string())?;
        let mut command = Command::new(program);
        // (Removed first: `extra` may give Weston its X display.)
        command.env_remove("WAYLAND_DISPLAY").env_remove("DISPLAY").env_remove("WAYLAND_SOCKET");
        command.args(args).envs(env).envs(extra.iter().copied()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
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
    let mut tests = Command::new(&command[0]);
    tests.args(&command[1..]).env_remove("DISPLAY").env_remove("WAYLAND_SOCKET").envs(env).envs(test_env.iter().copied());
    let mut untested = false;
    let ran = run_shown(tests, |line| untested |= line.contains("not tested"));
    drop(servers);
    let passed = ran?.success();
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

/// Runs `command`, its output (both streams) shown as it comes and handed line by line to `each`,
/// until it ends. (Not until its output does: a process it started may outlive it and hold the
/// pipe open, as the accessibility bus of a test does.)
fn run_shown(mut command: Command, mut each: impl FnMut(&str)) -> Result<ExitStatus, String> {
    let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    command.stdout(writer.try_clone().map_err(|e| e.to_string())?).stderr(writer);
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", command.get_program().display()))?;
    // (The command holds the writers: the reader ends with the child only once it is gone.)
    drop(command);
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || read_lines(reader, &send));
    let mut show = |line: String| {
        println!("{line}");
        each(&line);
    };
    let status = loop {
        match lines.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => show(line),
            Err(RecvTimeoutError::Disconnected) => break child.wait(),
            Err(RecvTimeoutError::Timeout) => match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {}
                Err(error) => break Err(error),
            },
        }
    };
    // What it wrote before it ended: until the pipe is quiet for a moment, 5 s at most.
    let ended = Instant::now();
    while let (Ok(line), true) = (lines.recv_timeout(Duration::from_millis(500)), ended.elapsed() < Duration::from_secs(5)) {
        show(line);
    }
    status.map_err(|e| e.to_string())
}

/// Sends each line `reader` gives, as text, until it ends or nobody listens.
fn read_lines(reader: std::io::PipeReader, send: &mpsc::Sender<String>) {
    let mut reader = BufReader::new(reader);
    let mut bytes = Vec::new();
    while reader.read_until(b'\n', &mut bytes).is_ok_and(|read| read > 0) {
        let line = String::from_utf8_lossy(&bytes).trim_end_matches(['\n', '\r']).to_string();
        if send.send(line).is_err() {
            return;
        }
        bytes.clear();
    }
}

/// `ci test`: runs `command` and annotates what failed (see the top).
fn test(command: &[String]) -> ExitCode {
    let mut tests = Command::new(&command[0]);
    tests.args(&command[1..]);
    let mut report = TestReport::default();
    let status = match run_shown(tests, |line| report.read(line)) {
        Ok(status) => status,
        Err(error) => {
            println!("::error title={}::{}", escape_property(&command.join(" ")), escape_message(&error));
            return ExitCode::FAILURE;
        }
    };
    if status.success() {
        return ExitCode::SUCCESS;
    }
    for annotation in report.annotations(&command.join(" ")) {
        println!("{annotation}");
    }
    let code = status.code().and_then(|code| u8::try_from(code).ok()).filter(|&code| code != 0);
    ExitCode::from(code.unwrap_or(1))
}

/// What a test command's output says failed, read line by line: libtest's and cargo's words.
#[derive(Default)]
struct TestReport {
    /// The test program running, as cargo names it ("tests/renderers.rs", "src/lib.rs").
    program: String,
    /// The program's latest lines, for a crash.
    recent: VecDeque<String>,
    /// The whole output's latest lines, for a failure nothing names.
    last: VecDeque<String>,
    /// The test whose kept output is being read (`---- name stdout ----`).
    kept: Option<Said>,
    /// A panic message being read as tests print it (`--nocapture`, or no libtest at all).
    panic: Option<Said>,
    /// What each test said, kept output or panic message.
    said: Vec<Said>,
    /// Whether the lines are libtest's list of failed tests.
    listing: bool,
    /// The failed tests, as (program, name).
    failed: Vec<(String, String)>,
    /// The programs that failed.
    ended: Vec<Ended>,
}

/// What one test, or one panicking thread, said.
struct Said {
    program: String,
    test: String,
    lines: Vec<String>,
}

/// A test program that failed, and its last lines.
struct Ended {
    program: String,
    /// Why, when not for failed tests or a panic (exit code 101): "exit code: 0xc0000005, …".
    crash: Option<String>,
    last: Vec<String>,
}

impl TestReport {
    /// Takes in one line of output.
    fn read(&mut self, line: &str) {
        let line = without_colours(line);
        remember(&mut self.last, &line);
        if let Some(program) = program_started(&line) {
            self.program = program;
            self.recent.clear();
            return;
        }
        remember(&mut self.recent, &line);
        if let Some(test) = line.strip_prefix("---- ").and_then(|rest| rest.strip_suffix(" stdout ----")) {
            self.end_kept();
            self.listing = false;
            self.kept = Some(Said { program: self.program.clone(), test: test.to_string(), lines: Vec::new() });
            return;
        }
        let ends_kept = line == "failures:" || line.starts_with("test result:");
        if let (Some(kept), false) = (&mut self.kept, ends_kept) {
            kept.lines.push(line);
            return;
        }
        self.end_kept();
        self.read_panic(&line);
        self.read_list(&line);
        self.read_ended(&line);
    }

    /// Follows cargo's report of a program that failed: `error: test failed, to rerun pass …`
    /// (after the program's last words, maybe on their line), then for a crash `Caused by:` and
    /// `process didn't exit successfully: `… (why)`.
    fn read_ended(&mut self, line: &str) {
        if let Some((before, _)) = line.split_once("error: test failed, to rerun pass") {
            let mut last: Vec<String> = self.recent.iter().take(self.recent.len() - 1).cloned().collect();
            last.extend((!before.trim().is_empty()).then(|| before.to_string()));
            self.ended.push(Ended { program: self.program.clone(), crash: None, last });
        } else if let Some(why) = line.split("process didn't exit successfully: ").nth(1) {
            let why = why.rsplit(" (").next().unwrap_or(why).trim_end_matches(')');
            if let Some(ended) = self.ended.last_mut() {
                ended.crash = Some(why.to_string());
            }
        }
    }

    /// Follows a panic message printed as it comes: from `thread '…' panicked at` to the note,
    /// backtrace, blank or test line after it.
    fn read_panic(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("thread '").filter(|rest| is_panic(rest)) {
            self.end_panic();
            let test = rest.split('\'').next().unwrap_or_default().to_string();
            self.panic = Some(Said { program: self.program.clone(), test, lines: vec![line.to_string()] });
            return;
        }
        let Some(panic) = &mut self.panic else { return };
        if line.is_empty() || line.starts_with("test ") || ends_message(line) || panic.lines.len() == QUOTED_LINES {
            self.end_panic();
        } else {
            panic.lines.push(line.to_string());
        }
    }

    /// Follows libtest's closing list of failed tests: `failures:`, then one indented name a line.
    fn read_list(&mut self, line: &str) {
        if line == "failures:" {
            self.listing = true;
        } else if line.starts_with("test result:") {
            self.listing = false;
        } else if let (true, Some(test)) = (self.listing, line.strip_prefix("    ")) {
            self.failed.push((self.program.clone(), test.to_string()));
        }
    }

    fn end_kept(&mut self) {
        self.said.extend(self.kept.take());
    }

    fn end_panic(&mut self) {
        self.said.extend(self.panic.take());
    }

    /// The annotations for a failed `command`: each failed test, each panic in a program that
    /// failed without naming tests, each other failed program; or else the output's last lines.
    fn annotations(mut self, command: &str) -> Vec<Annotation> {
        self.end_kept();
        self.end_panic();
        let mut annotations: Vec<Annotation> = self.failed.iter().map(|(program, test)| self.failed_test(program, test)).collect();
        for ended in &self.ended {
            annotations.extend(self.unnamed_failure(ended));
        }
        if annotations.is_empty() {
            annotations.push(Annotation::quoting(format!("{command} failed"), &Vec::from(self.last)));
        }
        if annotations.len() > MOST_ANNOTATIONS {
            let more: Vec<String> = annotations.drain(MOST_ANNOTATIONS - 1..).map(|annotation| annotation.title).collect();
            annotations.push(Annotation { title: format!("{} more failed", more.len()), place: None, lines: more });
        }
        annotations
    }

    /// The annotation for one failed test: its panic message, else the end of what it said.
    fn failed_test(&self, program: &str, test: &str) -> Annotation {
        let title = format!("{program}: {test}");
        match self.said.iter().find(|said| said.program == program && said.test == test) {
            Some(said) => Annotation::quoting(title, &said.lines),
            None => Annotation { title, place: None, lines: vec!["failed (no message found)".to_string()] },
        }
    }

    /// The annotations for a failed program beyond the tests libtest named: a crash, or else each
    /// thread that panicked (a program without libtest), or else its last lines.
    fn unnamed_failure(&self, ended: &Ended) -> Vec<Annotation> {
        let program = &ended.program;
        if let Some(why) = &ended.crash {
            return vec![Annotation::quoting(format!("{program}: {why}"), &ended.last)];
        }
        if self.failed.iter().any(|(failed_in, _)| failed_in == program) {
            return Vec::new();
        }
        let panics = self.said.iter().filter(|said| said.program == *program && said.lines.iter().any(|line| is_panic(line)));
        let mut annotations: Vec<Annotation> = panics.map(|said| Annotation::quoting(format!("{program}: thread '{}' panicked", said.test), &said.lines)).collect();
        if annotations.is_empty() {
            annotations.push(Annotation::quoting(format!("{program} failed"), &ended.last));
        }
        annotations
    }
}

/// One `::error` annotation: a title, the source line it points at, the lines it quotes.
struct Annotation {
    title: String,
    place: Option<(String, u32)>,
    lines: Vec<String>,
}

impl Annotation {
    /// An annotation quoting `lines`: with panics in them, the last one's message and the place
    /// it names (a test quoting a program it ran, such as the iOS app, has that program's last);
    /// else their last lines that say something.
    fn quoting(title: String, lines: &[String]) -> Annotation {
        let Some(panic) = lines.iter().rposition(|line| is_panic(line)) else {
            let mut lines: Vec<String> = lines.iter().filter(|line| !line.trim().is_empty()).cloned().collect();
            lines.drain(..lines.len().saturating_sub(QUOTED_LINES));
            return Annotation { title, place: None, lines };
        };
        let message = lines[panic..].iter().take_while(|line| !ends_message(line)).take(QUOTED_LINES);
        Annotation { title, place: place_of(&lines[panic]), lines: message.cloned().collect() }
    }
}

impl std::fmt::Display for Annotation {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "::error ")?;
        if let Some((file, line)) = &self.place {
            write!(f, "file={},line={line},", escape_property(file))?;
        }
        let lines: Vec<String> = self.lines.iter().map(|line| line.chars().take(QUOTED_WIDTH).collect()).collect();
        write!(f, "title={}::{}", escape_property(&self.title), escape_message(&lines.join("\n")))
    }
}

/// The program cargo starts running on `line` ("Running tests/renderers.rs (target/…)", or
/// "Doc-tests taut"), as cargo names it.
fn program_started(line: &str) -> Option<String> {
    let line = line.trim_start();
    if line.starts_with("Doc-tests ") {
        return Some("doc tests".to_string());
    }
    let (program, _binary) = line.strip_prefix("Running ")?.strip_suffix(')')?.split_once(" (")?;
    Some(program.trim_start_matches("unittests ").replace('\\', "/"))
}

/// Whether `line` starts a panic message.
fn is_panic(line: &str) -> bool {
    line.contains(" panicked at ")
}

/// Whether `line` follows a panic message: Rust's note or backtrace.
fn ends_message(line: &str) -> bool {
    line.starts_with("note: ") || line.starts_with("stack backtrace:")
}

/// The file and line a panic line names (`… panicked at src/video.rs:1271:5:`), when the file is
/// in the repository.
fn place_of(line: &str) -> Option<(String, u32)> {
    let place = line.split(" panicked at ").nth(1)?.trim_end_matches(':');
    let mut parts = place.rsplitn(3, ':');
    let (_column, line, file) = (parts.next()?, parts.next()?.parse().ok()?, parts.next()?);
    let file = file.replace('\\', "/");
    let outside = file.starts_with('/') || file.contains(':') || file.starts_with("..");
    (!outside).then_some((file, line))
}

/// Keeps `line` as the latest of at most `QUOTED_LINES` in `lines`.
fn remember(lines: &mut VecDeque<String>, line: &str) {
    if lines.len() == QUOTED_LINES {
        lines.pop_front();
    }
    lines.push_back(line.to_string());
}

/// `line` without its colour codes (`ESC [ … letter`).
fn without_colours(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            chars.by_ref().find(char::is_ascii_alphabetic);
        } else {
            plain.push(c);
        }
    }
    plain
}

/// `text` as an annotation's message: GitHub reads `%`, CR and LF as escapes.
fn escape_message(text: &str) -> String {
    text.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A")
}

/// `text` as an annotation's property (its title, its file): also `:` and `,`.
fn escape_property(text: &str) -> String {
    escape_message(text).replace(':', "%3A").replace(',', "%2C")
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

/// The home folder.
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
