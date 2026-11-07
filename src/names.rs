use serde_json::Value;

/// `re.match(r"^[...]+$", s)`: `$` also matches before one trailing newline.
pub fn matches_set(s: &str, ok: impl Fn(char) -> bool) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    !s.is_empty() && s.chars().all(ok)
}

pub fn safe_name(s: &str) -> bool {
    matches_set(s, |c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// `display_power.safe_name` on any JSON value: a truthy value is tested as its text.
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
