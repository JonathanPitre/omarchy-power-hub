//! The on-resume run: the update steps in order, one notification at the end.
use crate::{exec, paths, resume, t3};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Set for a run in a visible terminal, which shows each step's output as well as logging it.
static TERMINAL: AtomicBool = AtomicBool::new(false);

const LONG: Duration = Duration::from_secs(2 * 3600);
const MAX_LISTED: usize = 8;

/// `Updated` holds (name, from, to).
pub enum StepResult {
    Current,
    Updated(Vec<(String, String, String)>),
    Failed(String),
}

type Versions = Vec<(String, String)>;

fn dir() -> std::path::PathBuf {
    paths::state_dir().join("omarchy-power-hub")
}

pub fn log(text: &str) {
    let _ = fs::create_dir_all(dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(resume::log_file()) {
        let _ = writeln!(f, "{text}");
    }
}

/// Runs one command, appending its output to the log under a timestamp.
pub fn sh(name: &str, argv: &[&str]) -> Result<(), String> {
    println!("{name}");
    let stamp = exec::run(&["date", "+%F %T"], Duration::from_secs(5)).1;
    let head = format!("[{}] {name}: {}", stamp.trim(), argv.join(" "));
    let code = if TERMINAL.load(Ordering::Relaxed) {
        log(&head);
        stream(argv)
    } else {
        let (code, out) = exec::run(argv, LONG);
        log(&format!("{head}\n{}", out.trim_end()));
        code
    };
    if code == 0 { Ok(()) } else { Err(format!("exit {code}")) }
}

/// Gives the step the terminal, for sudo's prompt, and echoes its output to it and the log as the bytes arrive.
/// `--foreground` keeps the step in the terminal's process group, where it may read; `timeout` keeps the limit.
fn stream(argv: &[&str]) -> i32 {
    let child = Command::new("sh").args(["-c", "exec timeout --foreground 7200 \"$@\" 2>&1", "sh"]).args(argv).stdout(Stdio::piped()).spawn();
    let Ok(mut child) = child else { return 1 };
    let (mut pipe, mut buf, mut last) = (child.stdout.take().expect("piped"), [0; 4096], b'\n');
    let mut file = OpenOptions::new().append(true).open(resume::log_file()).ok();
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut tty = std::io::stdout().lock();
                let _ = tty.write_all(&buf[..n]).and_then(|()| tty.flush());
                file.as_mut().map(|f| f.write_all(&buf[..n]));
                last = buf[n - 1];
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    if last != b'\n' {
        println!();
        file.as_mut().map(|f| f.write_all(b"\n"));
    }
    child.wait().ok().and_then(|s| s.code()).unwrap_or(1)
}

fn probe(argv: &[&str]) -> String {
    let (code, out) = exec::run(argv, Duration::from_secs(30));
    if code == 0 { out } else { String::new() }
}

fn packages(extra: &[&str]) -> Versions {
    let argv = [&["pacman", "-Q"], extra].concat();
    probe(&argv).lines().filter_map(|l| l.split_once(' ')).map(|(n, v)| (n.into(), v.trim().into())).collect()
}

/// The second word of a tool's output, which is its version for `lean-ctx --version` and `mise ls`.
fn version(name: &str, argv: &[&str]) -> Versions {
    let out = probe(argv);
    out.split_whitespace().nth(1).map(|v| vec![(name.into(), v.into())]).unwrap_or_default()
}

fn plugin_heads() -> Versions {
    let mut dirs: Vec<_> = fs::read_dir(paths::home().join(".config/omarchy/plugins")).into_iter().flatten().flatten().map(|e| e.path()).collect();
    dirs.sort();
    dirs.iter()
        .filter_map(|d| {
            let head = probe(&["git", "-C", &d.to_string_lossy(), "rev-parse", "--short", "HEAD"]);
            Some((d.file_name()?.to_string_lossy().into_owned(), head.trim().to_owned())).filter(|(_, h)| !h.is_empty())
        })
        .collect()
}

/// Runs `argv` and reports what `snap` saw change across it.
fn tracked(name: &str, argv: &[&str], snap: impl Fn() -> Versions) -> StepResult {
    let before = snap();
    if let Err(e) = sh(name, argv) {
        return StepResult::Failed(e);
    }
    let changed: Vec<_> = snap().into_iter().filter_map(|(n, to)| before.iter().find(|(b, from)| *b == n && *from != to).map(|(_, from)| (n, from.clone(), to))).collect();
    if changed.is_empty() { StepResult::Current } else { StepResult::Updated(changed) }
}

fn script_step(path: &str) -> StepResult {
    if !exec::executable(std::path::Path::new(path)) {
        log(&format!("script is missing or not executable: {path}"));
        return StepResult::Failed(format!("{path} is missing or not executable"));
    }
    sh("Script", &[path]).map_or_else(StepResult::Failed, |()| StepResult::Current)
}

fn plan(cfg: &Value, installed: &Value) -> Vec<&'static str> {
    if cfg["script"] != "" {
        return vec!["Script"];
    }
    let (on, has, alone) = (|k: &str| cfg[k] == true, |k: &str| installed[k] == true, cfg["omarchy"] == false);
    [
        (on("omarchy"), "Omarchy"),
        (on("plugins"), "Omarchy plugins"),
        (on("leanCtx") && has("leanCtx"), "lean-ctx"),
        (alone && on("rulesync") && has("rulesync"), "Rulesync"),
        (alone && on("skillsManager") && has("skillsManager"), "Skills Manager"),
        (cfg["t3code"] != "off" && has("t3code"), "T3 Code"),
    ]
    .into_iter()
    .filter_map(|(run, name)| run.then_some(name))
    .collect()
}

fn step(name: &str, cfg: &Value) -> StepResult {
    match name {
        "Omarchy" => tracked(name, &["omarchy", "update", "-y"], || packages(&[])),
        "Omarchy plugins" => tracked(name, &["omarchy", "plugin", "update", "--yes"], plugin_heads),
        "lean-ctx" => tracked(name, &["lean-ctx", "update"], || version("lean-ctx", &["lean-ctx", "--version"])),
        "Rulesync" => tracked(name, &["mise", "up", "npm:rulesync"], || version("rulesync", &["mise", "ls", "npm:rulesync"])),
        "Skills Manager" => tracked(name, &["skills-manager-refresh", "--noconfirm"], || packages(&["skills-manager-prebuilt"])),
        "T3 Code" => t3::step(cfg["t3code"].as_str().unwrap_or("update")),
        _ => script_step(cfg["script"].as_str().unwrap_or("")),
    }
}

/// The checks of `omarchy-update-restart`: a newer kernel, the flag file, a replaced Hyprland.
pub fn reboot_required() -> bool {
    let running = fs::read_to_string(paths::root("/proc/sys/kernel/osrelease")).unwrap_or_default();
    let installed: Vec<_> = fs::read_dir(paths::root("/usr/lib/modules"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("modules.builtin").is_file() && exec::run(&["pacman", "-Qo", &e.path().join("modules.builtin").to_string_lossy()], Duration::from_secs(10)).0 == 0)
        .collect();
    let kernel = !running.trim().is_empty() && !installed.is_empty() && !installed.iter().any(|e| e.file_name().to_string_lossy() == running.trim());
    let hypr = probe(&["pgrep", "-x", "Hyprland"]).trim().to_owned();
    let replaced = !hypr.is_empty() && fs::read_link(paths::root(&format!("/proc/{hypr}/exe"))).is_ok_and(|p| p.to_string_lossy().ends_with("(deleted)"));
    kernel || replaced || paths::home().join(".local/state/omarchy/reboot-required").exists()
}

/// Holds the run's lock and `runningPid`; dropping it, even in a panic, releases both.
struct Held;

impl Drop for Held {
    fn drop(&mut self) {
        let _ = resume::update_state(|s| {
            s.remove("runningPid");
        });
        let _ = fs::remove_file(dir().join("resume.lock"));
    }
}

/// Takes the run's exclusive lock, replacing one whose holder is gone.
fn lock() -> Option<Held> {
    let path = dir().join("resume.lock");
    let _ = fs::create_dir_all(dir());
    for _ in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => return writeln!(f, "{}", std::process::id()).ok().map(|()| Held),
            Err(_) if fs::read_to_string(&path).ok().and_then(|p| p.trim().parse().ok()).is_some_and(resume::running) => return None,
            Err(_) => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    None
}

fn notify(updates: &[(String, String, String)], failed: &[(&str, String)], reboot: bool) {
    let mut lines: Vec<String> = updates.iter().take(MAX_LISTED).map(|(n, from, to)| format!("{n} {from} → {to}")).collect();
    if updates.len() > MAX_LISTED {
        lines.push(format!("+{} more", updates.len() - MAX_LISTED));
    }
    lines.extend(failed.iter().map(|(n, _)| format!("{n} failed, see {}", resume::log_file().display())));
    let summary = if !failed.is_empty() { "Update failed" } else if reboot { "Restart to finish" } else { "Updated" };
    let body = lines.join("\n");
    let mut argv = vec!["notify-send", "-a", "Power Hub"];
    if reboot {
        argv.extend(["-A", "reboot=Reboot"]);
    }
    argv.extend([summary, &body]);
    // With an action the call waits for the user, so nothing is held open by now.
    let (_, out) = exec::run(&argv, Duration::from_secs(3600));
    if reboot && out.trim() == "reboot" {
        exec::run(&["omarchy-system", "reboot"], Duration::from_secs(60));
    }
}

/// `resume-run --exec`: runs the planned steps and reports once. Always 0.
pub fn exec() -> i32 {
    let Some(held) = lock() else { return 0 };
    let _ = resume::update_state(|s| {
        s.insert("runningPid".into(), json!(std::process::id()));
    });
    let (cfg, status) = (resume::settings(), resume::status());
    let names = plan(&cfg, &status["installed"]);
    let terminal = status["needsSudo"] == true;
    TERMINAL.store(terminal, Ordering::Relaxed);
    let mut sudo = true;
    if terminal {
        names.iter().for_each(|n| println!("- {n}"));
        println!("\n{}", if cfg["omarchy"] == true { "Omarchy update installs system packages" } else { "Skills Manager installs a system package" });
        // `omarchy update` asks itself: it re-execs under `script`, a new tty that a ticket from here would not cover.
        if names.contains(&"Skills Manager") {
            sudo = Command::new("sudo").arg("true").status().is_ok_and(|s| s.success());
            if !sudo {
                log("sudo true failed: skipping Skills Manager");
            }
        }
    }
    let mut results: Vec<(&str, StepResult)> = names.iter().map(|n| (*n, if !sudo && *n == "Skills Manager" { StepResult::Failed("sudo".into()) } else { step(n, &cfg) })).collect();
    let reboot = reboot_required();
    if cfg["restartShell"] == true && !reboot && results.iter().any(|(n, r)| *n == "Omarchy plugins" && matches!(r, StepResult::Updated(_))) {
        let r = sh("Restart shell", &["omarchy", "restart", "shell"]).map_or_else(StepResult::Failed, |()| StepResult::Current);
        results.push(("Restart shell", r));
    }
    let updates: Vec<_> = results.iter().filter_map(|(_, r)| if let StepResult::Updated(u) = r { Some(u.clone()) } else { None }).flatten().collect();
    let failed: Vec<_> = results.iter().filter_map(|(n, r)| if let StepResult::Failed(e) = r { Some((*n, e.clone())) } else { None }).collect();
    let last_error = failed.first().map_or(Value::Null, |(n, e)| json!(format!("{n}: {e}")));
    let _ = resume::update_state(|s| {
        s.insert("lastError".into(), last_error);
    });
    drop(held);
    if cfg["t3code"] == "restart" && !reboot && results.iter().any(|(n, r)| *n == "T3 Code" && matches!(r, StepResult::Updated(_))) {
        let restore = cfg["sessionRestore"] == true && status["installed"]["hyprPersist"] == true;
        let bin = paths::bin_dir().join("power-hub").to_string_lossy().into_owned();
        resume::detach(&["uwsm-app", "--", &bin, "resume-run", "--t3-restart"].into_iter().chain(restore.then_some("--restore")).collect::<Vec<_>>());
    }
    if !updates.is_empty() || !failed.is_empty() || reboot {
        notify(&updates, &failed, reboot);
    }
    0
}
