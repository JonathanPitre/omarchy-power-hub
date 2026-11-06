use crate::paths;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

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
