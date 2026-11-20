//! `lights off|on`: the keyboard and power-button lights follow system sleep, called by the root sleep hook.
use crate::{config, idle, kblight, keyboard, paths};
use serde_json::Value;
use std::fs;

fn read_json(path: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn disabled() -> bool {
    read_json(&paths::shell_file()).is_some_and(|v| v["disabledPlugins"].as_array().is_some_and(|a| a.iter().any(|p| p == "omarchy-power-hub")))
}

pub fn lights_sleep(off: bool) {
    let path = paths::plugin_run_dir().join("kblight.json");
    let auto = keyboard::is_auto(&config::saved_prefs());
    if off {
        if disabled() || path.exists() {
            return;
        }
        kblight::sync(true, auto);
        if let Some(Value::Object(mut state)) = read_json(&path) {
            state.insert("sleep".into(), true.into());
            let _ = fs::write(&path, Value::Object(state).to_string() + "\n");
        }
    } else if read_json(&path).is_some_and(|v| v["sleep"] == true) && idle::lid_is_open(&idle::lid_text()) {
        kblight::sync(false, auto);
    }
}
