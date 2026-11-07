use crate::config::truthy;
use crate::travel::valid_travel_snapshot;
use crate::{names::safe_name_v, paths};
use serde_json::{Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const MALFORMED_TRAVEL: &str = "Travel restore point is malformed; restore ownership manually";
const MALFORMED_DIM: &str = "Quick dim restore point is malformed; restore ownership manually";

/// Python `type(v) is int`: a JSON integer, never a bool or float.
pub fn is_int(v: &Value) -> bool {
    v.is_i64() || v.is_u64()
}

pub fn int_in(v: &Value, lo: i64, hi: i64) -> bool {
    v.as_i64().is_some_and(|n| (lo..=hi).contains(&n))
}

/// Python `float(v)`.
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Python `int(v)`.
pub fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn invalid(error: &str) -> Value {
    json!({"on": true, "invalid": true, "error": error})
}

/// Saved energy state: the default when absent, an invalid marker when unreadable or not an object.
pub fn read_energy(path: &Path, default: Value) -> Value {
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

/// Write `value` and its newline to a 0600 temp file, fsync it, then rename it over `path`.
#[allow(dead_code)] // wired by the write verbs
pub fn atomic_state(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let write = || -> io::Result<()> {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(serde_json::to_string(value).map_err(io::Error::other)?.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    };
    write().inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// An exclusive flock on `<run>/energy-controls.lock`, shared with ops.py, released on drop.
#[allow(dead_code)] // the file is held open for its flock
pub struct EnergyLock(File);

pub fn energy_lock() -> io::Result<EnergyLock> {
    let path = paths::plugin_run_dir().join("energy-controls.lock");
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new().write(true).create(true).truncate(true).open(path)?;
    file.lock()?;
    Ok(EnergyLock(file))
}

pub fn travel_path() -> PathBuf {
    paths::plugin_run_dir().join("travel.json")
}

pub fn travel_state() -> Value {
    let state = read_energy(&travel_path(), json!({"on": false}));
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

/// Whether Travel holds the CPU controls, and with them the remembered profiles.
pub fn cpu_owned() -> bool {
    let travel = travel_state();
    truthy(&travel["on"]) && travel.get("reduceCpu").is_none_or(truthy)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_output_keeps_key_order() {
        let v: Value = serde_json::from_str(r#"{"z":1,"a":2}"#).unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), r#"{"z":1,"a":2}"#);
    }

    #[test]
    fn long_floats_round_trip() {
        let v: Value = serde_json::from_str("0.30000000000000004").unwrap();
        assert_eq!(v.to_string(), "0.30000000000000004");
    }
}
