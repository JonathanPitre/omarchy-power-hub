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
    s.stub("mise", "ls npm:rulesync", 0, "npm:rulesync  1.2.3\n");
    assert_eq!(installed(&s), json!({"leanCtx": true, "rulesync": true, "skillsManager": true, "t3code": true, "hyprPersist": true}));
}

#[test]
fn test_rulesync_is_asked_once_an_hour() {
    let s = Sandbox::new();
    s.set_path(&s.stubs.display().to_string());
    s.stub("mise", "ls npm:rulesync", 0, "npm:rulesync  1.2.3\n");
    s.rust(&["data"]);
    s.stub("mise", "ls npm:rulesync", 0, "");
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
