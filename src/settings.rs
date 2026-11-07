use crate::state::{atomic_state, cpu_owned, energy_lock, py_float, py_int, travel_path, travel_state};
use crate::config::{self, LID_ACTIONS, truthy};
use crate::{charge, exec, hypr, idle, paths, power, system, travel, tweaks};
use serde_json::{Value, json};
use std::time::Duration;
use std::{fs, io};

const WEEK: i64 = 7 * 24 * 3600;
const BEGIN: &str = "-- BEGIN Omarchy Power Hub lid action override";
const END: &str = "-- END Omarchy Power Hub lid action override";

fn io_err(r: io::Result<()>) -> String {
    r.err().map(|e| e.to_string()).unwrap_or_default()
}

fn weekly(n: i64) -> i64 {
    if n <= 0 { WEEK } else { n }
}

/// Python `idle.get(k) == v` for a number.
fn equals(v: Option<&Value>, want: i64) -> bool {
    v.is_some_and(|v| matches!(v, Value::Number(_) | Value::Bool(_)) && py_float(v) == Some(want as f64))
}

/// Rewrite `idle` in shell.json, which the shell watches and reloads whole on every write.
pub fn shell_idle(screensaver: i64, lock: i64) -> io::Result<String> {
    let path = paths::shell_file();
    let Some(Value::Object(mut data)) = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()) else {
        return Ok("shell.json unreadable".into());
    };
    let idle = data.entry("idle").or_insert_with(|| json!({}));
    let Some(idle) = idle.as_object_mut() else { return Ok("shell.json unreadable".into()) };
    let want = [("screensaver", weekly(screensaver)), ("lock", weekly(lock))];
    if want.iter().all(|(k, v)| equals(idle.get(*k), *v)) {
        return Ok(String::new());
    }
    for (k, v) in want {
        idle.insert(k.into(), json!(v));
    }
    fs::write(&path, serde_json::to_string_pretty(&data).map_err(io::Error::other)? + "\n")?;
    Ok(String::new())
}

fn int_of(src: &Value, key: &str) -> i64 {
    py_int(&src[key]).unwrap_or(0)
}

/// Displays going off first would close the saver, so it never starts. Mirrors effectiveSaver in Model.js.
pub fn effective_saver(src: &Value) -> i64 {
    let (saver, displays) = (int_of(src, "screensaver"), if truthy(&src["displaysOff"]) { int_of(src, "displaysOff") } else { 0 });
    if 0 < displays && displays <= saver { 0 } else { saver }
}

fn side_file(side: &str) -> &'static str {
    if side == "battery" { "battery" } else { "ac" }
}

pub fn remember_profile(side: &str, name: &str) -> io::Result<()> {
    let path = paths::profile_state_dir().join(side_file(side));
    fs::create_dir_all(path.parent().unwrap_or(&path))?;
    fs::write(path, format!("{name}\n"))
}

/// Omarchy's battery service reapplies the remembered profile on every power-source change, so Travel
/// holds both files at power-saver and restore writes the normal profiles back.
pub fn remember_travel_profiles(cfg: Option<&Value>, held: Option<&str>) -> io::Result<()> {
    for side in ["ac", "battery"] {
        if let Some(name) = held.map(String::from).or_else(|| cfg?.get(side).map(|s| canonical_profile(s["profile"].as_str().unwrap_or_default(), side))) {
            remember_profile(side, &name)?;
        }
    }
    Ok(())
}

pub fn canonical_profile(value: &str, side: &str) -> String {
    config::alias(value).map_or_else(
        || if side == "ac" && config::offered(&mut None).iter().any(|p| p == "performance") { "performance" } else { "balanced" }.into(),
        String::from,
    )
}

pub fn apply_profile(side: &str, mode: &str, live: bool) -> io::Result<String> {
    let which = side_file(side);
    let name = canonical_profile(mode, which);
    if !live || which != power::active_side(&config::load()) {
        return remember_profile(which, &name).map(|()| String::new());
    }
    Ok(match exec::run(&["omarchy-powerprofiles-set", which, &name], Duration::from_secs(4)) {
        (0, _) => String::new(),
        (_, out) => out.trim().into(),
    })
}

/// Drop every BEGIN..END block with the newline that follows it; an unterminated BEGIN stays.
fn without_blocks(text: &str) -> String {
    let (mut out, mut rest) = (String::new(), text);
    while let Some(start) = rest.find(BEGIN) {
        let after = start + BEGIN.len();
        let Some(end) = rest[after..].find(END) else { break };
        out += &rest[..start];
        rest = &rest[after + end + END.len()..];
        rest = rest.strip_prefix('\n').unwrap_or(rest);
    }
    out + rest
}

pub fn write_lid_block() -> io::Result<()> {
    let path = paths::bindings_file();
    let original = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut text = without_blocks(&original);
    let exe = paths::bin_dir().join("power-hub");
    let exe = exe.display();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text += &format!(
        "{BEGIN}\nhl.unbind(\"switch:on:Lid Switch\")\nhl.unbind(\"switch:off:Lid Switch\")\no.bind(\"switch:on:Lid Switch\", nil, \"{exe} lid-closed\", {{ locked = true }})\no.bind(\"switch:off:Lid Switch\", nil, \"{exe} lid-opened\", {{ locked = true }})\n{END}\n"
    );
    if text != original {
        fs::write(path, text)?;
        hypr::request("reload");
    }
    Ok(())
}

fn hibernate_delay_applied() -> i64 {
    let text = fs::read_to_string(paths::root("/etc/systemd/sleep.conf.d/90-omarchy-power-hub.conf")).unwrap_or_default();
    text.split('\n')
        .find_map(|l| l.strip_prefix("HibernateDelaySec=").filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())))
        .map_or(0, |d| d.parse().unwrap_or(i64::MAX))
}

pub fn apply_hibernate_delay(seconds: i64) -> String {
    if seconds == hibernate_delay_applied() {
        return String::new();
    }
    let info = system::hibernate_available();
    if seconds > 0 && !truthy(&info["available"]) {
        return match info["reason"].as_str() {
            Some(r) if !r.is_empty() => r.into(),
            _ => "hibernate is not available".into(),
        };
    }
    charge::privileged(&["hibernate-delay", &seconds.to_string()])
}

pub fn set_key(side: &str, key: &str, value: &str) -> String {
    if !matches!(side, "ac" | "battery") {
        return "bad source".into();
    }
    let mut cfg = config::load();
    match key {
        "screensaver" | "displaysOff" | "lock" | "sleep" | "hibernateAfter" => match value.trim().parse::<i64>() {
            Ok(n) => cfg[side][key] = json!(n.max(0)),
            Err(_) => return "bad value".into(),
        },
        "lid" if LID_ACTIONS.contains(&value) => cfg[side][key] = json!(value),
        "lid" => return "bad lid action".into(),
        "profile" if config::alias(value).is_some() => cfg[side][key] = json!(canonical_profile(value, side)),
        "profile" => return "bad profile".into(),
        _ => return "bad key".into(),
    }
    let src = cfg[side].clone();
    if key == "profile" {
        let _lock = match energy_lock() {
            Ok(l) => l,
            Err(e) => return e.to_string(),
        };
        let owned = cpu_owned();
        if let Err(e) = config::save(&cfg) {
            return e.to_string();
        }
        // Travel holds Omarchy's remembered profiles; restore writes this one back.
        return if owned { String::new() } else { apply_profile(side, src["profile"].as_str().unwrap_or_default(), true).unwrap_or_else(|e| e.to_string()) };
    }
    if let Err(e) = config::save(&cfg) {
        return e.to_string();
    }
    if truthy(&travel_state()["on"]) || side != power::active_side(&cfg) {
        return String::new();
    }
    match key {
        "screensaver" | "displaysOff" | "lock" => shell_idle(effective_saver(&src), int_of(&src, "lock")).unwrap_or_else(|e| e.to_string()),
        "hibernateAfter" if int_of(&src, key) > 0 => apply_hibernate_delay(int_of(&src, key)),
        "lid" => io_err(write_lid_block()),
        _ => String::new(),
    }
}

/// Apply the active side's profile, idle, hibernate delay and lid action under the energy lock.
/// An error Python lets escape (an unwritable file) is an `Err`, which ends the verb with exit 1.
pub fn apply_active() -> io::Result<String> {
    let _lock = energy_lock()?;
    let cfg = config::load();
    let mut state = travel_state();
    let owned = truthy(&state["on"]) && state.get("reduceCpu").is_none_or(truthy);
    let side = power::active_side(&cfg);
    let src = &cfg[side];
    let mut err = String::new();
    if owned {
        if !truthy(&state["complete"]) {
            err = state["error"].as_str().filter(|e| !e.is_empty()).unwrap_or("Travel mode restoration is incomplete").into();
        } else if !travel::older_version(&state) {
            if !truthy(&state["controls"]["profile"]["available"]) {
                let normal = canonical_profile(src["profile"].as_str().unwrap_or_default(), side);
                if travel::listed_profiles().unwrap_or_default().contains(&normal) {
                    err = apply_profile(side, &normal, true)?;
                }
            }
            if err.is_empty() {
                err = travel::apply_cpu_overlay(&mut state).unwrap_or_else(|e| format!("Could not save CPU overlay progress: {e}"));
            }
            if !err.is_empty() {
                state["complete"] = json!(false);
                state["error"] = json!(err);
                atomic_state(&travel_path(), &state)?;
            }
        }
    }
    let profile_error = if owned { String::new() } else { apply_profile(side, src["profile"].as_str().unwrap_or_default(), true)? };
    let mut source_error = shell_idle(effective_saver(src), int_of(src, "lock"))?;
    if int_of(src, "hibernateAfter") > 0 {
        source_error = format!("{source_error} {}", apply_hibernate_delay(int_of(src, "hibernateAfter"))).trim().into();
    }
    write_lid_block()?;
    Ok(format!("{err} {profile_error} {source_error}").trim().into())
}

/// `apply`: the active side, then the pinned tweaks (after the lock is released), then the HID wakeup when the lid is open.
pub fn apply() -> String {
    let active = apply_active();
    // A failed profile apply must not leave a unit that still names ops.py.
    tweaks::refresh_unit();
    let active = match active {
        Ok(e) => e,
        Err(e) => return e.to_string(),
    };
    let mut err = format!("{active} {}", tweaks::apply_saved_tweaks()).trim().to_string();
    if idle::lid_is_open(&idle::lid_text()) {
        err = format!("{err} {}", charge::privileged(&["hid-wakeup", "on"])).trim().into();
    }
    err
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_go_with_one_newline_and_an_unterminated_begin_stays() {
        let block = format!("{BEGIN}\nx\n{END}\n");
        assert_eq!(without_blocks(&format!("a\n{block}{block}b\n")), "a\nb\n");
        assert_eq!(without_blocks(&format!("a\n{BEGIN}\nx\n")), format!("a\n{BEGIN}\nx\n"));
        assert_eq!(without_blocks(&format!("{BEGIN}{END}")), "");
    }
}
