//! Checks on the QML and Model.js source the panel ships. 
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn file(name: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn has_all(text: &str, name: &str, wanted: &[&str]) {
    for want in wanted {
        assert!(text.contains(want), "{name} lacks {want:?}");
    }
}

fn has_none(text: &str, name: &str, unwanted: &[&str]) {
    for no in unwanted {
        assert!(!text.contains(no), "{name} still has {no:?}");
    }
}

/// The text between the braces of every `AccentButton {`.
fn button_bodies(text: &str) -> Vec<&str> {
    text.match_indices("AccentButton").filter_map(|(at, _)| {
        let word_start = text[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let rest = text[at + "AccentButton".len()..].trim_start();
        let open = text.len() - rest.len();
        if !word_start || !rest.starts_with('{') {
            return None;
        }
        let mut depth = 0;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&text[open + 1..open + i]);
                    }
                }
                _ => {}
            }
        }
        None
    }).collect()
}

mod backend_switch {
    use super::*;

    #[test]
    fn hot_paths_use_the_binary() {
        let service = file("Service.qml");
        for verb in [r#""config""#, r#""boost-check""#, r#""led-sudo-needed""#, r#""helper-needed""#, r#""idle-wake-poll""#, r#""displays", "sync""#] {
            assert!(!service.contains(&format!(r#""python3", root.ops, {verb}"#)), "{verb} still runs Python");
            assert!(service.contains(&format!("root.hub, {verb}")), "{verb} does not run the binary");
        }
        has_all(&file("PlanSection.qml"), "PlanSection.qml", &[r#"[root.hub, "status"]"#, r#"[root.hub, "gpu"]"#]);
        let panel = file("Panel.qml");
        has_all(&panel, "Panel.qml", &[r#""battery-history"]"#]);
        has_none(&panel, "Panel.qml", &[r#""python3", root.planOpsBin, "battery-history""#]);
    }

    #[test]
    fn plan_section_runs_no_python() {
        has_none(&file("PlanSection.qml"), "PlanSection.qml", &["python3", "ops.py"]);
    }

    #[test]
    fn energy_results_read_the_verb_from_the_new_position() {
        let plan = file("PlanSection.qml");
        has_all(&plan, "PlanSection.qml", &[r#"act.command[1] === "travel" || act.command[1] === "quick-dim""#]);
        has_none(&plan, "PlanSection.qml", &["act.command[2]"]);
    }

    #[test]
    fn apply_runs_when_the_helper_check_cannot_start() {
        let service = file("Service.qml");
        has_all(&service, "Service.qml", &["onRunningChanged: if (!running && !applyProc.running && !helperPkexec.running) applyProc.running = true"]);
        has_none(&service, "Service.qml", &["onExited: function(code)"]);
    }

    #[test]
    fn dark_displays_are_not_blanked_again() {
        let service = file("Service.qml");
        has_all(&service, "Service.qml", &[r#"out === "blank" && root.displaysOff && root.displaysIdle"#]);
        has_none(&service, "Service.qml", &["\n        if (root.displaysOff && root.displaysIdle"]);
    }

    #[test]
    fn poll_uses_the_binary() {
        has_all(&file("PowerService.qml"), "PowerService.qml", &[r#"[root.binDir + "/power-hub", "data"]"#]);
        assert!(!Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/battery-plus-data").exists());
        has_all(&file("src/action.rs"), "src/action.rs", &["data::collect()"]);
    }

    #[test]
    fn binary_is_executable_and_matches_checksum() {
        let hub = Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/power-hub");
        assert!(fs::metadata(&hub).unwrap().permissions().mode() & 0o111 != 0);
        let want = file("bin/power-hub.sha256");
        let sum = std::process::Command::new("sha256sum").arg(&hub).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&sum.stdout).split_whitespace().next(), want.split_whitespace().next());
    }

    #[test]
    fn failed_backend_shows_one_clear_line() {
        has_all(&file("PowerService.qml"), "PowerService.qml", &["property bool backendFailed"]);
        has_all(&file("Panel.qml"), "Panel.qml", &["service.backendFailed", "Power Hub needs an x86-64 system to run its backend"]);
    }
}

mod button_chrome {
    use super::*;

    #[test]
    fn plan_and_panel_buttons_use_accent_button() {
        for name in ["PlanSection.qml", "Panel.qml"] {
            let src = file(name);
            assert!(!button_bodies(&src).is_empty(), "{name}");
            let bare = src.lines().any(|l| l.trim_start().strip_prefix("Button").is_some_and(|r| r.trim_start().starts_with('{')));
            assert!(!bare, "{name} has a bare Button");
        }
        has_all(&file("AccentButton.qml"), "AccentButton.qml", &["bordered: true", "hot || selected || active"]);
    }

    #[test]
    fn switches_use_accent_switch() {
        for name in ["PlanSection.qml", "Panel.qml"] {
            has_none(&file(name), name, &["ToggleSwitch {", "CheckBox {"]);
        }
        has_all(&file("Panel.qml"), "Panel.qml", &["AccentSwitch {"]);
    }

    /// A disabled button kept the selected fill and label colour, so Dim screen's "Restore" looked live while Travel mode owned the display.
    #[test]
    fn disabled_accent_buttons_grey_out() {
        has_all(&file("AccentButton.qml"), "AccentButton.qml", &["opacity: enabled ? 1 : 0.35"]);
    }

    #[test]
    fn energy_focus_targets_exist() {
        has_all(&file("PlanSection.qml"), "PlanSection.qml", &["id: energyTravelButton", "id: energyDimButton"]);
    }

    /// Bare `Color` does not resolve to the theme singleton inside plugins.
    #[test]
    fn theme_colors_use_qualified_singleton() {
        for name in ["PlanSection.qml", "Panel.qml"] {
            let src = file(name);
            for (at, _) in src.match_indices("Color.") {
                let before = src[..at].chars().next_back();
                assert!(before.is_some_and(|c| c == '.' || c.is_alphanumeric() || c == '_'), "{name} has a bare Color. at byte {at}");
            }
        }
    }

    #[test]
    fn idle_grid_shows_plugged_in_then_battery() {
        let src = file("PlanSection.qml");
        assert!(src.find(r#"side: "ac", title: "PLUGGED IN""#).unwrap() < src.find(r#"side: "battery", title: "BATTERY""#).unwrap());
        has_all(&src, "PlanSection.qml", &["icon: \"󰚥\"", "icon: \"󰁹\"", "delegate: Dropdown {", r#"text: "POWER & BATTERY""#, r#"{ key: "profile", label: "Power mode" }"#]);
        has_none(&src, "PlanSection.qml", &["NOW", "sidePicked", "system-default"]);
    }

    /// Dropdown sets its own value before emitting changed, so comparing against it would skip every save.
    #[test]
    fn grid_dropdown_saves_against_config_not_own_value() {
        let src = file("PlanSection.qml");
        has_none(&src, "PlanSection.qml", &["v !== value"]);
        has_all(&src, "PlanSection.qml", &[r#"if (v !== String(saved)) root.call(["set""#]);
    }

    #[test]
    fn bar_icon_is_not_a_chip() {
        let src = file("Panel.qml");
        let body = src.match_indices("BarIconButton").find_map(|(at, w)| src[at + w.len()..].trim_start().strip_prefix('{')).expect("BarIconButton has no body");
        assert!(!body[..body.find('}').unwrap()].contains("bordered: true"));
    }
}

mod copy {
    use super::*;

    #[test]
    fn plan_section_uses_glossary() {
        let src = file("PlanSection.qml");
        let model = file("Model.js");
        has_all(&src, "PlanSection.qml", &[r#"side: "ac", title: "PLUGGED IN""#, r#"side: "battery", title: "BATTERY""#, r#"{ key: "displaysOff", label: "Displays off""#]);
        has_all(&model, "Model.js", &["{ value: \"power-saver\", label: \"󰌪 Power saver\"", r#"{ value: "display-off", label: "Displays off" }"#, r#"{ value: "nothing", label: "Do nothing" }"#]);
        has_none(&src, "PlanSection.qml", &[r#"{ side: "ac", title: "AC" }"#, "On battery", "Blank screens", r#"label: "Power-saver""#, r#"{ value: "nothing", label: "Nothing" }"#]);
    }

    #[test]
    fn hero_and_tooltip_use_glossary() {
        let model = file("Model.js");
        let panel = file("Panel.qml");
        has_all(&model, "Model.js", &[r#"{ state: "PLUGGED IN · CHARGING", time: "" }"#, r#"{ state: "PLUGGED IN · NOT CHARGING", time: "" }"#,
                                       r#"{ state: "PLUGGED IN", time: "" }"#, r#"{ state: "DISCHARGING", time: t ? t + " REMAINING" : "" }"#]);
        has_none(&model, "Model.js", &["ON AC", "ON BATTERY"]);
        has_all(&panel, "Panel.qml", &[r#": "Plugged in""#]);
        has_none(&panel, "Panel.qml", &["Running on AC"]);
    }

    #[test]
    fn charge_section_names_backends() {
        let src = file("PlanSection.qml");
        has_all(&src, "PlanSection.qml", &["CHARGE LIMIT", "BATTERY HEALTH", "80% is recommended for everyday use", "Set the BIOS charge limit to match this value",
                                            r#""Needs permission""#, r#"root.call(["health""#]);
        has_none(&src, "PlanSection.qml", &["Desk use"]);
    }

    #[test]
    fn model_helpers_exist() {
        let model = file("Model.js");
        has_all(&model, "Model.js", &["function drawValue(b)", "function historyBars(points, now)"]);
        has_none(&model, "Model.js", &["function drawLabel(b)", "function ecLabel(raw)"]);
    }

    /// UPower has no rate for ~90 s after unplug and refreshes every ~30 s.
    #[test]
    fn panel_time_and_watts_prefer_sysfs_poll() {
        has_all(&file("Panel.qml"), "Panel.qml", &["Number(service.battery.watts) || Math.abs(Number(dev.changeRate)",
                                                   "service.battery.status === root.batStatus && Number(service.battery.minutes)"]);
    }

    #[test]
    fn travel_labels_and_auto_off_on_plug_in() {
        has_all(&file("PlanSection.qml"), "PlanSection.qml", &[r#"label: "Max brightness""#, r#"label: "Max refresh rate""#]);
        has_all(&file("Service.qml"), "Service.qml", &["root.lastBatteryState === UPowerDeviceState.Discharging", r#"command: [root.hub, "travel", "off"]"#]);
    }

    /// A process and a login shell every 2 s kept the CPU awake with the panel closed.
    #[test]
    fn no_process_polling_at_idle() {
        let service = file("Service.qml");
        has_none(&service, "Service.qml", &["lid-state", "pgrep"]);
        has_all(&service, "Service.qml", &[r#"appId === "org.omarchy.screensaver""#]);
    }

    #[test]
    fn power_profile_follows_battery_state_not_only_on_battery() {
        let service = file("Service.qml");
        let at = service.find("function onStateChanged()").unwrap();
        let window: String = service[at..].chars().take(500).collect();
        assert!(window.contains("applyProfileDebounce.restart()"));
    }

    #[test]
    fn pinned_tweaks_read_the_saved_prefs_not_tlp() {
        let data = file("src/paths.rs") + &file("src/data.rs");
        has_all(&data, "src/paths.rs and src/data.rs", &["power-hub-tweaks.json"]);
        has_none(&data, "src/paths.rs and src/data.rs", &["/etc/tlp.d/02-battery-plus.conf"]);
    }

    #[test]
    fn new_power_tweaks_are_listed_and_routed() {
        let model = file("Model.js");
        has_all(&model, "Model.js", &[r#"key: "abm""#, r#"key: "effects""#]);
        has_all(&file("src/action.rs"), "src/action.rs", &[r#""wifi" | "usb" | "pcie" | "audio" | "abm" | "effects""#]);
        has_all(&file("bin/ops-priv"), "bin/ops-priv", &["  abm)\n"]);
    }

    #[test]
    fn panel_holds_estimate_after_plug_or_unplug() {
        has_all(&file("Panel.qml"), "Panel.qml", &["estimating: root.estimating", "interval: 30000; onTriggered: root.estimating = false", r#"if (root.lastStatus !== "Unknown")"#]);
    }

    #[test]
    fn panel_battery_usage_not_sparkline() {
        let panel = file("Panel.qml");
        has_all(&panel, "Panel.qml", &[r#"text: "BATTERY USAGE""#, r#"label: root.charging ? "Charging at" : "Power draw""#, "property bool historyOpen: false",
                                       "Model.drawValue", "battery-history", "24h ago"]);
        has_none(&panel, "Panel.qml", &["function drawLabel", "ecLabel", r#"label: "Temp""#]);
    }

    #[test]
    fn service_drops_since_open_history() {
        has_none(&file("PowerService.qml"), "PowerService.qml", &["property var history:", "historyMax"]);
    }

    #[test]
    fn removed_install_cards_leave_no_trace() {
        let names = ["Install Wi-Fi suspend fix", "Install power tuning", "wifiFix", "WIFI_FIX_INFO", "showWifiFixBanner", "install-wifi-fix"];
        for name in ["Panel.qml", "Model.js", "PowerService.qml", "src/data.rs", "src/action.rs"] {
            has_none(&file(name), name, &names);
        }
        for name in ["battery-dashboard", "battery-common.sh"] {
            assert!(!Path::new(env!("CARGO_MANIFEST_DIR")).join("extras").join(name).exists(), "{name}");
        }
    }
}

mod idle_wiring {
    use super::*;

    #[test]
    fn lock_timer_outlives_the_saver_displays_off_closes() {
        has_all(&file("Service.qml"), "Service.qml", &["root.displaysIdle && root.lockTimeout >= root.displaysTimeout", r#""idle-lock""#]);
    }
}

mod no_python {
    use super::*;
    use std::process::Command;

    fn root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    fn files_under(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files_under(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn no_python_is_tracked() {
        let listing = Command::new("git").args(["ls-files", "-z"]).current_dir(root()).output().unwrap().stdout;
        for name in String::from_utf8(listing).unwrap().split('\0').filter(|n| !n.is_empty()) {
            assert!(!name.ends_with(".py"), "{name} is Python");
            let head = fs::read(root().join(name)).unwrap_or_default();
            let first = head.split(|b| *b == b'\n').next().unwrap_or_default();
            assert!(!(first.starts_with(b"#!") && String::from_utf8_lossy(first).contains("python")), "{name} has a Python shebang");
        }
    }

    #[test]
    fn nothing_starts_python() {
        let mut paths = Vec::new();
        for dir in ["src", "bin", "scripts", "extras"] {
            files_under(&root().join(dir), &mut paths);
        }
        paths.push(root().join(".github/workflows/ci.yml"));
        paths.extend(fs::read_dir(root()).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "qml" || e == "js")));
        for path in paths {
            if path.file_name().is_some_and(|n| n == "power-hub") {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&path) {
                assert!(!text.contains("python3"), "{} starts python3", path.display());
            }
        }
    }
}
