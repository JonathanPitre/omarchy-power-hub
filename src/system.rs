use crate::config::{self, truthy};
use crate::travel::{self, BrightnessCache};
use crate::{charge, exec, hypr, paths, power};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::Duration;

fn text(p: &Path) -> String {
    fs::read_to_string(p).unwrap_or_default()
}

/// Discrete GPU presence and runtime-suspend state, from PCI sysfs only.
pub fn gpu_status() -> Value {
    let mut devs: Vec<_> = fs::read_dir(paths::root("/sys/bus/pci/devices"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    devs.sort();
    for dev in devs {
        // boot_vga exists only on VGA-class devices; Optimus cards are 3D controllers without it.
        let boot_vga = text(&dev.join("boot_vga")).trim().to_string();
        let driver = fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        let discrete = boot_vga == "0" || (boot_vga.is_empty() && matches!(driver.as_str(), "nvidia" | "nouveau"));
        if text(&dev.join("class")).starts_with("0x03") && discrete {
            let asleep = text(&dev.join("power/runtime_status")).trim() == "suspended";
            return json!({"present": true, "asleep": asleep});
        }
    }
    json!({"present": false, "asleep": false})
}

/// `a(udu) N t pct state ...` from UPower's GetHistory; empty on any malformed row.
fn parse_history(raw: &str) -> Vec<Value> {
    let first = raw.trim().split('\n').next().unwrap_or("");
    let parts: Vec<&str> = first.split_whitespace().collect();
    let [kind, count, vals @ ..] = parts.as_slice() else { return vec![] };
    let Ok(n) = count.parse::<usize>() else { return vec![] };
    if *kind != "a(udu)" || vals.len() < n.saturating_mul(3) {
        return vec![];
    }
    vals.as_chunks::<3>()
        .0
        .iter()
        .take(n)
        .map(|r| Some(json!({"t": r[0].parse::<i64>().ok()?, "pct": r[1].parse::<f64>().ok()?, "charging": r[2].parse::<i64>().ok()? == 1})))
        .collect::<Option<_>>()
        .unwrap_or_default()
}

pub fn battery_history() -> Value {
    let timeout = Duration::from_secs(4);
    let (code, out) = exec::run(&["upower", "-e"], timeout);
    let device = out.lines().map(str::trim).find(|p| p.contains("battery_") && !p.ends_with("/DisplayDevice"));
    let Some(device) = device.filter(|_| code == 0) else { return json!([]) };
    let args = ["busctl", "call", "org.freedesktop.UPower", device, "org.freedesktop.UPower.Device", "GetHistory", "suu", "charge", "172800", "400"];
    let (code, out) = exec::run(&args, timeout);
    if code == 0 { json!(parse_history(&out)) } else { json!([]) }
}

/// Whether the Framework power LED lacks its passwordless `tee` rule.
pub fn led_sudo_needed() -> &'static str {
    let has_led = fs::read_dir(paths::root("/sys/class/leds")).is_ok_and(|d| {
        d.flatten().any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("chromeos") && name.ends_with(":power") && e.path().join("brightness").exists()
        })
    });
    if !power::framework() || !has_led {
        return "skip";
    }
    let (code, out) = exec::run(&["sudo", "-n", "-l"], Duration::from_secs(4));
    if code == 0 && out.contains("chromeos") && out.contains("/usr/bin/tee") { "ok" } else { "needed" }
}

/// First `[word]` in the kernel's lockdown file is the active mode.
fn lockdown_on() -> bool {
    let raw = text(&paths::root("/sys/kernel/security/lockdown"));
    let mode = raw.match_indices('[').find_map(|(i, _)| {
        let rest = &raw[i + 1..];
        rest.find(']').filter(|&j| j > 0).map(|j| &rest[..j])
    });
    mode.is_some_and(|m| m != "none")
}

fn bitwarden_running() -> bool {
    fs::read_dir(paths::root("/proc")).is_ok_and(|d| {
        d.flatten().any(|e| {
            e.file_name().to_string_lossy().starts_with(|c: char| c.is_ascii_digit())
                && fs::read(e.path().join("cmdline")).is_ok_and(|raw| String::from_utf8_lossy(&raw).replace('\0', " ").contains("/bitwarden/"))
        })
    })
}

fn unavailable(reason: &str) -> Value {
    json!({"available": false, "reason": reason})
}

pub fn hibernate_available() -> Value {
    if text(&paths::root("/sys/power/disk")).trim() == "[disabled]" {
        return if lockdown_on() {
            unavailable("kernel lockdown is blocking hibernation")
        } else if text(&paths::root("/proc/cmdline")).split_whitespace().any(|w| w == "nohibernate") {
            unavailable("the kernel was booted with nohibernate")
        } else if bitwarden_running() {
            unavailable("Bitwarden holds secret memory that blocks hibernation. Set SECURE_KEY_CONTAINER_BACKEND=mlock and restart Bitwarden.")
        } else {
            unavailable("an application holds secret memory, which blocks hibernation")
        };
    }
    let args = ["busctl", "call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "CanHibernate"];
    let out = exec::run(&args, Duration::from_secs(4)).1.to_lowercase();
    let ok = out.contains("yes") && !out.contains("challenge") && !out.contains("\"no\"");
    json!({"available": ok, "reason": if ok { "" } else { "hibernate is not available: check swap and a resume= kernel parameter" }})
}

/// Everything the panel shows, from one monitors query and at most one brightness read per display.
pub fn status() -> Value {
    let mut profiles = None;
    let cfg = config::load_with(&mut profiles);
    let monitors = hypr::monitors();
    let mut brightness = BrightnessCache::default();
    json!({
        "side": power::active_side(&cfg),
        "config": cfg,
        "travel": travel::travel_status(&monitors, &mut brightness, &mut profiles),
        "quickDim": travel::quick_dim_status(&monitors, &mut brightness),
        "charge": charge::maybe_reapply(&cfg, charge::charge_status()),
        "boost": truthy(&cfg["boost"]),
        "hibernate": hibernate_available(),
        "tweaks": config::tweak_state(),
        "framework": power::framework(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_rows_parse_or_the_whole_list_is_empty() {
        let rows = parse_history("a(udu) 2 100 85.5 1 200 84 2\n");
        assert_eq!(rows, [json!({"t": 100, "pct": 85.5, "charging": true}), json!({"t": 200, "pct": 84.0, "charging": false})]);
        assert!([parse_history("a(udu) 2 1 2 3"), parse_history("a(udu) 1 x 2 3"), parse_history("b 1"), parse_history("a(udu) -1"), parse_history("")].iter().all(Vec::is_empty));
    }
}
