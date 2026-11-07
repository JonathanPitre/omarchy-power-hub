use crate::config::truthy;
use crate::monitor::{or_default, restore_lua};
use crate::names::{safe_name_v, safe_output};
use crate::{hypr, paths};
use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::path::Path;

/// `display_power.awake`: some monitor is enabled and every enabled one is lit.
pub fn awake(monitors: &[Value]) -> bool {
    let mut enabled = monitors.iter().filter(|m| !truthy(&m["disabled"])).peekable();
    enabled.peek().is_some() && enabled.all(|m| truthy(&m["dpmsStatus"]))
}

/// `display_power.partly_lit`: enabled monitors are a mix of lit and dark.
pub fn partly_lit(monitors: &[Value]) -> bool {
    let lit = |want: bool| monitors.iter().any(|m| !truthy(&m["disabled"]) && truthy(&m["dpmsStatus"]) == want);
    lit(true) && lit(false)
}

/// `display_power.blanked`: some enabled monitor is dark.
#[allow(dead_code)] // consumed by the display, lid and light paths in the next tasks
pub fn blanked(monitors: &[Value]) -> bool {
    monitors.iter().any(|m| !truthy(&m["disabled"]) && !truthy(&m["dpmsStatus"]))
}

/// `display_power.load_state`, hardened: always an object whose `disabled` is an array.
#[allow(dead_code)] // consumed by the display, lid and light paths in the next tasks
pub fn load_state(path: &Path) -> Value {
    let mut state = fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    if !state["disabled"].is_array() {
        state["disabled"] = json!([]);
    }
    state
}

fn restore(state: &Value) {
    for spec in state["disabled"].as_array().into_iter().flatten() {
        let spec = match spec {
            Value::Object(_) => spec.clone(),
            other => Value::Object(Map::from_iter([("name".into(), other.clone())])),
        };
        if safe_name_v(&spec["name"]) && safe_output(&or_default(&spec["output"], spec["name"].clone())) {
            hypr::eval(&restore_lua(&spec));
        }
    }
}

/// Runs `f` holding the display-state lock; None when the lock cannot be taken, as Python's `state_lock` raises.
pub fn with_state_lock<T>(f: impl FnOnce() -> T) -> Option<T> {
    let dir = paths::plugin_run_dir();
    fs::create_dir_all(&dir).ok()?;
    let lock = OpenOptions::new().write(true).create(true).truncate(true).open(dir.join("display-off.lock")).ok()?;
    lock.lock().ok()?;
    Some(f())
}

/// `displays sync` under the display-state lock: put parked heads back once every other one is lit.
/// None when the lock cannot be taken: nothing is printed or touched.
pub fn sync() -> Option<String> {
    with_state_lock(|| sync_locked(&paths::plugin_run_dir().join("display-off.json")))
}

fn sync_locked(state_path: &Path) -> String {
    let state = fs::read_to_string(state_path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_default();
    if !truthy(&state["disabled"]) {
        return "idle".into();
    }
    if !awake(&hypr::monitors()) {
        return "waiting".into();
    }
    restore(&state);
    let _ = fs::write(state_path, "{\"disabled\": []}\n");
    "restored".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_of_any_shape_has_a_list() {
        let path = std::env::temp_dir().join(format!("ph-state-{}.json", std::process::id()));
        for text in ["[]", "5", "\"x\"", "{\"disabled\": 1}", "nope"] {
            fs::write(&path, text).unwrap();
            assert_eq!(load_state(&path), json!({"disabled": []}), "{text}");
        }
        fs::write(&path, "{\"disabled\": [\"a\"]}").unwrap();
        assert_eq!(load_state(&path), json!({"disabled": ["a"]}));
        let _ = fs::remove_file(&path);
        assert_eq!(load_state(&path), json!({"disabled": []}));
    }
}
