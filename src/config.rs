use crate::{exec, paths};
use serde_json::{Map, Value, json};
use std::time::Duration;
use std::{fs, io};

const LID_ACTIONS: [&str; 4] = ["nothing", "display-off", "suspend", "hibernate"];

/// Python truthiness of a JSON value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `int(cfg.chargeLimit or 80)`, with 80 for anything unparseable.
pub fn charge_limit(cfg: &Value) -> i64 {
    let v = &cfg["chargeLimit"];
    if !truthy(v) {
        return 80;
    }
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(80),
        Value::String(s) => s.trim().parse().unwrap_or(80),
        Value::Bool(_) => 1,
        _ => 80,
    }
}

fn parse_profiles(out: &str) -> Vec<String> {
    out.split("\"Profile\" s \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next().map(String::from))
        .collect()
}

/// Profiles power-profiles-daemon offers, or None when it does not answer.
pub fn power_profiles() -> Option<Vec<String>> {
    let args = ["busctl", "get-property", "net.hadess.PowerProfiles", "/net/hadess/PowerProfiles", "net.hadess.PowerProfiles", "Profiles"];
    let (code, out) = exec::run(&args, Duration::from_secs(4));
    (code == 0).then(|| parse_profiles(&out))
}

fn alias(name: &str) -> Option<&'static str> {
    match name {
        "auto" | "balanced" => Some("balanced"),
        "saver" | "power-saver" => Some("power-saver"),
        "full" | "performance" => Some("performance"),
        _ => None,
    }
}

fn read_json(p: &std::path::Path) -> Value {
    fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

/// The saved file as is (Null when missing or malformed); never forks.
pub fn raw() -> Value {
    read_json(&paths::cfg_file())
}

/// Saved settings merged over defaults; asks power-profiles-daemon only when a side lacks a valid profile.
pub fn load() -> Value {
    let raw = raw();
    let mut available: Option<Option<Vec<String>>> = None;
    let mut cfg = Map::new();
    for (k, v) in [("chargeLimit", json!(80)), ("boost", json!(false)), ("boostSavedLimit", json!(80))] {
        cfg.insert(k.into(), v);
    }
    for side in ["ac", "battery"] {
        let src = raw[side].as_object().cloned().unwrap_or_default();
        let profile = match src.get("profile").and_then(Value::as_str).and_then(alias) {
            Some(p) => p,
            None => {
                let list = available.get_or_insert_with(power_profiles);
                let performance = list.as_ref().is_some_and(|l| l.iter().any(|p| p == "performance"));
                if side == "ac" && performance { "performance" } else { "balanced" }
            }
        };
        let mut base = Map::new();
        for (k, v) in [("screensaver", json!(150)), ("displaysOff", json!(0)), ("lock", json!(300)), ("sleep", json!(0)), ("hibernateAfter", json!(0)), ("lid", json!("suspend"))] {
            base.insert(k.into(), src.get(k).cloned().unwrap_or(v));
        }
        // Older configs saved "system-default", which logind handles as suspend.
        if !base["lid"].as_str().is_some_and(|l| LID_ACTIONS.contains(&l)) {
            base.insert("lid".into(), json!("suspend"));
        }
        base.insert("profile".into(), json!(profile));
        cfg.insert(side.into(), Value::Object(base));
    }
    for k in ["chargeLimit", "boost", "boostSavedLimit"] {
        if let Some(v) = raw.get(k) {
            cfg.insert(k.into(), v.clone());
        }
    }
    Value::Object(cfg)
}

pub fn save(cfg: &Value) -> io::Result<()> {
    let p = paths::cfg_file();
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(p, serde_json::to_string_pretty(cfg).map_err(io::Error::other)? + "\n")
}

pub fn tweak_state() -> Value {
    match read_json(&paths::tweaks_file()) {
        v @ Value::Object(_) => v,
        _ => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_limit_follows_python_int_or_80() {
        let cl = |v: Value| charge_limit(&json!({ "chargeLimit": v }));
        assert_eq!(
            [cl(json!(90)), cl(json!(0)), cl(json!(null)), cl(json!("85")), cl(json!("abc")), cl(json!(90.7)), cl(json!([1]))],
            [90, 80, 80, 85, 80, 90, 80]
        );
        assert_eq!(charge_limit(&json!({})), 80);
    }

    #[test]
    fn truthy_matches_python() {
        assert!([json!(null), json!(false), json!(0), json!(0.0), json!(""), json!([]), json!({})].iter().all(|v| !truthy(v)));
        assert!([json!(true), json!(1), json!("0"), json!([0]), json!({"a": 1})].iter().all(truthy));
    }

    #[test]
    fn profiles_are_the_strings_after_profile_keys() {
        let out = r#"aa{sv} 2 2 "Profile" s "power-saver" "Driver" s "platform_profile" 1 "Profile" s "balanced""#;
        assert_eq!(parse_profiles(out), ["power-saver", "balanced"]);
        assert!(parse_profiles("aa{sv} 0").is_empty());
    }
}
