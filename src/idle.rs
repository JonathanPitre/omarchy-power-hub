use crate::config::truthy;
use crate::monitor::{or_default, py_str, restore_lua};
use crate::names::{matches_set, safe_name_v, safe_output};
use crate::{hypr, paths};
use serde_json::{Map, Value};
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// A cursor older than this is from the last blank and would read as motion.
const CURSOR_FRESH: Duration = Duration::from_secs(5);

/// First readable `state` file under the ACPI lid directory; open when there is none.
pub fn lid_text() -> String {
    let dir = paths::root("/proc/acpi/button/lid");
    let mut names: Vec<_> = fs::read_dir(dir.as_path())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| e.path())
        .collect();
    names.sort();
    names
        .iter()
        .find_map(|p| fs::read(p.join("state")).ok())
        .map_or_else(|| "state:      open\n".into(), |b| String::from_utf8_lossy(&b).into_owned())
}

pub fn lid_is_open(text: &str) -> bool {
    let t = text.to_lowercase();
    !t.contains("closed") && t.contains("open")
}

/// The cursor stored by the previous poll, if recent; always stores the current one.
fn cursor_prev(cursor: &str) -> Option<Value> {
    let path = paths::plugin_run_dir().join("idle-wake.json");
    let fresh = fs::metadata(&path).and_then(|m| m.modified()).ok().filter(|mtime| {
        SystemTime::now().duration_since(*mtime).map_or(true, |age| age < CURSOR_FRESH)
    });
    let prev = fresh
        .and_then(|_| serde_json::from_str::<Value>(&fs::read_to_string(&path).ok()?).ok())
        .and_then(|v| v.get("cursor").cloned());
    let _ = fs::create_dir_all(paths::plugin_run_dir());
    let _ = fs::write(&path, format!("{{\"cursor\": {}}}\n", Value::String(cursor.into())));
    prev
}

/// `display_power.awake`: some monitor is enabled and every enabled one is lit.
fn awake(monitors: &[Value]) -> bool {
    let mut enabled = monitors.iter().filter(|m| !truthy(&m["disabled"])).peekable();
    enabled.peek().is_some() && enabled.all(|m| truthy(&m["dpmsStatus"]))
}

/// `display_power.partly_lit`: enabled monitors are a mix of lit and dark.
fn partly_lit(monitors: &[Value]) -> bool {
    let lit = |want: bool| monitors.iter().any(|m| !truthy(&m["disabled"]) && truthy(&m["dpmsStatus"]) == want);
    lit(true) && lit(false)
}

fn device_names(devices: &Value, key: &str, keep: impl Fn(&str) -> bool) -> Vec<String> {
    devices[key]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| if truthy(&row["name"]) { py_str(&row["name"]) } else { String::new() })
        .filter(|n| matches_set(n, |c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-')) && keep(n))
        .collect()
}

fn set_enabled(names: &[String], enabled: bool) {
    for name in names {
        hypr::eval(&format!(r#"hl.device({{ name = "{name}", enabled = {enabled} }})"#));
    }
}

/// One `idle-wake-poll`: `closed`, `wake`, `blank` or `hold`, with input devices enabled or not to match.
pub fn poll(blanked: bool) -> String {
    let cursor = hypr::cursor();
    let monitors = hypr::monitors();
    let devices = hypr::devices();
    let lid_text = lid_text();
    let lid_open = lid_is_open(&lid_text);
    let prev = cursor_prev(&cursor);
    let moved = prev.is_some_and(|p| !p.is_null() && p != Value::String(cursor));
    let result = if !lid_open {
        "closed"
    } else if moved || (blanked && awake(&monitors)) {
        "wake"
    } else if blanked && partly_lit(&monitors) {
        "blank"
    } else {
        "hold"
    };
    let enabled = result == "wake" || lid_open || !blanked;
    set_enabled(&device_names(&devices, "mice", |_| true), enabled);
    set_enabled(
        &device_names(&devices, "keyboards", |n| {
            let n = n.strip_suffix('\n').unwrap_or(n);
            !(n.contains("video-bus") || n.contains("gpio-keys") || n == "power-button")
        }),
        enabled,
    );
    result.into()
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

/// `displays sync` under the display-state lock: put parked heads back once every other one is lit.
/// None when the lock cannot be taken, as Python's `state_lock` raises: nothing is printed or touched.
pub fn sync() -> Option<String> {
    let dir = paths::plugin_run_dir();
    fs::create_dir_all(&dir).ok()?;
    let lock = OpenOptions::new().write(true).create(true).truncate(true).open(dir.join("display-off.lock")).ok()?;
    lock.lock().ok()?;
    Some(sync_locked(&dir.join("display-off.json")))
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
    use serde_json::json;

    #[test]
    fn lid_text_decides_open() {
        assert!(lid_is_open("state:      open\n") && !lid_is_open("state:      closed\n") && !lid_is_open("") && !lid_is_open("unknown"));
    }

    #[test]
    fn names_follow_python_str() {
        let names = [json!(5), json!(true), json!(false), json!(null), json!(""), json!(1.5), json!(["a"]), json!("a b"), json!("ok")];
        let d = json!({"mice": names.iter().map(|name| json!({ "name": name })).collect::<Vec<_>>()});
        assert_eq!(device_names(&d, "mice", |_| true), ["5", "True", "1.5", "ok"]);
    }
}
