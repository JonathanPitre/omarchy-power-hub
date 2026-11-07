use crate::config::truthy;
use crate::names::{laptop_monitor, matches_set, safe_name, safe_name_v, safe_output};
use crate::state::{int_in, is_int, py_float, py_int};
use crate::{exec, hypr};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// How long a mode change may take to show up after Hyprland accepts it.
const MODE_SETTLE: Duration = Duration::from_secs(3);

/// Python `repr(float)`: shortest round-trip digits, `60.0` not `60`, exponent from 1e16 or below 1e-4.
fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.into();
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    if x == 0.0 {
        return format!("{sign}0.0");
    }
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits = mantissa.replace('.', "");
    if (-4..16).contains(&exp) {
        let body = if exp < 0 {
            format!("0.{}{digits}", "0".repeat((-exp - 1) as usize))
        } else {
            let int_len = exp as usize + 1;
            match digits.len() {
                n if n <= int_len => format!("{digits}{}.0", "0".repeat(int_len - n)),
                _ => format!("{}.{}", &digits[..int_len], &digits[int_len..]),
            }
        };
        return format!("{sign}{body}");
    }
    let frac = if digits.len() > 1 { format!(".{}", &digits[1..]) } else { String::new() };
    format!("{sign}{}{frac}e{}{:02}", &digits[..1], if exp < 0 { '-' } else { '+' }, exp.abs())
}

/// Python `str(value)` for a JSON value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => n.as_f64().filter(|_| n.is_f64()).map_or_else(|| n.to_string(), py_float_repr),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}

/// `spec.get(key) or default`, as Python evaluates it.
pub fn or_default(v: &Value, default: Value) -> Value {
    if truthy(v) { v.clone() } else { default }
}

pub fn restore_lua(spec: &Value) -> String {
    let name = py_str(&or_default(&spec["output"], spec["name"].clone()));
    let (w, h) = (or_default(&spec["width"], 0.into()), or_default(&spec["height"], 0.into()));
    if truthy(&w) && truthy(&h) {
        let field = |k: &str, d: i64| py_str(&or_default(&spec[k], d.into()));
        return format!(
            r#"hl.monitor({{ output = "{name}", mode = "{}x{}@{}", position = "{}x{}", scale = {}, disabled = false }})"#,
            py_str(&w),
            py_str(&h),
            field("refreshRate", 60),
            field("x", 0),
            field("y", 0),
            field("scale", 1)
        );
    }
    format!(r#"hl.monitor({{ output = "{name}", disabled = false }})"#)
}

pub fn valid_monitor_snapshot(saved: &Value) -> bool {
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
pub fn target_refresh(mon: &Value) -> Option<f64> {
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

/// `brightness_get`: None for an unsafe or disconnected monitor, a failed read or an out-of-range value.
/// `monitor` defaults to the laptop panel.
pub fn brightness_get(monitor: Option<&str>, monitors: &[Value]) -> Option<i64> {
    let internal = monitor.filter(|m| !m.is_empty()).map(String::from).or_else(|| laptop_monitor(monitors))?;
    if !safe_name(&internal) || !monitors.iter().any(|m| m["name"] == internal.as_str()) {
        return None;
    }
    let (code, out) = exec::run(&["omarchy", "brightness", "display", "--monitor", &internal], Duration::from_secs(4));
    let text = out.trim();
    if code != 0 || text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|v| (0..=100).contains(v))
}

/// `brightness_set`: the error to show, or an empty string.
pub fn brightness_set(monitor: &Value, value: &Value) -> String {
    let (Some(name), true) = (monitor.as_str().filter(|_| safe_name_v(monitor)), int_in(value, 0, 100)) else {
        return "Invalid display brightness target".into();
    };
    if !hypr::monitors_within(Duration::from_secs(4)).iter().any(|m| m["name"] == *monitor) {
        return "Display is no longer connected".into();
    }
    let (code, out) = exec::run(&["omarchy", "brightness", "display", "--no-osd", "--monitor", name, &format!("{value}%")], Duration::from_secs(4));
    if code == 0 { String::new() } else { Some(out.trim()).filter(|o| !o.is_empty()).unwrap_or("Brightness command failed").into() }
}

/// `display_power.selector`: a description Hyprland rules can match, else the name.
pub fn selector(mon: &Value) -> Value {
    let desc = py_str(&or_default(&mon["description"], json!("")));
    if matches_set(&desc, |c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '(' | ')' | '+' | '-')) {
        return json!(format!("desc:{desc}"));
    }
    mon["name"].clone()
}

/// What Travel saves to put a monitor back: `display_power.snapshot` with its transform, sides swapped when rotated.
pub fn monitor_snapshot(mon: &Value) -> Result<Value, String> {
    let transform = mon.get("transform").unwrap_or(&json!(0)).clone();
    let field = |k: &str, d: i64| or_default(&mon[k], d.into());
    let (mut width, mut height) = (field("width", 0), field("height", 0));
    if !int_in(&transform, 0, 7) {
        return Err("Invalid laptop display transform".into());
    }
    if matches!(transform.as_i64(), Some(1 | 3 | 5 | 7)) {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(json!({"name": mon["name"], "output": selector(mon), "width": width, "height": height, "refreshRate": field("refreshRate", 60),
        "x": field("x", 0), "y": field("y", 0), "scale": field("scale", 1), "transform": transform}))
}

/// Whether the monitor now runs at the saved size and `refresh` (within 0.1 Hz); anything unreadable is a no.
fn mode_holds(saved: &Value, actual: &Value, refresh: &Value) -> bool {
    let (mut width, mut height) = (saved["width"].as_i64(), saved["height"].as_i64());
    if matches!(saved.get("transform").and_then(Value::as_i64), Some(1 | 3 | 5 | 7)) {
        std::mem::swap(&mut width, &mut height);
    }
    let (Some(w), Some(h), Some(rate), Some(want)) = (py_int(&actual["width"]), py_int(&actual["height"]), py_float(&actual["refreshRate"]), py_float(refresh)) else {
        return false;
    };
    Some(w) == width && Some(h) == height && (rate - want).abs() <= 0.1
}

/// Put the saved monitor back at `refresh` and check it took: the error to show, or an empty string.
pub fn monitor_change(saved: &Value, refresh: &Value) -> String {
    if !valid_monitor_snapshot(saved) {
        return "Saved monitor restore point is invalid".into();
    }
    let mut spec = saved.clone();
    spec["refreshRate"] = refresh.clone();
    if let Err(e) = hypr::eval_checked(&restore_lua(&spec)) {
        return Some(e.trim()).filter(|e| !e.is_empty()).unwrap_or("Monitor mode change failed").into();
    }
    // Hyprland replies first and changes the mode about 0.7 s later, refusing reads while it connects.
    let deadline = Instant::now() + MODE_SETTLE;
    loop {
        let failure = match hypr::monitors_within(Duration::from_secs(4)).iter().find(|m| m["name"] == saved["name"]) {
            None => "Saved laptop display is unavailable",
            Some(actual) if mode_holds(saved, actual, refresh) => return String::new(),
            Some(_) => "Laptop display mode could not be verified",
        };
        if Instant::now() >= deadline {
            return failure.into();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_like_python_repr() {
        let got: Vec<_> = [60.0, 59.951, 1.25, 144.0, 0.1, 1e15, 1e16, 0.0001, 0.00001, 1.5e300, -2.5, 0.0, 165.00001, 123456.789]
            .map(py_float_repr)
            .into();
        assert_eq!(got, ["60.0", "59.951", "1.25", "144.0", "0.1", "1000000000000000.0", "1e+16", "0.0001", "1e-05", "1.5e+300", "-2.5", "0.0", "165.00001", "123456.789"]);
    }

    #[test]
    fn restore_lua_keeps_python_number_forms() {
        let spec = json!({"name": "DP-1", "width": 2560, "height": 1600, "refreshRate": 59.951, "x": 0, "y": 0, "scale": 1.25});
        assert_eq!(restore_lua(&spec), r#"hl.monitor({ output = "DP-1", mode = "2560x1600@59.951", position = "0x0", scale = 1.25, disabled = false })"#);
        let spec = json!({"name": "DP-1", "width": 1920, "height": 1080, "refreshRate": 60.0, "scale": 1});
        assert!(restore_lua(&spec).contains(r#"mode = "1920x1080@60.0", position = "0x0", scale = 1,"#));
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

    fn panel() -> Value {
        json!({"name": "eDP-1", "description": "BOE NE160 (0x0A)", "width": 1920, "height": 1200, "x": 0, "y": 0, "refreshRate": 165.004, "scale": 1.25, "transform": 0})
    }

    #[test]
    fn snapshot_keeps_key_order_and_number_forms() {
        let got = monitor_snapshot(&panel()).unwrap();
        assert_eq!(got.to_string(), r#"{"name":"eDP-1","output":"desc:BOE NE160 (0x0A)","width":1920,"height":1200,"refreshRate":165.004,"x":0,"y":0,"scale":1.25,"transform":0}"#);
    }

    #[test]
    fn snapshot_falls_back_like_python_or() {
        let got = monitor_snapshot(&json!({"name": "eDP-1", "description": "bad;desc", "width": 0, "refreshRate": null, "scale": 0.0})).unwrap();
        assert_eq!(got.to_string(), r#"{"name":"eDP-1","output":"eDP-1","width":0,"height":0,"refreshRate":60,"x":0,"y":0,"scale":1,"transform":0}"#);
        assert_eq!(monitor_snapshot(&json!({"description": ""})).unwrap()["output"], Value::Null);
    }

    #[test]
    fn snapshot_swaps_the_sides_of_a_rotated_panel() {
        for (t, sides) in [(1, (1200, 1920)), (2, (1920, 1200)), (3, (1200, 1920)), (4, (1920, 1200)), (7, (1200, 1920))] {
            let got = monitor_snapshot(&json!({"name": "eDP-1", "width": 1920, "height": 1200, "transform": t})).unwrap();
            assert_eq!((got["width"].as_i64(), got["height"].as_i64(), &got["transform"]), (Some(sides.0), Some(sides.1), &json!(t)), "transform {t}");
        }
    }

    #[test]
    fn snapshot_rejects_a_transform_that_is_not_0_to_7() {
        for t in [json!(8), json!(-1), json!(null), json!("1"), json!(true), json!(1.0)] {
            assert_eq!(monitor_snapshot(&json!({"name": "eDP-1", "transform": t})), Err("Invalid laptop display transform".into()), "{t}");
        }
    }

    fn saved(transform: i64) -> Value {
        json!({"output": "eDP-1", "name": "eDP-1", "width": 1920, "height": 1200, "x": 0, "y": 0, "refreshRate": 165.004, "scale": 1.25, "transform": transform})
    }

    #[test]
    fn change_refuses_an_invalid_restore_point_before_touching_hyprland() {
        for bad in [json!(null), json!({}), json!({"name": "eDP-1"}), {let mut s = saved(0); s["width"] = json!(0); s}, saved(9)] {
            assert_eq!(monitor_change(&bad, &json!(60.0)), "Saved monitor restore point is invalid");
        }
    }

    #[test]
    fn mode_holds_within_a_tenth_of_a_hertz() {
        let actual = |w: Value, r: Value| json!({"width": w, "height": 1200, "refreshRate": r});
        assert!(mode_holds(&saved(0), &actual(json!(1920), json!(59.94)), &json!(60.0)));
        assert!(mode_holds(&saved(0), &actual(json!("1920"), json!(60.05)), &json!(60.0)));
        assert!(!mode_holds(&saved(0), &actual(json!(1920), json!(60.2)), &json!(60.0)));
        assert!(!mode_holds(&saved(0), &actual(json!(1280), json!(60.0)), &json!(60.0)));
        assert!(!mode_holds(&saved(0), &json!({"width": 1920, "height": 1200}), &json!(60.0)));
    }

    #[test]
    fn mode_holds_compares_a_rotated_panel_by_its_saved_swap() {
        let rotated = json!({"width": 1200, "height": 1920, "refreshRate": 60.0});
        assert!(mode_holds(&saved(1), &rotated, &json!(60.0)));
        assert!(!mode_holds(&saved(0), &rotated, &json!(60.0)));
    }

    #[test]
    fn brightness_targets_are_checked_before_any_command() {
        for (monitor, value) in [(json!("eDP-1"), json!(101)), (json!("eDP-1"), json!(-1)), (json!("eDP-1"), json!(5.0)), (json!("eDP-1"), json!(true)), (json!("e DP"), json!(5)), (json!(""), json!(5))] {
            assert_eq!(brightness_set(&monitor, &value), "Invalid display brightness target", "{monitor} {value}");
        }
    }
}
