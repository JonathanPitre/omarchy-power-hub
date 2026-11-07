use crate::config::{self, truthy};
use crate::display::{awake, blanked, load_state, partly_lit, set_displays, state_path, sync_lights, with_state_lock};
use crate::monitor::py_str;
use crate::names::matches_set;
use crate::{charge, exec, hypr, paths, power};
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

/// The active side's lid action; a config without one acts as `nothing`.
fn lid_action(cfg: &Value) -> &str {
    cfg[power::active_side(cfg)]["lid"].as_str().unwrap_or("nothing")
}

/// Lid switch closed. Inputs go off first so a bump cannot wake what is about to sleep.
/// Prints nothing and exits 0; when the display-state lock cannot be taken, `display-off` touches nothing.
pub fn lid_closed() {
    charge::privileged(&["hid-wakeup", "off"]);
    set_inputs(&hypr::devices(), false);
    let cfg = config::load();
    let systemctl = |verb: &str| exec::run(&["systemctl", verb], Duration::from_secs(8)).0;
    match lid_action(&cfg) {
        "display-off" => {
            if with_state_lock(|| set_displays(true, false)).is_some() {
                sync_lights(true);
            }
        }
        "suspend" => {
            systemctl("suspend");
        }
        "hibernate" if systemctl("hibernate") != 0 => {
            systemctl("suspend");
        }
        _ => {}
    }
}

/// Lid switch opened. Inputs come back before the displays, so a failed display step never leaves them dead.
/// Prints nothing and exits 0; when the display-state lock cannot be taken, the displays stay as they are.
pub fn lid_opened() {
    charge::privileged(&["hid-wakeup", "on"]);
    set_inputs(&hypr::devices(), true);
    // Key presses re-enable DPMS but not heads that Displays off disabled.
    let locked = with_state_lock(|| {
        let parked = load_state(&state_path())["disabled"].as_array().is_some_and(|d| !d.is_empty());
        if parked || blanked(&hypr::monitors_within(hypr::WRITE_WAIT)) {
            set_displays(false, true);
        }
    });
    // Lit displays can still have blanked lights; this does nothing when none were saved.
    if locked.is_some() {
        sync_lights(false);
    }
}

fn stay_awake() -> bool {
    paths::home().join(".local/state/omarchy/indicators/stay-awake").exists()
}

/// Closing the saver for Displays off reads to Omarchy as the user returning,
/// which cancels its lock countdown, so the plugin keeps that deadline itself.
pub fn idle_lock() {
    if !stay_awake() {
        exec::run(&["omarchy-system-lock"], Duration::from_secs(8));
    }
}

/// Omarchy's idle timer honors Wayland idle inhibitors (a playing video in a
/// browser holds one), so it never starts the screensaver. Only step in then;
/// otherwise Omarchy launches it and a second launch would race the first.
pub fn saver_past_inhibitor() {
    let clients = hypr::request("j/clients").and_then(|r| serde_json::from_str::<Value>(&r).ok());
    let inhibited = clients.as_ref().and_then(Value::as_array).is_some_and(|c| c.iter().any(|c| truthy(&c["inhibitingIdle"])));
    if !inhibited || stay_awake() || exec::run(&["omarchy-shell", "lock", "isLocked"], Duration::from_secs(4)).1.trim() == "true" {
        return;
    }
    exec::run(&["omarchy-launch-screensaver"], Duration::from_secs(20));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_config_without_a_lid_action_does_nothing() {
        assert_eq!(lid_action(&json!({})), "nothing");
        assert_eq!(lid_action(&json!({"ac": {"lid": 5}, "battery": {"lid": 5}})), "nothing");
    }

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
