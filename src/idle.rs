use crate::config::truthy;
use crate::display::{awake, partly_lit};
use crate::monitor::py_str;
use crate::names::matches_set;
use crate::{hypr, paths};
use serde_json::Value;
use std::fs;
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
    set_inputs(&devices, result == "wake" || lid_open || !blanked);
    result.into()
}

/// Enable or disable every mouse and every keyboard that is not a lid, power or video-bus switch.
pub fn set_inputs(devices: &Value, enabled: bool) {
    set_enabled(&device_names(devices, "mice", |_| true), enabled);
    set_enabled(
        &device_names(devices, "keyboards", |n| {
            let n = n.strip_suffix('\n').unwrap_or(n);
            !(n.contains("video-bus") || n.contains("gpio-keys") || n == "power-button")
        }),
        enabled,
    );
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
