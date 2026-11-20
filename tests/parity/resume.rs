//! The on-resume settings, status and panel action.
use crate::read::j;
use crate::support::{Sandbox, executable};
use serde_json::{Value, json};
use std::fs;

const DEFAULTS: fn() -> Value = || json!({
    "triggers": ["wake"], "awayMinutes": 30, "everyHours": 4, "omarchy": true, "plugins": true, "leanCtx": true, "rulesync": true,
    "skillsManager": true, "t3code": "restart", "restartShell": true, "sessionRestore": true, "script": "",
});

fn status(s: &Sandbox) -> Value {
    j(&s.rust(&["resume-status"]))
}

#[test]
fn test_defaults_when_unset() {
    let s = Sandbox::new();
    assert_eq!(status(&s)["settings"], DEFAULTS());
}

#[test]
fn test_set_each_key() {
    let s = Sandbox::new();
    for (key, value, saved) in [
        ("triggers", "wake,unlock", json!(["wake", "unlock"])),
        ("awayMinutes", "45", json!(45)),
        ("everyHours", "2", json!(2)),
        ("t3code", "update", json!("update")),
        ("omarchy", "off", json!(false)),
        ("rulesync", "on", json!(true)),
        ("script", "/home/u/x.fish", json!("/home/u/x.fish")),
    ] {
        s.rust(&["action", "resume", key, value]);
        assert_eq!(s.read_config()["onResume"][key], saved, "{key}");
    }
    let settings = status(&s)["settings"].clone();
    assert_eq!((&settings["triggers"], &settings["omarchy"], &settings["script"]), (&json!(["wake", "unlock"]), &json!(false), &json!("/home/u/x.fish")));
}

#[test]
fn test_bad_values_fall_back_per_key() {
    let s = Sandbox::new();
    s.config(json!({"onResume": {"awayMinutes": -5, "triggers": ["nap"], "t3code": 7, "everyHours": 2}}));
    let settings = status(&s)["settings"].clone();
    assert_eq!((&settings["awayMinutes"], &settings["triggers"], &settings["t3code"], &settings["everyHours"]), (&json!(30), &json!(["wake"]), &json!("restart"), &json!(2)));
    for (key, value) in [("awayMinutes", "abc"), ("awayMinutes", "1441"), ("everyHours", "-1"), ("triggers", "nap"), ("t3code", "x"), ("omarchy", "maybe"), ("nope", "1")] {
        let before = s.read_config();
        s.rust(&["action", "resume", key, value]);
        assert_eq!(s.read_config(), before, "{key} {value}");
    }
}

#[test]
fn test_profile_edit_keeps_on_resume() {
    let s = Sandbox::new();
    s.config(json!({}));
    s.rust(&["action", "resume", "everyHours", "2"]);
    s.rust(&["set", "ac", "profile", "balanced"]);
    assert_eq!(s.read_config()["onResume"]["everyHours"], 2);
}

#[test]
fn test_installed_detection() {
    let s = Sandbox::new();
    let installed = |s: &Sandbox| status(s)["installed"].clone();
    let none = json!({"leanCtx": false, "rulesync": false, "skillsManager": false, "t3code": false, "hyprPersist": false});
    s.set_path(&s.stubs.display().to_string());
    assert_eq!(installed(&s), none);
    let lean = s.write("~/.local/bin/lean-ctx", "#!/bin/sh\n");
    executable(&lean, "#!/bin/sh\n");
    s.write("~/.local/share/t3code/T3-Code.AppImage", "");
    s.write_stub("skills-manager-refresh");
    s.stub("hypr-persist", "restore --help", 0, "restore <name> --class <regex>");
    s.stub("mise", "ls --json npm:rulesync", 0, r#"[{"version": "1.2.3"}]"#);
    assert_eq!(installed(&s), json!({"leanCtx": true, "rulesync": true, "skillsManager": true, "t3code": true, "hyprPersist": true}));
}

#[test]
fn test_rulesync_is_asked_once_an_hour() {
    let s = Sandbox::new();
    s.set_path(&s.stubs.display().to_string());
    s.stub("mise", "ls --json npm:rulesync", 0, r#"[{"version": "1.2.3"}]"#);
    s.rust(&["data"]);
    s.stub("mise", "ls --json npm:rulesync", 0, "[]");
    assert_eq!(j(&s.rust(&["data"]))["resume"]["installed"]["rulesync"], true);
    let state = s.target("~/.local/state/omarchy-power-hub/resume.json");
    let mut cached: Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
    cached["lastRun"] = json!(1);
    cached["rulesync"]["at"] = json!(1);
    fs::write(&state, cached.to_string()).unwrap();
    let after = j(&s.rust(&["data"]));
    assert_eq!(after["resume"]["installed"]["rulesync"], false);
    let kept: Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
    assert_eq!(kept["lastRun"], 1);
}

#[test]
fn test_needs_sudo() {
    let s = Sandbox::new();
    let sudo = |s: &Sandbox| status(s)["needsSudo"].clone();
    assert_eq!(sudo(&s), true);
    s.rust(&["action", "resume", "script", "/x"]);
    assert_eq!(sudo(&s), false);
    s.rust(&["action", "resume", "script", ""]);
    s.rust(&["action", "resume", "omarchy", "off"]);
    assert_eq!(sudo(&s), false);
    s.write_stub("skills-manager-refresh");
    assert_eq!(sudo(&s), true);
    s.rust(&["action", "resume", "skillsManager", "off"]);
    assert_eq!(sudo(&s), false);
}

#[test]
fn test_rulesync_ignores_stderr() {
    let s = Sandbox::new();
    executable(&s.stubs.join("mise"), "#!/bin/sh\necho warning >&2\n");
    assert_eq!(status(&s)["installed"]["rulesync"], false);
}

#[test]
fn test_failing_mise_is_asked_once_an_hour() {
    let s = Sandbox::new();
    s.stub("mise", "", 1, "");
    let asked = |s: &Sandbox| s.rust(&["data"]).calls.iter().filter(|c| c.starts_with("mise")).count();
    assert_eq!((asked(&s), asked(&s)), (1, 0));
}

/// Runs `resume-run`, then waits for the detached dispatch (or the lack of one) to show in the calls log.
fn dispatched(s: &Sandbox, argv: &[&str], expect: bool) -> Vec<String> {
    s.stub("uwsm-app", "", 0, "");
    let argv: Vec<&str> = [&["resume-run"], argv].concat();
    assert_eq!(s.rust(&argv).code, 0);
    for _ in 0..if expect { 100 } else { 10 } {
        if expect && !fs::read_to_string(&s.calls_log).unwrap().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    fs::read_to_string(&s.calls_log).unwrap().lines().map(String::from).collect()
}

fn state(s: &Sandbox, v: Value) {
    s.write("~/.local/state/omarchy-power-hub/resume.json", &v.to_string());
}

fn bin(s: &Sandbox) -> String {
    format!("{}/power-hub", s.bin.display())
}

const TERMINAL: &str = "uwsm-app -- xdg-terminal-exec --app-id=org.omarchy.terminal --title=Stack Refresh -e";

#[test]
fn test_source_not_selected_skips() {
    let s = Sandbox::new();
    assert!(dispatched(&s, &["--source", "unlock", "--away", "9999"], false).is_empty());
}

#[test]
fn test_away_threshold_only_for_displays_and_unlock() {
    let s = Sandbox::new();
    s.config(json!({"onResume": {"triggers": ["wake", "displays"]}}));
    assert!(dispatched(&s, &["--source", "displays", "--away", "600"], false).is_empty());
    assert_eq!(dispatched(&s, &["--source", "displays", "--away", "1800"], true).len(), 1);
    state(&s, json!({}));
    assert_eq!(dispatched(&s, &["--source", "wake", "--away", "0"], true).len(), 1);
}

#[test]
fn test_every_hours_limit() {
    let s = Sandbox::new();
    state(&s, json!({"lastRun": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - 3600}));
    assert!(dispatched(&s, &["--source", "wake", "--away", "0"], false).is_empty());
    assert_eq!(dispatched(&s, &["--now"], true).len(), 1);
}

#[test]
fn test_running_pid_alive_skips() {
    let s = Sandbox::new();
    let mut run = std::process::Command::new("sh").args(["-c", "sleep 30; true", "resume-run"]).spawn().unwrap();
    state(&s, json!({"runningPid": run.id()}));
    assert!(dispatched(&s, &["--source", "wake", "--away", "0"], false).is_empty());
    assert!(dispatched(&s, &["--now"], false).is_empty());
    run.kill().unwrap();
    run.wait().unwrap();
    state(&s, json!({"runningPid": 4_000_000}));
    assert_eq!(dispatched(&s, &["--source", "wake", "--away", "0"], true).len(), 1);
    state(&s, json!({"runningPid": std::process::id()}));
    assert_eq!(dispatched(&s, &["--now"], true).len(), 1);
}

#[test]
fn test_second_trigger_right_behind_is_too_soon() {
    let s = Sandbox::new();
    assert_eq!(dispatched(&s, &["--source", "wake", "--away", "0"], true).len(), 1);
    assert!(dispatched(&s, &["--source", "wake", "--away", "0"], false).is_empty());
}

#[test]
fn test_background_dispatch() {
    let s = Sandbox::new();
    s.config(json!({"onResume": {"omarchy": false}}));
    assert_eq!(dispatched(&s, &["--now"], true), [format!("uwsm-app -- {} resume-run --exec", bin(&s))]);
}

#[test]
fn test_terminal_dispatch() {
    let s = Sandbox::new();
    assert_eq!(dispatched(&s, &["--source", "wake", "--away", "0"], true), [format!("{TERMINAL} {} resume-run --exec", bin(&s))]);
}

#[test]
fn test_resume_now_action_returns_data_and_dispatches() {
    let s = Sandbox::new();
    s.stub("uwsm-app", "", 0, "");
    fs::create_dir_all(&s.bin).unwrap();
    std::os::unix::fs::symlink(crate::support::RUST_BIN, s.bin.join("power-hub")).unwrap();
    let out = s.rust(&["action", "resume-now"]);
    assert!(j(&out).get("resume").is_some());
    for _ in 0..100 {
        if !fs::read_to_string(&s.calls_log).unwrap().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(fs::read_to_string(&s.calls_log).unwrap().contains("resume-run --exec"));
}

/// Answers `probe` with `before` the first time and `after` from then on; any other call exits `other`.
fn versioned(s: &Sandbox, tool: &str, probe: &str, before: &str, after: &str, other: i32) {
    let n = s.calls_log.with_file_name(format!("{tool}.n")).display().to_string();
    s.stateful(tool, &format!("case \"$*\" in\n  *{probe}*) c=$(cat {n} 2>/dev/null || echo 0); echo $((c+1)) > {n}; [ \"$c\" = 0 ] && echo '{before}' || echo '{after}';;\n  *) exit {other};;\nesac"));
}

/// Every tool installed and succeeding with unchanged versions.
fn ready(s: &Sandbox) {
    executable(&s.write("~/.local/bin/lean-ctx", ""), "#!/bin/sh\n");
    s.stub("mise", "ls --json npm:rulesync", 0, r#"[{"version": "1"}]"#);
    s.stub("mise", "up npm:rulesync", 0, "");
    s.stub("lean-ctx", "update", 0, "");
    s.stub("lean-ctx", "--version", 0, "lean-ctx 1.0.0");
    for (tool, pattern) in [("omarchy", "update -y"), ("omarchy", "plugin update --yes"), ("omarchy", "restart shell"), ("sudo", "true"), ("notify-send", ""), ("skills-manager-refresh", ""), ("omarchy-system", "reboot")] {
        s.stub(tool, pattern, 0, "");
    }
}

fn exec(s: &Sandbox) -> crate::support::Outcome {
    s.rust(&["resume-run", "--exec"])
}

fn at(o: &crate::support::Outcome, call: &str) -> usize {
    o.calls.iter().position(|c| c.starts_with(call)).unwrap_or_else(|| panic!("no {call:?} in {:?}", o.calls))
}

fn called(o: &crate::support::Outcome, call: &str) -> bool {
    o.calls.iter().any(|c| c.starts_with(call))
}

fn log(s: &Sandbox) -> String {
    fs::read_to_string(s.target("~/.local/state/omarchy-power-hub/resume.log")).unwrap_or_default()
}

fn plugin(s: &Sandbox) {
    s.write("~/.config/omarchy/plugins/p/manifest.json", "");
}

#[test]
fn test_step_order_with_defaults() {
    let s = Sandbox::new();
    ready(&s);
    let o = exec(&s);
    assert!(at(&o, "omarchy update -y") < at(&o, "omarchy plugin update --yes") && at(&o, "omarchy plugin update --yes") < at(&o, "lean-ctx update"));
    assert!(!called(&o, "mise up") && !called(&o, "skills-manager-refresh"));
    let state = fs::read_to_string(s.target("~/.local/state/omarchy-power-hub/resume.json")).unwrap_or_default();
    assert!(!state.contains("runningPid\": "), "{state}");
    assert!(!s.target("~/.local/state/omarchy-power-hub/resume.lock").exists());
}

#[test]
fn test_rulesync_and_skills_run_alone_when_omarchy_off() {
    let s = Sandbox::new();
    ready(&s);
    s.config(json!({"onResume": {"omarchy": false, "plugins": false, "leanCtx": false}}));
    let o = exec(&s);
    assert!(at(&o, "mise up npm:rulesync") < at(&o, "skills-manager-refresh"));
    assert!(!called(&o, "omarchy") && at(&o, "sudo true") < at(&o, "skills-manager-refresh"));
    assert!(o.stdout.contains("Skills Manager installs a system package") && !o.stdout.contains("Omarchy update"));
}

#[test]
fn test_failed_sudo_skips_the_steps_that_need_it() {
    let s = Sandbox::new();
    ready(&s);
    s.config(json!({"onResume": {"omarchy": false}}));
    s.stub("sudo", "true", 1, "");
    let o = exec(&s);
    assert!(!called(&o, "skills-manager-refresh") && called(&o, "omarchy plugin update") && called(&o, "mise up npm:rulesync"));
    assert!(o.calls[at(&o, "notify-send")].contains("Skills Manager failed"));
}

/// `omarchy update` re-execs under `script`, a new tty whose sudo ticket is its own, so it asks for the password itself.
#[test]
fn test_terminal_step_gets_the_terminal() {
    let s = Sandbox::new();
    ready(&s);
    s.stateful("omarchy", "case \"$*\" in\n  \"update -y\") read -r pw; printf 'got %s' \"$pw\";;\nesac");
    s.stateful("timeout", "[ \"$1\" = --foreground ] && shift; shift; exec \"$@\"");
    let mut c = std::process::Command::new(crate::support::RUST_BIN);
    c.args(["resume-run", "--exec"]).env_clear().envs(s.env()).current_dir(&s.base).stdin(std::process::Stdio::piped());
    let mut p = crate::support::spawn(&mut c);
    std::io::Write::write_all(&mut p.stdin.take().unwrap(), b"hunter2\n").unwrap();
    let out = String::from_utf8_lossy(&p.wait_with_output().unwrap().stdout).into_owned();
    let calls = fs::read_to_string(&s.calls_log).unwrap();
    assert!(out.contains("got hunter2") && log(&s).contains("got hunter2"), "{out}");
    assert!(calls.contains("timeout --foreground 7200 omarchy update -y") && !calls.contains("sudo true"), "{calls}");
}

/// A prompt without a newline shows at once, and a byte that is not UTF-8 neither ends the echo nor stalls the step on a full pipe.
#[test]
fn test_terminal_output_streams_raw_bytes() {
    let s = Sandbox::new();
    ready(&s);
    s.stateful("omarchy", "case \"$*\" in\n  \"update -y\") printf 'a\\377b\\n'; head -c 200000 /dev/zero | tr '\\0' x; printf 'Password: ';;\nesac");
    let o = exec(&s);
    assert!(o.stdout.contains(&format!("a\u{FFFD}b\n{}Password: ", "x".repeat(200000))) && called(&o, "lean-ctx update"));
    let raw = fs::read(s.target("~/.local/state/omarchy-power-hub/resume.log")).unwrap();
    assert!(raw.windows(4).any(|w| w == b"a\xffb\n") && String::from_utf8_lossy(&raw).contains("Password: \n"));
}

#[test]
fn test_terminal_run_shows_step_output() {
    let s = Sandbox::new();
    ready(&s);
    s.stub("omarchy", "update -y", 0, "installing the things");
    assert!(exec(&s).stdout.contains("installing the things"));
    s.config(json!({"onResume": {"omarchy": false, "skillsManager": false, "leanCtx": false}}));
    s.stub("mise", "up npm:rulesync", 0, "quiet run");
    assert!(!exec(&s).stdout.contains("quiet run"));
}

#[test]
fn test_failure_does_not_stop_later_steps() {
    let s = Sandbox::new();
    ready(&s);
    s.config(json!({"onResume": {"omarchy": false}}));
    s.stub("lean-ctx", "update", 1, "boom");
    let o = exec(&s);
    assert!(at(&o, "lean-ctx update") < at(&o, "mise up npm:rulesync") && called(&o, "skills-manager-refresh"));
    let note = &o.calls[at(&o, "notify-send")];
    assert!(note.contains("lean-ctx") && note.contains("resume.log"), "{note}");
    assert!(log(&s).contains("boom"));
    assert!(fs::read_to_string(s.target("~/.local/state/omarchy-power-hub/resume.json")).unwrap().contains("lean-ctx"));
}

#[test]
fn test_nothing_changed_no_notification() {
    let s = Sandbox::new();
    ready(&s);
    assert!(!called(&exec(&s), "notify-send"));
}

#[test]
fn test_updates_listed_from_to() {
    let s = Sandbox::new();
    ready(&s);
    versioned(&s, "pacman", "-Q", "omarchy 4.5.0-3", "omarchy 4.5.1-1", 1);
    let note = exec(&s);
    assert!(note.calls[at(&note, "notify-send")].contains("omarchy 4.5.0-3 → 4.5.1-1"), "{:?}", note.calls);
}

#[test]
fn test_shell_restart_only_after_plugin_change() {
    let changed = Sandbox::new();
    ready(&changed);
    plugin(&changed);
    versioned(&changed, "git", "rev-parse", "aaa", "bbb", 1);
    let o = exec(&changed);
    assert!(called(&o, "omarchy restart shell") && o.calls[at(&o, "notify-send")].contains("p aaa → bbb"));
    let same = Sandbox::new();
    ready(&same);
    plugin(&same);
    versioned(&same, "git", "rev-parse", "aaa", "aaa", 1);
    assert!(!called(&exec(&same), "omarchy restart shell"));
    let omarchy_only = Sandbox::new();
    ready(&omarchy_only);
    versioned(&omarchy_only, "pacman", "-Q", "omarchy 1-1", "omarchy 2-1", 1);
    assert!(!called(&exec(&omarchy_only), "omarchy restart shell"));
}

#[test]
fn test_reboot_required_skips_restart_and_session_restore() {
    let s = Sandbox::new();
    ready(&s);
    plugin(&s);
    versioned(&s, "git", "rev-parse", "aaa", "bbb", 1);
    s.write("~/.local/state/omarchy/reboot-required", "");
    let o = exec(&s);
    assert!(!called(&o, "omarchy restart shell") && !called(&o, "omarchy-system"));
    assert!(o.calls[at(&o, "notify-send")].contains("-A reboot=Reboot"));
    s.stub("notify-send", "", 0, "reboot");
    assert!(called(&exec(&s), "omarchy-system reboot"));
}

#[test]
fn test_reboot_checks() {
    let s = Sandbox::new();
    ready(&s);
    s.stub("pacman", "-Qo", 0, "");
    s.write("/usr/lib/modules/9.9.9/modules.builtin", "");
    s.write("/proc/sys/kernel/osrelease", "9.9.9\n");
    assert!(!called(&exec(&s), "notify-send"));
    s.write("/proc/sys/kernel/osrelease", "1.0.0\n");
    assert!(exec(&s).calls.iter().any(|c| c.starts_with("notify-send") && c.contains("-A reboot=Reboot")));
    s.write("/proc/sys/kernel/osrelease", "9.9.9\n");
    s.stub("pgrep", "-x Hyprland", 0, "123");
    fs::create_dir_all(s.target("/proc/123")).unwrap();
    std::os::unix::fs::symlink("/usr/bin/Hyprland (deleted)", s.target("/proc/123/exe")).unwrap();
    assert!(exec(&s).calls.iter().any(|c| c.starts_with("notify-send") && c.contains("-A reboot=Reboot")));
}

#[test]
fn test_custom_script_runs_alone() {
    let s = Sandbox::new();
    ready(&s);
    s.stub("myscript", "", 0, "");
    s.config(json!({"onResume": {"script": s.stubs.join("myscript")}}));
    let o = exec(&s);
    assert_eq!(o.calls.iter().filter(|c| !c.starts_with("mise ls")).collect::<Vec<_>>(), ["myscript "]);
}

#[test]
fn test_missing_script_reports_failure() {
    let s = Sandbox::new();
    ready(&s);
    s.config(json!({"onResume": {"script": "/nonexistent/x.sh"}}));
    let o = exec(&s);
    assert!(o.calls[at(&o, "notify-send")].contains("failed"));
    assert!(log(&s).contains("/nonexistent/x.sh"));
    assert!(!called(&o, "omarchy"));
}

#[test]
fn test_terminal_header() {
    let s = Sandbox::new();
    ready(&s);
    let o = exec(&s);
    let (plan, reason) = (o.stdout.find("lean-ctx").unwrap(), o.stdout.find("Omarchy update installs system packages").unwrap());
    assert!(o.stdout.find("Omarchy").unwrap() < reason && plan < reason);
    assert!(!called(&o, "sudo true") && called(&o, "omarchy update -y"));
}

#[test]
fn test_exec_lock_blocks_second_exec() {
    let s = Sandbox::new();
    ready(&s);
    let mut holder = std::process::Command::new("sh").args(["-c", "sleep 30; true", "resume-run"]).spawn().unwrap();
    s.write("~/.local/state/omarchy-power-hub/resume.lock", &holder.id().to_string());
    assert!(exec(&s).calls.iter().all(|c| c.starts_with("mise ls")));
    assert!(s.target("~/.local/state/omarchy-power-hub/resume.lock").exists());
    holder.kill().unwrap();
    holder.wait().unwrap();
    s.write("~/.local/state/omarchy-power-hub/resume.lock", "4000000");
    assert!(called(&exec(&s), "omarchy update -y"));
    assert!(!s.target("~/.local/state/omarchy-power-hub/resume.lock").exists());
}

const CURRENT: &str = "version: 1\npath: T3-Code-1.AppImage\nsha512: AAA\n";
const NIGHTLY: &str = "v0.1.11-nightly.20261119.4941";

/// Only the T3 Code step is on; the feed's hash is `AAA` and the installed file hashes to `hash`.
fn t3(s: &Sandbox, mode: &str, hash: &str) {
    ready(s);
    s.write("~/.local/share/t3code/T3-Code.AppImage", "");
    s.config(json!({"onResume": {"omarchy": false, "plugins": false, "leanCtx": false, "rulesync": false, "skillsManager": false, "t3code": mode}}));
    s.write("~/.t3/userdata/desktop-settings.json", r#"{"updateChannel":"latest"}"#);
    s.stub("curl", "latest-linux.yml", 0, CURRENT);
    s.stub("openssl", "", 0, "");
    s.stub("base64", "", 0, hash);
    s.stub("omarchy-install-t3code-app", "", 0, "");
    s.stub("uwsm-app", "", 0, "");
    s.stub("t3", "--version", 0, "t3 v1");
}

/// Waits for the detached restart to show in the calls log, or for the absence of one.
fn spawned(s: &Sandbox, expect: bool) -> Option<String> {
    for _ in 0..if expect { 100 } else { 10 } {
        let log = fs::read_to_string(&s.calls_log).unwrap();
        if let Some(l) = log.lines().find(|l| l.starts_with("uwsm-app")) {
            return Some(l.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

#[test]
fn test_channel_from_settings_then_version() {
    let s = Sandbox::new();
    t3(&s, "update", "AAA");
    assert!(called(&exec(&s), "curl -fsSL --max-time 30 https://github.com/pingdotgg/t3code/releases/latest/download/latest-linux.yml"));
    fs::remove_file(s.target("~/.t3/userdata/desktop-settings.json")).unwrap();
    s.stub("t3", "--version", 0, &format!("t3 {NIGHTLY}"));
    s.stub("curl", "api.github.com", 0, &format!(r#"[{{"draft":true,"tag_name":"v9.9.9-nightly.1.1"}},{{"draft":false,"tag_name":"v0.2.0"}},{{"draft":false,"tag_name":"{NIGHTLY}"}}]"#));
    s.stub("curl", "nightly-linux.yml", 0, CURRENT);
    let o = exec(&s);
    assert!(called(&o, &format!("curl -fsSL --max-time 30 https://github.com/pingdotgg/t3code/releases/download/{NIGHTLY}/nightly-linux.yml")), "{:?}", o.calls);
}

#[test]
fn test_matching_hash_skips_install() {
    let s = Sandbox::new();
    t3(&s, "restart", "AAA");
    let o = exec(&s);
    assert!(!called(&o, "omarchy-install-t3code-app") && !called(&o, "notify-send"));
    assert!(spawned(&s, false).is_none());
}

#[test]
fn test_new_release_runs_installer_for_channel() {
    let s = Sandbox::new();
    t3(&s, "update", "BBB");
    s.write("~/.t3/userdata/desktop-settings.json", r#"{"updateChannel":"nightly"}"#);
    s.stub("curl", "api.github.com", 0, &format!(r#"[{{"draft":false,"tag_name":"{NIGHTLY}"}}]"#));
    s.stub("curl", "nightly-linux.yml", 0, CURRENT);
    versioned(&s, "t3", "--version", "t3 v1", "t3 v2", 1);
    let o = exec(&s);
    assert!(called(&o, "omarchy-install-t3code-app nightly"));
    assert!(o.calls[at(&o, "notify-send")].contains("T3 Code v1 → v2"), "{:?}", o.calls);
}

#[test]
fn test_feed_unreachable_fails_the_step() {
    let s = Sandbox::new();
    t3(&s, "update", "BBB");
    s.stub("curl", "latest-linux.yml", 22, "");
    let o = exec(&s);
    assert!(!called(&o, "omarchy-install-t3code-app") && o.calls[at(&o, "notify-send")].contains("T3 Code failed"));
    assert!(fs::read_to_string(s.target("~/.local/state/omarchy-power-hub/resume.json")).unwrap().contains("T3 Code: T3 Code feed unreachable"));
}

#[test]
fn test_unknown_version_fails_the_step() {
    let s = Sandbox::new();
    t3(&s, "update", "BBB");
    s.stub("t3", "--version", 1, "");
    let o = exec(&s);
    assert!(!called(&o, "omarchy-install-t3code-app") && o.calls[at(&o, "notify-send")].contains("T3 Code failed"));
}

#[test]
fn test_update_only_never_restarts() {
    let s = Sandbox::new();
    t3(&s, "update", "BBB");
    versioned(&s, "t3", "--version", "t3 v1", "t3 v2", 1);
    let o = exec(&s);
    assert!(called(&o, "omarchy-install-t3code-app stable") && !called(&o, "pkill"));
    assert!(spawned(&s, false).is_none());
}

#[test]
fn test_restart_is_spawned_detached_after_an_install() {
    let s = Sandbox::new();
    t3(&s, "restart", "BBB");
    s.stub("hypr-persist", "restore --help", 0, "--class");
    versioned(&s, "t3", "--version", "t3 v1", "t3 v2", 1);
    exec(&s);
    assert_eq!(spawned(&s, true).unwrap(), format!("uwsm-app -- {} resume-run --t3-restart --restore", bin(&s)));
    assert!(!s.target("~/.local/state/omarchy-power-hub/resume.lock").exists());
}

#[test]
fn test_restart_skips_restore_when_off_or_missing() {
    let off = Sandbox::new();
    t3(&off, "restart", "BBB");
    off.stub("hypr-persist", "", 0, "");
    off.config(json!({"onResume": {"omarchy": false, "plugins": false, "leanCtx": false, "t3code": "restart", "sessionRestore": false}}));
    versioned(&off, "t3", "--version", "t3 v1", "t3 v2", 1);
    exec(&off);
    assert_eq!(spawned(&off, true).unwrap(), format!("uwsm-app -- {} resume-run --t3-restart", bin(&off)));
    let missing = Sandbox::new();
    t3(&missing, "restart", "BBB");
    versioned(&missing, "t3", "--version", "t3 v1", "t3 v2", 1);
    exec(&missing);
    assert_eq!(spawned(&missing, true).unwrap(), format!("uwsm-app -- {} resume-run --t3-restart", bin(&missing)));
    let o = missing.rust(&["resume-run", "--t3-restart"]);
    assert!(!called(&o, "hypr-persist"));
}

#[test]
fn test_reboot_required_skips_t3_restart() {
    let s = Sandbox::new();
    t3(&s, "restart", "BBB");
    versioned(&s, "t3", "--version", "t3 v1", "t3 v2", 1);
    s.write("~/.local/state/omarchy/reboot-required", "");
    exec(&s);
    assert!(spawned(&s, false).is_none());
}

/// A T3 server whose first thread list shows a running thread, and later ones an old idle one.
fn busy_then_idle(s: &Sandbox) {
    s.stub("sqlite3", "", 0, "p1\n");
    let n = s.calls_log.with_file_name("t3.n").display().to_string();
    s.stateful("t3", &format!("case \"$*\" in\n  *\"thread list\"*) c=$(cat {n} 2>/dev/null || echo 0); echo $((c+1)) > {n}\n    if [ \"$c\" = 0 ]; then echo '[{{\"status\":\"running\",\"updatedAt\":\"2020-01-01T00:00:00.000Z\"}}]'; else echo '[{{\"status\":\"completed\",\"updatedAt\":\"2020-01-01T00:00:00.000Z\"}}]'; fi;;\n  *) exit 1;;\nesac"));
    for tool in ["notify-send", "hypr-persist", "uwsm-app"] {
        s.stub(tool, "", 0, "");
    }
}

#[test]
fn test_restart_waits_for_idle() {
    let s = Sandbox::new();
    busy_then_idle(&s);
    let o = s.rust(&["resume-run", "--t3-restart", "--restore"]);
    assert_eq!(o.code, 0, "{:?}", o.calls);
    let order = ["notify-send -a Power Hub T3 Code update ready, restarting when idle", "hypr-persist save power-hub-t3", "pkill -x t3code", &format!("uwsm-app -- {}/.local/share/t3code/T3-Code.AppImage", s.home.display()), "hypr-persist restore power-hub-t3 --class ^t3code$"];
    let at: Vec<_> = order.iter().map(|c| o.calls.iter().position(|l| l.starts_with(c)).unwrap_or_else(|| panic!("no {c:?} in {:?}", o.calls))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{:?}", o.calls);
    assert_eq!(o.calls.iter().filter(|c| c.starts_with("t3 thread list")).count(), 2);
}

#[test]
fn test_idle_check_failure_never_restarts() {
    let s = Sandbox::new();
    busy_then_idle(&s);
    s.stub("sqlite3", "", 1, "");
    let o = s.rust(&["resume-run", "--t3-restart", "--restore"]);
    assert!(!called(&o, "pkill") && !called(&o, "hypr-persist") && !called(&o, "uwsm-app"), "{:?}", o.calls);
    let notes: Vec<_> = o.calls.iter().filter(|c| c.starts_with("notify-send")).collect();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("T3 Code updated, restart it to finish"));
}

#[test]
fn test_never_idle_gives_up_and_never_restarts() {
    let s = Sandbox::new();
    busy_then_idle(&s);
    s.stub("t3", "thread list", 0, r#"[{"status":"running","updatedAt":"2020-01-01T00:00:00.000Z"}]"#);
    let o = s.rust(&["resume-run", "--t3-restart"]);
    assert!(!called(&o, "pkill") && o.calls.iter().any(|c| c.starts_with("notify-send") && c.contains("restart it to finish")), "{:?}", o.calls);
}

#[test]
fn test_session_restore_off_or_missing() {
    let s = Sandbox::new();
    busy_then_idle(&s);
    let o = s.rust(&["resume-run", "--t3-restart"]);
    assert!(called(&o, "pkill -x t3code") && !called(&o, "hypr-persist"), "{:?}", o.calls);
}

#[test]
fn test_status_carries_the_triggers_the_service_polls_for() {
    let s = Sandbox::new();
    assert_eq!(status(&s)["settings"]["triggers"], json!(["wake"]));
    s.config(json!({"onResume": {"triggers": ["unlock"]}}));
    assert_eq!(status(&s)["settings"]["triggers"], json!(["unlock"]));
}

#[test]
fn test_hypr_persist_needs_the_class_filter() {
    let s = Sandbox::new();
    s.stub("hypr-persist", "restore --help", 0, "usage: hypr-persist restore <name>");
    assert_eq!(status(&s)["installed"]["hyprPersist"], false);
    let s = Sandbox::new();
    s.stub("hypr-persist", "restore --help", 0, "usage: hypr-persist restore <name> --class <regex>");
    assert_eq!(status(&s)["installed"]["hyprPersist"], true);
}
