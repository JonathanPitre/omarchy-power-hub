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
    state(&s, json!({"runningPid": std::process::id()}));
    assert!(dispatched(&s, &["--source", "wake", "--away", "0"], false).is_empty());
    assert!(dispatched(&s, &["--now"], false).is_empty());
    state(&s, json!({"runningPid": 4_000_000}));
    assert_eq!(dispatched(&s, &["--source", "wake", "--away", "0"], true).len(), 1);
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
