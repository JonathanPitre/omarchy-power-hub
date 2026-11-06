use crate::config::{charge_limit, truthy};
use crate::{exec, paths};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const MIN_ADAPTER_WATTS: f64 = 15.0;

fn supplies(prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<_> = fs::read_dir(paths::root("/sys/class/power_supply"))
        .map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(prefix)).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn read_i64(p: PathBuf) -> Option<i64> {
    fs::read_to_string(p).ok()?.trim().parse().ok()
}

/// Best online USB-C source contract in watts, or None without UCSI.
fn ucsi_source_watts() -> Option<f64> {
    let ports = supplies("ucsi-source-psy-");
    if ports.is_empty() {
        return None;
    }
    let mut best = 0.0_f64;
    for port in ports {
        if fs::read_to_string(port.join("online")).map(|s| s.trim() != "1").unwrap_or(true) {
            continue;
        }
        if let (Some(v), Some(a)) = (read_i64(port.join("voltage_now")), read_i64(port.join("current_now"))) {
            best = best.max(v as f64 / 1e6 * (a as f64 / 1e6));
        }
    }
    Some(best)
}

/// Monitors and docks can feed a few watts back over USB-C, and ACPI and UPower count that as AC.
/// Only a source that can run the laptop counts on UCSI.
pub fn on_battery() -> bool {
    if let Some(w) = ucsi_source_watts() {
        return w < MIN_ADAPTER_WATTS;
    }
    let args = ["busctl", "get-property", "org.freedesktop.UPower", "/org/freedesktop/UPower", "org.freedesktop.UPower", "OnBattery"];
    exec::run(&args, Duration::from_secs(4)).1.to_lowercase().contains("true")
}

fn pack_discharging() -> bool {
    supplies("BAT").iter().any(|b| fs::read_to_string(b.join("status")).is_ok_and(|s| s.trim() == "Discharging"))
}

fn pack_percent() -> i64 {
    supplies("BAT").into_iter().filter_map(|b| read_i64(b.join("capacity"))).max().unwrap_or(0)
}

pub fn framework() -> bool {
    fs::read_to_string(paths::root("/sys/class/dmi/id/sys_vendor")).is_ok_and(|v| v.to_lowercase().contains("framework"))
}

/// Above the limit the Framework EC discharges the pack with the adapter connected.
fn draining_to_limit(cfg: &Value) -> bool {
    framework() && !truthy(&cfg["boost"]) && pack_percent() > charge_limit(cfg)
}

pub fn drain_to(cfg: &Value) -> i64 {
    if pack_discharging() && draining_to_limit(cfg) && !on_battery() { charge_limit(cfg) } else { 0 }
}

pub fn active_side(cfg: &Value) -> &'static str {
    if pack_discharging() && !draining_to_limit(cfg) || on_battery() { "battery" } else { "ac" }
}
