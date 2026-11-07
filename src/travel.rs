use crate::config::truthy;
use crate::monitor::{brightness_get, brightness_set, target_refresh, valid_monitor_snapshot};
use crate::names::{laptop_monitor, safe_name, safe_name_v};
use crate::state::{atomic_state, energy_lock, int_in, py_float, quick_dim_state, travel_state};
use crate::{config, hypr, paths};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::{fs, io};

/// Saving tweaks Travel turns on for its duration when they are off.
const TRAVEL_TWEAKS: [&str; 2] = ["abm", "effects"];

/// Python's `%g` formatting (`format(x, "g")`): six significant digits, trailing zeros dropped.
pub fn fmt_g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.into();
    }
    let trim = |s: &str| if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() };
    let sci = format!("{x:.5e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..6).contains(&exp) {
        trim(&format!("{:.*}", (5 - exp) as usize, x))
    } else {
        format!("{}e{}{:02}", trim(mantissa), if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// Brightness read once per monitor name, however many reports ask for it.
#[derive(Default)]
pub struct BrightnessCache(HashMap<String, Option<i64>>);

impl BrightnessCache {
    /// `brightness_get`: None for an unsafe or disconnected monitor, a failed read or an out-of-range value.
    pub fn get(&mut self, monitors: &[Value], monitor: &Value) -> Option<i64> {
        let name = monitor.as_str().filter(|n| safe_name(n))?;
        if !monitors.iter().any(|m| m["name"] == *monitor) {
            return None;
        }
        *self.0.entry(name.to_string()).or_insert_with(|| brightness_get(Some(name), monitors))
    }
}

pub fn valid_travel_snapshot(s: &Value) -> bool {
    let (Some(complete), Some(reduce_cpu)) = (s["complete"].as_bool(), s["reduceCpu"].as_bool()) else { return false };
    let (Some(pending), true, true, true) = (s["pending"].as_array(), s["cpu"].is_object(), s["controls"].is_object(), s["error"].is_string()) else {
        return false;
    };
    if !matches!(s["source"].as_str(), Some("ac" | "battery")) {
        return false;
    }
    let (profile, brightness, monitor, cpu) = (&s["profile"], &s["brightness"], &s["monitor"], &s["cpu"]);
    if !(profile.is_null() || profile.is_string()) {
        return false;
    }
    if !brightness.is_null() && !(brightness.is_object() && safe_name_v(&brightness["monitor"]) && int_in(&brightness["before"], 0, 100)) {
        return false;
    }
    if !monitor.is_null() && !valid_monitor_snapshot(monitor) {
        return false;
    }
    let turbo = &cpu["turbo"];
    if !(turbo.is_null() || turbo.is_boolean()) {
        return false;
    }
    let Some(preferences) = cpu["epp"].as_array() else { return false };
    let mut indexes = BTreeSet::new();
    for entry in preferences {
        let cpu_index = entry["cpu"].as_i64().filter(|n| *n >= 0);
        let before_ok = matches!(entry["before"].as_str(), Some("power" | "balance_power" | "balance_performance" | "performance"));
        let target_ok = entry["target"].is_null() || matches!(entry["target"].as_str(), Some("power" | "balance_power"));
        match cpu_index {
            Some(i) if entry.is_object() && before_ok && target_ok && indexes.insert(i) => {}
            _ => return false,
        }
    }
    let Some(tokens) = pending.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else { return false };
    let pending_set: BTreeSet<&str> = tokens.iter().copied().collect();
    if pending_set.len() != tokens.len() {
        return false;
    }
    let none = json!([]);
    let Some(tweaks) = s.get("tweaks").unwrap_or(&none).as_array().and_then(|t| t.iter().map(Value::as_str).collect::<Option<BTreeSet<_>>>()) else {
        return false;
    };
    if !tweaks.iter().all(|t| TRAVEL_TWEAKS.contains(t)) {
        return false;
    }
    let mut valid: BTreeSet<String> = ["profile", "turbo", "brightness", "refresh"].map(String::from).into();
    valid.extend(indexes.iter().map(|i| format!("epp:{i}")));
    valid.extend(tweaks.iter().map(|t| format!("tweak:{t}")));
    if !pending_set.iter().all(|t| valid.contains(*t)) {
        return false;
    }
    let has = |t: &str| pending_set.contains(t);
    let profile_empty = profile.as_str().is_none_or(str::is_empty);
    if has("profile") && profile_empty
        || has("turbo") && turbo.is_null()
        || has("brightness") && brightness.is_null()
        || has("refresh") && monitor.is_null()
        || !reduce_cpu && (!profile.is_null() || !turbo.is_null() || !preferences.is_empty() || pending_set.iter().any(|t| t.starts_with("epp:")))
    {
        return false;
    }
    for name in ["profile", "turbo", "epp", "brightness", "refresh"] {
        let item = &s["controls"][name];
        if !(item.is_object() && item["available"].is_boolean() && item["detail"].is_string()) {
            return false;
        }
    }
    if complete {
        let mut expected: BTreeSet<String> = BTreeSet::new();
        for (present, token) in [(!profile.is_null(), "profile"), (!turbo.is_null(), "turbo"), (!brightness.is_null(), "brightness")] {
            if present {
                expected.insert(token.into());
            }
        }
        expected.extend(indexes.iter().map(|i| format!("epp:{i}")));
        expected.extend(tweaks.iter().map(|t| format!("tweak:{t}")));
        if !monitor.is_null() && s["controls"]["refresh"]["detail"].as_str().is_some_and(|d| d.starts_with("Laptop refresh capped at ")) {
            expected.insert("refresh".into());
        }
        if expected != pending_set.iter().map(|t| t.to_string()).collect() {
            return false;
        }
    }
    true
}

fn cpu_available(profiles: &mut config::Profiles) -> (bool, bool, bool) {
    let cpu = paths::root("/sys/devices/system/cpu");
    let turbo = ["intel_pstate/no_turbo", "cpufreq/boost"].iter().map(|p| cpu.join(p)).find(|p| p.exists()).is_some_and(|p| {
        let raw = fs::read_to_string(&p).unwrap_or_default();
        matches!(raw.trim(), "0" | "1")
    });
    let epp = fs::read_dir(&cpu).is_ok_and(|d| {
        d.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let freq = e.path().join("cpufreq");
            name.strip_prefix("cpu").is_some_and(|n| n.starts_with(|c: char| c.is_ascii_digit()))
                && freq.join("energy_performance_preference").exists()
                && fs::read_to_string(freq.join("energy_performance_available_preferences")).is_ok_and(|t| t.split_whitespace().any(|p| matches!(p, "power" | "balance_power")))
        })
    });
    let profile = config::offered(profiles).iter().any(|p| p == "power-saver");
    (profile, turbo, epp)
}

/// A control the restore point froze keeps its saved report; otherwise it reflects this machine now.
fn control_report(saved: &Value, fallback: bool, detail: &str) -> Value {
    match saved["available"].as_bool() {
        Some(available) if saved.is_object() => json!({"available": available, "detail": saved["detail"].as_str().unwrap_or(detail)}),
        _ => json!({"available": fallback, "detail": detail}),
    }
}

pub fn travel_status(monitors: &[Value], brightness: &mut BrightnessCache, profiles: &mut config::Profiles) -> Value {
    let state = travel_state();
    let active = truthy(&state["on"]);
    let laptop = laptop_monitor(monitors).map_or(Value::Null, Value::from);
    let internal = monitors.iter().find(|m| m["name"] == laptop).filter(|m| truthy(m));
    let (profile, turbo, epp) = cpu_available(profiles);
    let current_refresh = internal.and_then(|m| py_float(&m["refreshRate"]));
    let target = internal.and_then(target_refresh);
    let refresh_available = internal.is_some() && (target.is_some() || current_refresh.is_some_and(|c| c <= 60.0));
    let refresh_detail = match (target, current_refresh) {
        (Some(t), _) => format!("Laptop refresh capped at {} Hz", fmt_g(t)),
        (None, Some(c)) if refresh_available => format!("Laptop refresh is {} Hz (already at or below 60 Hz)", fmt_g(c)),
        _ => "No supported mode at or below 60 Hz".into(),
    };
    let frozen = |name: &str| if active { state["controls"][name].clone() } else { Value::Null };
    let readable = internal.is_some_and(|m| brightness.get(monitors, &m["name"]).is_some());
    let reduce_cpu = active && state.get("reduceCpu").map_or(state.get("complete").is_none(), truthy);
    let phase = match (active, truthy(&state["complete"])) {
        (false, _) => "off",
        (true, true) => "on",
        (true, false) => "incomplete",
    };
    let error = match state.get("error") {
        None => "",
        Some(Value::String(s)) => s.as_str(),
        Some(_) => "Travel restore state is malformed",
    };
    json!({
        "phase": phase,
        "reduceCpu": reduce_cpu,
        "error": error,
        "controls": {
            "profile": control_report(&frozen("profile"), profile, "Power saver"),
            "turbo": control_report(&frozen("turbo"), turbo, "CPU turbo off"),
            "epp": control_report(&frozen("epp"), epp, "Energy-saving CPU preference"),
            "brightness": control_report(&frozen("brightness"), readable, if readable { "Laptop brightness capped at 40%" } else { "Brightness cannot be read" }),
            "refresh": control_report(&frozen("refresh"), refresh_available, &refresh_detail),
        },
    })
}

/// `quick-dim on|off` under the energy lock: the error to show, or an empty string.
/// An Err is a state file that could not be written, which the caller words.
pub fn quick_dim(on: bool) -> io::Result<String> {
    let _lock = energy_lock()?;
    if truthy(&travel_state()["on"]) {
        return Ok("Turn off Travel mode before changing Quick dim".into());
    }
    let path = paths::plugin_run_dir().join("quick-dim.json");
    let mut state = quick_dim_state();
    if truthy(&state["invalid"]) {
        return Ok(state["error"].as_str().unwrap_or("Quick dim restore point is malformed").into());
    }
    if state["on"].as_bool() == Some(on) {
        return Ok(String::new());
    }
    let (monitor, target, mut saved) = if on {
        let screens = hypr::monitors();
        let Some(monitor) = laptop_monitor(&screens) else { return Ok("Laptop display is unavailable".into()) };
        let Some(before) = brightness_get(Some(&monitor), &screens) else { return Ok("Laptop brightness cannot be read".into()) };
        let saved = json!({"on": true, "monitor": monitor, "brightness": before, "error": ""});
        if let Err(e) = atomic_state(&path, &saved) {
            return Ok(format!("Could not save brightness restore point: {e}"));
        }
        (monitor, before.min(40), saved)
    } else {
        let before = state["brightness"].as_i64().unwrap_or_default();
        (state["monitor"].as_str().unwrap_or_default().to_string(), before, std::mem::take(&mut state))
    };
    let err = brightness_set(&json!(monitor), &json!(target));
    let actual = brightness_get(Some(&monitor), &hypr::monitors());
    if !err.is_empty() || actual.is_none_or(|a| (a - target).abs() > 1) {
        let reason = if !err.is_empty() { err } else if on { "Brightness did not reach the requested level".into() } else { "Brightness restoration could not be verified".into() };
        saved["error"] = json!(reason);
        atomic_state(&path, &saved)?;
        return Ok(reason);
    }
    if !on {
        atomic_state(&path, &json!({"on": false}))?;
    }
    Ok(String::new())
}

pub fn quick_dim_status(monitors: &[Value], brightness: &mut BrightnessCache) -> Value {
    let state = quick_dim_state();
    let monitor = if truthy(&state["on"]) { state["monitor"].clone() } else { laptop_monitor(monitors).map_or(Value::Null, Value::from) };
    let value = brightness.get(monitors, &monitor);
    // Python `state.get("on", False) or value is not None` yields the saved `on` itself when truthy.
    let available = if truthy(&state["on"]) { state["on"].clone() } else { json!(value.is_some()) };
    json!({"on": truthy(&state["on"]), "available": available, "brightness": value, "error": state.get("error").cloned().unwrap_or_else(|| json!(""))})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::safe_output;

    #[test]
    fn fmt_g_matches_python() {
        assert_eq!(fmt_g(60.0), "60");
        assert_eq!(fmt_g(59.94), "59.94");
        assert_eq!(fmt_g(165.004), "165.004");
        assert_eq!(fmt_g(59.99997), "60");
    }

    #[test]
    fn fmt_g_switches_to_exponents_outside_six_digits() {
        assert_eq!([fmt_g(0.0), fmt_g(1234567.0), fmt_g(0.0001), fmt_g(0.00001234), fmt_g(-2.5), fmt_g(100000.0)], ["0", "1.23457e+06", "0.0001", "1.234e-05", "-2.5", "100000"]);
    }

    #[test]
    fn names_allow_a_trailing_newline_like_python_dollar() {
        assert!(["eDP-1", "DP-3.a_b", "x\n"].iter().all(|n| safe_name(n)));
        assert!(["", "\n", "a b", "a\n\n", "a/b", "é"].iter().all(|n| !safe_name(n)));
    }

    #[test]
    fn outputs_are_names_or_safe_descriptions() {
        assert!([json!("eDP-1"), json!("desc:BOE NE160 (1)+x.y-z")].iter().all(safe_output));
        assert!([json!(""), json!(null), json!("desc:"), json!("desc:a;b"), json!("a b"), json!(false)].iter().all(|v| !safe_output(v)));
    }

    #[test]
    fn laptop_monitor_takes_the_first_safe_internal_panel() {
        let m = [json!({"name": "DP-1"}), json!({"name": "eDP bad"}), json!({"name": "eDP-2"}), json!({"name": "eDP-1"}), json!(5)];
        assert_eq!(laptop_monitor(&m).as_deref(), Some("eDP-2"));
        assert_eq!(laptop_monitor(&[json!({"name": "DP-1"})]), None);
    }
}
