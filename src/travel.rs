use crate::config::truthy;
use crate::{exec, paths};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Duration;
use std::{fs, io};

/// Saving tweaks Travel turns on for its duration when they are off.
const TRAVEL_TWEAKS: [&str; 2] = ["abm", "effects"];
const MALFORMED_TRAVEL: &str = "Travel restore point is malformed; restore ownership manually";
const MALFORMED_DIM: &str = "Quick dim restore point is malformed; restore ownership manually";

/// Python `type(v) is int`: a JSON integer, never a bool or float.
fn is_int(v: &Value) -> bool {
    v.is_i64() || v.is_u64()
}

fn int_in(v: &Value, lo: i64, hi: i64) -> bool {
    v.as_i64().is_some_and(|n| (lo..=hi).contains(&n))
}

/// Python `float(v)`.
fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Python `int(v)`.
fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `re.match(r"^[...]+$", s)`: `$` also matches before one trailing newline.
pub fn matches_set(s: &str, ok: impl Fn(char) -> bool) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    !s.is_empty() && s.chars().all(ok)
}

pub fn safe_name(s: &str) -> bool {
    matches_set(s, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `display_power.safe_name` on any JSON value: Python tests `str(value)` when it is truthy.
pub fn safe_name_v(v: &Value) -> bool {
    match v {
        Value::String(s) => safe_name(s),
        Value::Number(n) => n.as_f64() != Some(0.0) && safe_name(&n.to_string()),
        Value::Bool(b) => *b,
        _ => false,
    }
}

pub fn safe_output(v: &Value) -> bool {
    match v.as_str().and_then(|s| s.strip_prefix("desc:")) {
        Some(desc) => matches_set(desc, |c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '(' | ')' | '+' | '-')),
        None => safe_name_v(v),
    }
}

/// The first internal panel (`eDP-`, `LVDS-` or `DSI-`) with a safe name.
pub fn laptop_monitor(monitors: &[Value]) -> Option<String> {
    monitors
        .iter()
        .filter_map(|m| m["name"].as_str())
        .find(|n| ["eDP-", "LVDS-", "DSI-"].iter().any(|p| n.starts_with(p)) && safe_name(n))
        .map(String::from)
}

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
        *self.0.entry(name.to_string()).or_insert_with(|| {
            let (code, out) = exec::run(&["omarchy", "brightness", "display", "--monitor", name], Duration::from_secs(4));
            let text = out.trim();
            if code != 0 || text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            text.parse().ok().filter(|v| (0..=100).contains(v))
        })
    }
}

fn invalid(error: &str) -> Value {
    json!({"on": true, "invalid": true, "error": error})
}

/// Saved energy state: the default when absent, an invalid marker when unreadable or not an object.
fn read_energy(path: &Path, default: Value) -> Value {
    match fs::read_to_string(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => default,
        Err(_) => invalid("Saved settings are unreadable; restore ownership manually"),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => invalid("Saved settings are malformed"),
            Err(_) => invalid("Saved settings are unreadable; restore ownership manually"),
        },
    }
}

fn valid_monitor_snapshot(saved: &Value) -> bool {
    if !saved.is_object() || !safe_output(&saved["output"]) || !safe_name_v(&saved["name"]) {
        return false;
    }
    if !["width", "height", "x", "y"].iter().all(|k| is_int(&saved[k])) || saved["width"].as_i64() <= Some(0) || saved["height"].as_i64() <= Some(0) {
        return false;
    }
    let zero = json!(0);
    if !int_in(saved.get("transform").unwrap_or(&zero), 0, 7) {
        return false;
    }
    [("refreshRate", 1000.0), ("scale", 10.0)]
        .iter()
        .all(|(k, high)| saved[k].is_number() && saved[k].as_f64().is_some_and(|v| 0.0 < v && v <= *high))
}

fn valid_travel_snapshot(s: &Value) -> bool {
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

pub fn travel_state() -> Value {
    let state = read_energy(&paths::plugin_run_dir().join("travel.json"), json!({"on": false}));
    if truthy(&state["invalid"]) {
        return state;
    }
    let Value::Bool(on) = state["on"] else { return invalid(MALFORMED_TRAVEL) };
    if on {
        let legacy = state["monitors"].is_array() && (state["brightness"].is_null() || is_int(&state["brightness"]));
        if !legacy && !valid_travel_snapshot(&state) {
            return invalid(MALFORMED_TRAVEL);
        }
    }
    state
}

pub fn quick_dim_state() -> Value {
    let state = read_energy(&paths::plugin_run_dir().join("quick-dim.json"), json!({"on": false}));
    if truthy(&state["invalid"]) {
        return state;
    }
    let Value::Bool(on) = state["on"] else { return invalid(MALFORMED_DIM) };
    if on && (!safe_name_v(&state["monitor"]) || !int_in(&state["brightness"], 0, 100)) {
        return invalid(MALFORMED_DIM);
    }
    state
}

/// `(\d+)x(\d+)@([\d.]+)(?:Hz)?`, fullmatch on the stripped mode string.
fn parse_mode(mode: &str) -> Option<(i64, i64, f64)> {
    let (w, rest) = mode.trim().split_once('x')?;
    let (h, rest) = rest.split_once('@')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
    let (rate, tail) = rest.split_at(end);
    if !digits(w) || !digits(h) || rate.is_empty() || !matches!(tail, "" | "Hz") {
        return None;
    }
    Some((w.parse().ok()?, h.parse().ok()?, rate.parse().ok()?))
}

/// The best mode at or below 60 Hz at the panel's size, when it now runs faster.
fn target_refresh(mon: &Value) -> Option<f64> {
    let current = py_float(&mon["refreshRate"])?;
    let (mut width, mut height) = (py_int(&mon["width"])?, py_int(&mon["height"])?);
    let zero = json!(0);
    let transform = mon.get("transform").unwrap_or(&zero);
    if current <= 60.0 || !int_in(transform, 0, 7) {
        return None;
    }
    if matches!(transform.as_i64(), Some(1 | 3 | 5 | 7)) {
        (width, height) = (height, width);
    }
    mon["availableModes"]
        .as_array()?
        .iter()
        .filter_map(|m| parse_mode(m.as_str()?))
        .filter(|&(w, h, rate)| w == width && h == height && 0.0 < rate && rate <= 60.0)
        .map(|(_, _, rate)| rate)
        .reduce(f64::max)
}

fn cpu_available() -> (bool, bool, bool) {
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
    let profile = crate::config::power_profiles().is_some_and(|l| l.iter().any(|p| p == "power-saver"));
    (profile, turbo, epp)
}

/// A control the restore point froze keeps its saved report; otherwise it reflects this machine now.
fn control_report(saved: &Value, fallback: bool, detail: &str) -> Value {
    match saved["available"].as_bool() {
        Some(available) if saved.is_object() => json!({"available": available, "detail": saved["detail"].as_str().unwrap_or(detail)}),
        _ => json!({"available": fallback, "detail": detail}),
    }
}

pub fn travel_status(monitors: &[Value], brightness: &mut BrightnessCache) -> Value {
    let state = travel_state();
    let active = truthy(&state["on"]);
    let laptop = laptop_monitor(monitors).map_or(Value::Null, Value::from);
    let internal = monitors.iter().find(|m| m["name"] == laptop).filter(|m| truthy(m));
    let (profile, turbo, epp) = cpu_available();
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
    fn modes_need_size_at_and_a_rate() {
        assert_eq!(parse_mode(" 1920x1200@60.00Hz "), Some((1920, 1200, 60.0)));
        assert_eq!(parse_mode("1280x720@59.94"), Some((1280, 720, 59.94)));
        assert!(["1920x1200", "x1@2", "1x2@", "1x2@Hz", "1x2@3Hzz", "1x2@..", "ax2@3", "1x2@3 Hz"].iter().all(|m| parse_mode(m).is_none()));
    }

    #[test]
    fn target_is_the_fastest_mode_up_to_60_at_the_panel_size() {
        let mon = |r: Value, t: Value| json!({"refreshRate": r, "width": 1920, "height": 1200, "transform": t, "availableModes": ["1920x1200@165.00Hz", "1920x1200@48.00Hz", "1920x1200@59.94Hz", "1200x1920@30.00Hz", "1920x1080@60.00Hz"]});
        assert_eq!(target_refresh(&mon(json!(165.0), json!(0))), Some(59.94));
        assert_eq!(target_refresh(&mon(json!(165.0), json!(1))), Some(30.0));
        assert_eq!([target_refresh(&mon(json!(60.0), json!(0))), target_refresh(&mon(json!(165.0), json!(8))), target_refresh(&mon(json!(null), json!(0)))], [None; 3]);
    }

    #[test]
    fn laptop_monitor_takes_the_first_safe_internal_panel() {
        let m = [json!({"name": "DP-1"}), json!({"name": "eDP bad"}), json!({"name": "eDP-2"}), json!({"name": "eDP-1"}), json!(5)];
        assert_eq!(laptop_monitor(&m).as_deref(), Some("eDP-2"));
        assert_eq!(laptop_monitor(&[json!({"name": "DP-1"})]), None);
    }
}
