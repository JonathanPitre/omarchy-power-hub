//! The on-resume run: the update steps in order, one notification at the end.
use crate::{exec, paths, resume};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::Duration;

const LONG: Duration = Duration::from_secs(2 * 3600);
const MAX_LISTED: usize = 8;

/// `Updated` holds (name, from, to).
pub enum StepResult {
    Current,
    Updated(Vec<(String, String, String)>),
    Failed(String),
    Skipped,
}

type Versions = Vec<(String, String)>;

fn dir() -> std::path::PathBuf {
    paths::state_dir().join("omarchy-power-hub")
}

fn log(text: &str) {
    let _ = fs::create_dir_all(dir());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(resume::log_file()) {
        let _ = writeln!(f, "{text}");
    }
}

/// Runs one command, appending its output to the log under a timestamp.
fn sh(name: &str, argv: &[&str]) -> Result<(), String> {
    println!("{name}");
    let stamp = exec::run(&["date", "+%F %T"], Duration::from_secs(5)).1;
    let (code, out) = exec::run(argv, LONG);
    log(&format!("[{}] {name}: {}\n{}", stamp.trim(), argv.join(" "), out.trim_end()));
    if code == 0 { Ok(()) } else { Err(format!("exit {code}")) }
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

/// Placeholder until the T3 Code step lands.
fn t3_step(_mode: &str) -> StepResult {
    StepResult::Skipped
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
        "T3 Code" => t3_step(cfg["t3code"].as_str().unwrap_or("update")),
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

/// Takes the run's exclusive lock, replacing one whose holder is gone.
fn lock() -> bool {
    let path = dir().join("resume.lock");
    let _ = fs::create_dir_all(dir());
    for _ in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => return writeln!(f, "{}", std::process::id()).is_ok(),
            Err(_) if fs::read_to_string(&path).ok().and_then(|p| p.trim().parse().ok()).is_some_and(resume::running) => return false,
            Err(_) => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    false
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
    if !lock() {
        return 0;
    }
    let _ = resume::update_state(|s| {
        s.insert("runningPid".into(), json!(std::process::id()));
    });
    let (cfg, status) = (resume::settings(), resume::status());
    let names = plan(&cfg, &status["installed"]);
    if status["needsSudo"] == true {
        names.iter().for_each(|n| println!("- {n}"));
        println!("\nOmarchy update installs system packages");
        let _ = std::process::Command::new("sudo").arg("true").status();
    }
    let mut results: Vec<(&str, StepResult)> = names.iter().map(|n| (*n, step(n, &cfg))).collect();
    let reboot = reboot_required();
    if cfg["restartShell"] == true && !reboot && results.iter().any(|(n, r)| *n == "Omarchy plugins" && matches!(r, StepResult::Updated(_))) {
        let r = sh("Restart shell", &["omarchy", "restart", "shell"]).map_or_else(StepResult::Failed, |()| StepResult::Current);
        results.push(("Restart shell", r));
    }
    let updates: Vec<_> = results.iter().filter_map(|(_, r)| if let StepResult::Updated(u) = r { Some(u.clone()) } else { None }).flatten().collect();
    let failed: Vec<_> = results.iter().filter_map(|(n, r)| if let StepResult::Failed(e) = r { Some((*n, e.clone())) } else { None }).collect();
    let last_error = failed.first().map_or(Value::Null, |(n, e)| json!(format!("{n}: {e}")));
    let _ = resume::update_state(|s| {
        s.remove("runningPid");
        s.insert("lastError".into(), last_error);
    });
    let _ = fs::remove_file(dir().join("resume.lock"));
    if !updates.is_empty() || !failed.is_empty() || reboot {
        notify(&updates, &failed, reboot);
    }
    0
}
