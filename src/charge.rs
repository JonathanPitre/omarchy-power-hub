use crate::config::{self, charge_limit, truthy};
use crate::{exec, paths, power};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use std::fs;

const HELPER_FILES: [&str; 2] = ["ops-priv", "hid_wakeup.py"];

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// The passwordless sudo rule covers only the root-owned copy; a copy that differs from the plugin's counts as absent.
pub fn helper_bin() -> Option<PathBuf> {
    let (src, dst) = (paths::bin_dir(), paths::helper_dir());
    let same = HELPER_FILES.iter().all(|n| matches!((fs::read(dst.join(n)), fs::read(src.join(n))), (Ok(a), Ok(b)) if a == b));
    same.then(|| dst.join("ops-priv"))
}

pub fn helper_needed() -> bool {
    let Some(helper) = helper_bin() else { return true };
    let (code, out) = exec::run(&["sudo", "-n", "-l"], secs(4));
    code != 0 || !out.contains(&format!("NOPASSWD: {}", helper.display()))
}

/// Runs a privileged op through the sudo-allowed helper, falling back to pkexec. Empty on success.
pub fn privileged(args: &[&str]) -> String {
    if let Some(helper) = helper_bin() {
        let helper = helper.to_string_lossy();
        let cmd: Vec<&str> = ["sudo", "-n", &helper].into_iter().chain(args.iter().copied()).collect();
        if exec::run(&cmd, secs(8)).0 == 0 {
            return String::new();
        }
    }
    let ops = paths::ops_priv();
    let ops = ops.to_string_lossy();
    let cmd: Vec<&str> = ["pkexec", &ops].into_iter().chain(args.iter().copied()).collect();
    match exec::run(&cmd, secs(30)) {
        (0, _) => String::new(),
        (_, out) if out.trim().is_empty() => "privilege required".into(),
        (_, out) => out.trim().into(),
    }
}

/// `re.search(r"Maximum\D*(\d{1,3})", out)`: the first "Maximum" decides, as no digit after it means none anywhere later.
fn parse_max(out: &str) -> Option<i64> {
    let rest = &out[out.find("Maximum")? + "Maximum".len()..];
    let digits = rest.trim_start_matches(|c: char| !c.is_ascii_digit());
    let n = digits.chars().take_while(char::is_ascii_digit).take(3).count();
    digits[..n].parse().ok()
}

fn none() -> Value {
    json!({"kind": "none", "available": false, "reason": "", "limit": null, "tool": false, "health": null, "policy": null})
}

/// Python's `parse_busctl_value`.
fn parse_busctl_value(raw: &str) -> Option<Value> {
    let text = raw.trim().split('\n').next().unwrap_or("");
    if text.is_empty() {
        return None;
    }
    let Some((typ, val)) = text.split_once(char::is_whitespace) else { return Some(json!(text)) };
    let val = val.trim_start();
    match typ {
        "b" => Some(json!(matches!(val.to_lowercase().as_str(), "true" | "1"))),
        "u" | "t" | "i" | "n" | "q" | "x" => val.parse::<i64>().ok().map(|n| json!(n)),
        "s" => Some(json!(val.trim().trim_matches('"'))),
        _ => Some(json!(val)),
    }
}

fn upower_prop(device: &str, prop: &str) -> Option<Value> {
    let args = ["busctl", "get-property", "org.freedesktop.UPower", device, "org.freedesktop.UPower.Device", prop];
    let (code, out) = exec::run(&args, secs(4));
    if code == 0 { parse_busctl_value(&out) } else { None }
}

fn upower_batteries() -> Vec<String> {
    let (code, out) = exec::run(&["upower", "-e"], secs(4));
    if code != 0 {
        return vec![];
    }
    out.lines()
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.ends_with("/DisplayDevice"))
        .filter(|d| upower_prop(d, "ChargeThresholdSupported") == Some(json!(true)))
        .map(String::from)
        .collect()
}

/// Python `isinstance(v, int)`, where bool counts.
fn as_int(v: &Option<Value>) -> Option<i64> {
    match v {
        Some(Value::Bool(b)) => Some(i64::from(*b)),
        Some(Value::Number(n)) => n.as_i64(),
        _ => None,
    }
}

fn upower_health_status() -> Value {
    let mut empty = none();
    empty["reason"] = json!("no charge threshold");
    let batteries = upower_batteries();
    if batteries.is_empty() {
        return empty;
    }
    let mut enabled = true;
    let mut policy: Option<String> = None;
    for device in &batteries {
        if upower_prop(device, "ChargeThresholdEnabled") != Some(json!(true)) {
            enabled = false;
        }
        if policy.is_some() {
            continue;
        }
        let settings = as_int(&upower_prop(device, "ChargeThresholdSettingsSupported")).unwrap_or(0);
        let start = as_int(&upower_prop(device, "ChargeStartThreshold"));
        let end = as_int(&upower_prop(device, "ChargeEndThreshold"));
        policy = match end {
            Some(end) if settings & 2 != 0 => Some(match start {
                Some(start) if settings & 1 != 0 && start < end => format!("{start}-{end}"),
                _ => end.to_string(),
            }),
            _ if settings & 4 != 0 => Some("firmware".into()),
            _ => None,
        };
    }
    json!({"kind": "upower", "available": true, "reason": "", "limit": null, "tool": false, "health": enabled, "policy": policy.unwrap_or_else(|| "firmware".into())})
}

pub fn charge_status() -> Value {
    if !power::framework() {
        return upower_health_status();
    }
    let mut v = none();
    if !exec::on_path("framework_tool") {
        v["reason"] = json!("framework_tool is not installed");
        return v;
    }
    let Some(helper) = helper_bin() else {
        return json!({"kind": "framework", "available": false, "reason": "permission", "limit": null, "tool": true, "health": null, "policy": null});
    };
    let (code, out) = exec::run(&["sudo", "-n", &helper.to_string_lossy(), "charge-limit"], secs(6));
    let limit = if code == 0 { parse_max(&out) } else { None };
    let reason = match limit {
        Some(_) => String::new(),
        None if out.contains("password") => "permission".into(),
        None => out.trim().into(),
    };
    json!({"kind": "framework", "available": limit.is_some(), "reason": reason, "limit": limit, "tool": true, "health": null, "policy": null})
}

pub fn set_charge(limit: i64) -> String {
    if !power::framework() {
        return "not a Framework machine".into();
    }
    if !exec::on_path("framework_tool") {
        return "framework_tool is not installed".into();
    }
    privileged(&["charge-limit", &limit.clamp(40, 100).to_string()])
}

/// Re-applies the saved Framework limit when the EC lost it; returns `ch` unchanged unless that succeeded.
pub fn maybe_reapply(cfg: &Value, ch: Value) -> Value {
    let Some(limit) = ch["limit"].as_f64() else { return ch };
    if ch["kind"] != "framework" || truthy(&cfg["boost"]) {
        return ch;
    }
    let wanted = charge_limit(cfg).clamp(40, 100);
    if limit == wanted as f64 {
        return ch;
    }
    let Some(helper) = helper_bin() else { return ch };
    let (code, _) = exec::run(&["sudo", "-n", &helper.to_string_lossy(), "charge-limit", &wanted.to_string()], secs(4));
    if code != 0 {
        return ch;
    }
    let mut next = ch;
    next["limit"] = json!(wanted);
    next["available"] = json!(true);
    next["reason"] = json!("");
    next
}

/// `re.search(r"percentage:\s+(\d+)", out)`; saturates instead of overflowing.
fn percentage(out: &str) -> Option<u64> {
    out.match_indices("percentage:").find_map(|(i, m)| {
        let rest = &out[i + m.len()..];
        let digits = rest.trim_start();
        if digits.len() == rest.len() {
            return None;
        }
        let d: String = digits.chars().take_while(char::is_ascii_digit).collect();
        (!d.is_empty()).then(|| d.parse().unwrap_or(u64::MAX))
    })
}

/// Runs every 30 s: returns before any fork while boost is off.
pub fn boost_check() -> (i32, String) {
    // load() forks busctl for a side without a valid saved profile, so decide from the raw file first.
    if !truthy(&config::raw()["boost"]) {
        return (0, "ok".into());
    }
    let mut cfg = config::load();
    let (_, out) = exec::run(&["upower", "-i", "/org/freedesktop/UPower/devices/DisplayDevice"], secs(4));
    let full = out.contains("fully-charged") || out.to_lowercase().contains("fully charged");
    if full || (percentage(&out).is_some_and(|p| p >= 100) && !power::on_battery()) {
        cfg["boost"] = json!(false);
        if let Err(e) = config::save(&cfg) {
            return (1, e.to_string());
        }
        let err = set_charge(charge_limit(&json!({"chargeLimit": cfg["boostSavedLimit"]})));
        return if err.is_empty() { (0, "restored".into()) } else { (1, err) };
    }
    (0, "holding".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximum_is_followed_by_up_to_three_digits() {
        let m = parse_max;
        assert_eq!(
            [m("Maximum charge level: 80%"), m("Maximum\n\n  100"), m("Maximum 1234"), m("Maximum x"), m("nothing 80"), m("Maximum Maximum 7")],
            [Some(80), Some(100), Some(123), None, None, Some(7)]
        );
    }

    #[test]
    fn percentage_needs_whitespace_then_digits() {
        assert_eq!([percentage("percentage:   100%"), percentage("percentage:100"), percentage("percentage: x percentage: 5")], [Some(100), None, Some(5)]);
    }

    #[test]
    fn busctl_values_follow_their_type() {
        let p = parse_busctl_value;
        assert_eq!(
            [p("b true\n"), p("u 7"), p("s \"hi\""), p("u x"), p("a(ii) 0"), p("")],
            [Some(json!(true)), Some(json!(7)), Some(json!("hi")), None, Some(json!("0")), None]
        );
    }
}
