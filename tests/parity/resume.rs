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
    s.write_stub("hypr-persist");
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
    assert!(!called(&o, "omarchy") && !called(&o, "sudo"));
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
    assert_eq!(o.calls.iter().filter(|c| *c == "sudo true").count(), 1);
    assert!(at(&o, "sudo true") < at(&o, "omarchy update -y"));
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
