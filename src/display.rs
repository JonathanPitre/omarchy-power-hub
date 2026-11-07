use crate::config::truthy;
use crate::monitor::{or_default, py_str, restore_lua, selector};
use crate::names::{laptop_monitor, safe_name, safe_name_v, safe_output};
use crate::state::py_int;
use crate::{config, exec, hypr, idle, paths, power, settings};
use serde_json::{Map, Value, json};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

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
pub fn blanked(monitors: &[Value]) -> bool {
    monitors.iter().any(|m| !truthy(&m["disabled"]) && !truthy(&m["dpmsStatus"]))
}

/// `display_power.load_state`, hardened: always an object whose `disabled` is an array.
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
            hypr::eval_slow(&restore_lua(&spec));
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
    // Unlike Python, a `disabled` that is not a list counts as empty: `idle`, not a crash or a restore of nothing.
    let state = load_state(state_path);
    if !truthy(&state["disabled"]) {
        return "idle".into();
    }
    if !awake(&hypr::monitors_within(hypr::WRITE_WAIT)) {
        return "waiting".into();
    }
    restore(&state);
    let _ = fs::write(state_path, "{\"disabled\": []}\n");
    "restored".into()
}

fn state_path() -> PathBuf {
    paths::plugin_run_dir().join("display-off.json")
}

/// Where heads that drop off the bus on DPMS off are remembered.
fn droppers_path() -> PathBuf {
    paths::state_dir().join("omarchy-power-hub/bus-droppers.json")
}

/// `display_power.load_droppers`: the description strings in the file, nothing when it is not a list.
pub fn droppers(path: &Path) -> Vec<String> {
    let data = fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok());
    data.as_ref()
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| d.as_str().map(String::from))
        .collect()
}

fn remember_dropper(desc: &Value, path: &Path) {
    let known = droppers(path);
    if !truthy(desc) || desc.as_str().is_some_and(|d| known.iter().any(|k| k == d)) {
        return;
    }
    let mut all: Vec<Value> = known.into_iter().map(Value::String).collect();
    all.push(desc.clone());
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(path, Value::Array(all).to_string() + "\n");
}

fn save_state(disabled: Vec<Value>) {
    let _ = fs::write(
        state_path(),
        json!({"disabled": disabled}).to_string() + "\n",
    );
}

/// The part of a parked entry that names the head: its `name`, or the entry itself when it is a bare name.
fn name_of(spec: &Value) -> &Value {
    if spec.is_object() {
        &spec["name"]
    } else {
        spec
    }
}

/// `display_power.snapshot`: what is saved to put a head back, without Travel's transform.
fn snapshot(m: &Value) -> Value {
    let field = |k: &str, d: i64| or_default(&m[k], d.into());
    json!({"name": m["name"], "output": selector(m), "width": field("width", 0), "height": field("height", 0),
        "refreshRate": field("refreshRate", 60), "x": field("x", 0), "y": field("y", 0), "scale": field("scale", 1)})
}

fn disable_lua(m: &Value) -> String {
    format!(
        r#"hl.monitor({{ output = "{}", disabled = true }})"#,
        py_str(&selector(m))
    )
}

fn dpms(action: &str, name: &Value) {
    hypr::dispatch(&format!(
        r#"hl.dsp.dpms({{ action = "{action}", monitor = "{}" }})"#,
        py_str(name)
    ));
}

/// Parked entries without `m`, then `m` parked: a head is listed once.
fn parked_with(mut disabled: Vec<Value>, m: &Value) -> Vec<Value> {
    disabled.retain(|s| name_of(s) != &m["name"]);
    disabled.push(snapshot(m));
    disabled
}

fn usable(m: &Value) -> bool {
    !truthy(&m["disabled"]) && safe_name_v(&m["name"])
}

/// Heads known to drop off the bus on DPMS off, all but one of them when they are every head.
fn plan_direct(monitors: &[Value], droppers: &[String]) -> Vec<Value> {
    let enabled: Vec<&Value> = monitors.iter().filter(|m| usable(m)).collect();
    let mut chosen: Vec<&Value> = enabled
        .iter()
        .copied()
        .filter(|m| {
            m["description"]
                .as_str()
                .is_some_and(|d| droppers.iter().any(|k| k == d))
        })
        .collect();
    if chosen.len() == enabled.len() {
        chosen.drain(..chosen.len().min(1));
    }
    chosen.into_iter().cloned().collect()
}

/// Lit heads to disable after DPMS off, keeping the laptop panel (else the first head) and never the last output.
fn fallback_disable(monitors: &[Value]) -> Vec<Value> {
    let enabled: Vec<&Value> = monitors.iter().filter(|m| usable(m)).collect();
    if enabled.len() <= 1 {
        return vec![];
    }
    let owned: Vec<Value> = enabled.iter().map(|m| (*m).clone()).collect();
    let keep = laptop_monitor(&owned).map_or_else(|| enabled[0]["name"].clone(), Value::String);
    // Python also counts down to never disable the last output, but `keep` is never chosen, so that cannot trigger.
    enabled
        .into_iter()
        .filter(|m| truthy(&m["dpmsStatus"]) && m["name"] != keep)
        .cloned()
        .collect()
}

pub fn dismiss_screensaver() {
    exec::run(&["pkill", "-x", "ttfx"], hypr::WRITE_WAIT);
    exec::run(
        &["pkill", "-f", "[o]rg.omarchy.screensaver"],
        hypr::WRITE_WAIT,
    );
    hypr::eval_slow("hl.config({ cursor = { invisible = false } })");
}

/// `on` restores parked heads and lights every monitor. `off` parks the heads DPMS cannot reach, blanks the rest and
/// parks any that stay lit; `disable` false is the lid close, which only sends DPMS.
/// Lid close skips the disabling: disabling the NVIDIA FlipGo heads leaves that GPU with no outputs and Hyprland 0.56 SIGSEGVs.
pub fn set_displays(off: bool, disable: bool) {
    dismiss_screensaver();
    let path = state_path();
    if !off {
        restore(&load_state(&path));
        save_state(vec![]);
        for m in hypr::monitors_within(hypr::WRITE_WAIT) {
            if safe_name_v(&m["name"]) {
                dpms("enable", &m["name"]);
            }
        }
        return;
    }
    // Heads that drop off the bus on DPMS off are disabled instead, so they lose
    // signal once rather than reconnecting lit and being parked afterwards.
    let parked = |p: &Path| {
        load_state(p)["disabled"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    };
    if disable {
        let mut disabled = parked(&path);
        for m in plan_direct(
            &hypr::monitors_within(hypr::WRITE_WAIT),
            &droppers(&droppers_path()),
        ) {
            hypr::eval_slow(&disable_lua(&m));
            disabled = parked_with(disabled, &m);
        }
        save_state(disabled);
    }
    for m in hypr::monitors_within(hypr::WRITE_WAIT)
        .iter()
        .filter(|m| usable(m))
    {
        dpms("disable", &m["name"]);
    }
    if !disable {
        return;
    }
    let mut disabled = parked(&path);
    for m in fallback_disable(&hypr::monitors_within(hypr::WRITE_WAIT)) {
        hypr::eval_slow(&disable_lua(&m));
        disabled = parked_with(disabled, &m);
    }
    save_state(disabled);
}

/// A head that drops off the bus on DPMS off comes back as a new, lit output.
/// DPMS again would repeat the drop, so it is disabled until the others wake.
pub fn park(name: &str) -> bool {
    if !safe_name(name) {
        return false;
    }
    let mons = hypr::monitors_within(hypr::WRITE_WAIT);
    let Some(at) = mons
        .iter()
        .position(|m| m["name"].as_str() == Some(name) && !truthy(&m["disabled"]))
    else {
        return false;
    };
    let others: Vec<Value> = mons
        .iter()
        .enumerate()
        .filter(|(i, m)| *i != at && !truthy(&m["disabled"]))
        .map(|(_, m)| m.clone())
        .collect();
    if others.is_empty() || !blanked(&others) {
        return false;
    }
    let target = &mons[at];
    hypr::eval_slow(&disable_lua(target));
    remember_dropper(&target["description"], &droppers_path());
    let parked = load_state(&state_path())["disabled"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    save_state(parked_with(parked, target));
    true
}

/// Put the idle timers where blanking (or waking) needs them. A config that cannot be read must not stop the blank.
fn idle_for_blank(blank: bool) {
    let cfg = config::load();
    let src = &cfg[power::active_side(&cfg)];
    let _ = settings::shell_idle(
        if blank {
            0
        } else {
            settings::effective_saver(src)
        },
        py_int(&src["lock"]).unwrap_or(0),
    );
}

/// Keyboard and power-LED lights follow the displays; nothing to do until the light path moves here.
pub fn sync_lights(_off: bool) {}

/// `displays on|off|disable`: the idle timers and the heads under the state lock, then the lights. None when the lock is unavailable.
pub fn displays(mode: &str) -> Option<String> {
    let off = matches!(mode, "off" | "disable");
    with_state_lock(|| {
        idle_for_blank(off);
        set_displays(off, true);
    })?;
    sync_lights(off);
    Some("ok".into())
}

/// `displays park NAME`: `skip` when the lid is closed, under the state lock.
pub fn park_verb(name: &str) -> Option<String> {
    with_state_lock(|| {
        if idle::lid_is_open(&idle::lid_text()) && park(name) {
            "parked"
        } else {
            "skip"
        }
        .into()
    })
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
