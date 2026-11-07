use crate::config::{save_tweaks, saved_prefs, truthy};
use crate::state::{cpu_owned, energy_lock};
use crate::{charge::privileged, data::panel_power_savings, exec, hypr, paths};
use serde_json::{Map, Value, json};
use std::{fs, io, time::Duration};

const TRAVEL_OWNS_CPU: &str = "Turn off Travel mode before changing CPU settings";
const UNIT: &str = "omarchy-power-hub-tweaks.service";
/// Hyprland option and the Lua table that sets it, `{}` standing for the value.
const EFFECT_OPTIONS: [(&str, &str); 2] = [
    ("animations:enabled", "animations = { enabled = {} }"),
    ("decoration:shadow:enabled", "decoration = { shadow = { enabled = {} } }"),
];

/// The option's boolean as Hyprland prints it: text after the last `:` of the first line.
fn hypr_option(name: &str) -> Option<bool> {
    let reply = hypr::request(&format!("getoption {name}"))?;
    match reply.lines().next().unwrap_or("").rsplit(':').next()?.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Live state of a saving tweak, or None when this machine cannot do it.
#[allow(dead_code)] // wired by Travel
pub fn tweak_on(key: &str) -> Option<bool> {
    if key == "abm" {
        return panel_power_savings();
    }
    let values: Option<Vec<bool>> = EFFECT_OPTIONS.iter().map(|(name, _)| hypr_option(name)).collect();
    values.map(|v| !v.contains(&true))
}

fn set_effects(on: bool) -> String {
    let mut prefs = saved_prefs();
    if on && prefs.get("effectsBefore").is_none() {
        let before: Map<String, Value> = EFFECT_OPTIONS.iter().map(|(name, _)| ((*name).into(), json!(hypr_option(name)))).collect();
        prefs["effectsBefore"] = Value::Object(before);
    }
    let before = if on { Value::Null } else { prefs.as_object_mut().and_then(|p| p.shift_remove("effectsBefore")).unwrap_or(Value::Null) };
    let mut errors = vec![];
    for (name, lua) in EFFECT_OPTIONS {
        let enabled = !on && before.get(name) != Some(&json!(false));
        if let Err(e) = hypr::eval_checked(&format!("hl.config({{ {} }})", lua.replace("{}", &enabled.to_string()))) {
            errors.push(if e.trim().is_empty() { format!("{name} could not be changed") } else { e.trim().into() });
        }
    }
    match save_tweaks(&prefs) {
        Ok(()) => errors.join(" "),
        Err(e) => e.to_string(),
    }
}

pub fn set_tweak(key: &str, on: bool) -> String {
    if key == "effects" { set_effects(on) } else { privileged(&[key, if on { "on" } else { "off" }]) }
}

fn write_unit() -> io::Result<()> {
    let dir = paths::home().join(".config/systemd/user");
    fs::create_dir_all(&dir)?;
    let bin = paths::bin_dir().join("power-hub");
    let unit = format!(
        "[Unit]\nDescription=Omarchy Power Hub device tweaks\n\n[Service]\nType=oneshot\nExecStart={} apply-tweaks\n\n[Install]\nWantedBy=default.target\n",
        bin.display()
    );
    fs::write(dir.join(UNIT), unit)?;
    exec::run(&["systemctl", "--user", "enable", "--now", UNIT], Duration::from_secs(8));
    Ok(())
}

fn apply_unlocked(key: &str, on: bool, persist: bool) -> String {
    let err = set_tweak(key, on);
    let mut prefs = saved_prefs();
    prefs[key] = json!(on);
    if persist {
        prefs[format!("{key}Pinned")] = json!(true);
    }
    let saved = save_tweaks(&prefs).and_then(|()| if persist { write_unit() } else { Ok(()) });
    saved.map_or_else(|e| e.to_string(), |()| err)
}

pub fn apply_tweak(key: &str, on: bool, persist: bool) -> String {
    if key != "turbo" {
        return apply_unlocked(key, on, persist);
    }
    let _lock = match energy_lock() {
        Ok(l) => l,
        Err(e) => return e.to_string(),
    };
    if cpu_owned() { TRAVEL_OWNS_CPU.into() } else { apply_unlocked(key, on, persist) }
}

pub fn apply_saved_tweaks() -> String {
    let prefs = saved_prefs();
    let owned = cpu_owned();
    let mut err = String::new();
    for key in ["wifi", "pcie", "usb", "audio", "abm", "effects", "turbo"] {
        if (key == "turbo" && owned) || !truthy(&prefs[format!("{key}Pinned")]) {
            continue;
        }
        let e = apply_tweak(key, truthy(&prefs[key]), false);
        if !e.is_empty() {
            err = e;
        }
    }
    if !owned && truthy(&prefs["eppPinned"]) && truthy(&prefs["epp"]) {
        let _lock = match energy_lock() {
            Ok(l) => l,
            Err(e) => return e.to_string(),
        };
        if let (false, Some(epp)) = (cpu_owned(), prefs["epp"].as_str()) {
            let e = privileged(&["epp", epp]);
            if !e.is_empty() {
                err = e;
            }
        }
    }
    err
}

fn save(prefs: &Value) -> String {
    save_tweaks(prefs).err().map(|e| e.to_string()).unwrap_or_default()
}

/// Forget a tweak's pin; every other key stays.
pub fn unpin(key: &str) -> String {
    let mut prefs = saved_prefs();
    if let Some(p) = prefs.as_object_mut() {
        p.shift_remove(&format!("{key}Pinned"));
    }
    save(&prefs)
}

pub fn daylight(on: bool) -> String {
    let mut prefs = saved_prefs();
    prefs["daylight"] = json!(on);
    save(&prefs)
}

pub fn cpu(which: &str, value: &str) -> String {
    let _lock = match energy_lock() {
        Ok(l) => l,
        Err(e) => return e.to_string(),
    };
    if cpu_owned() {
        return TRAVEL_OWNS_CPU.into();
    }
    let err = privileged(&[which, value]);
    if !err.is_empty() || which != "epp" {
        return err;
    }
    let mut prefs = saved_prefs();
    prefs["epp"] = json!(value);
    prefs["eppPinned"] = json!(true);
    save(&prefs)
}
