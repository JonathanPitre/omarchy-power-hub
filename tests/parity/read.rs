//! The read-only verbs, ported from `tests/test_parity.py` class by class under the same test names.
use crate::support::{Cursors, Outcome, Pci, Replay, Sandbox, assert_same_files, golden_step, replay, run, same_json, stopped_short, verify};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::process::Command;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

/// What the replay compares by default.
const D: Replay = Replay::direct();
pub(crate) const CONFIG_FILE: &str = Sandbox::CONFIG_PATH;
pub(crate) const PROFILES: &[&str] = &["power-saver", "balanced", "performance"];
const USB: &str = "USBC000:001";

pub(crate) fn j(r: &Outcome) -> Value {
    serde_json::from_str(&r.stdout).unwrap()
}

pub(crate) fn text(r: &Outcome) -> &str {
    r.stdout.trim()
}

pub(crate) fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// `assertEqual` on parsed JSON: 91 equals 91.0.
pub(crate) fn same(left: &Value, right: Value) {
    assert!(same_json(left, &right), "{left} != {right}");
}

mod sandbox_fixtures {
    use super::*;

    #[test]
    fn test_light_fixtures_land_under_root() {
        let sb = Sandbox::new();
        sb.dmi("Framework", "Laptop 16");
        sb.power_led("none", 0, 100);
        sb.light_sensor(40, Some("0.5"));
        let read = |p: &str| fs::read_to_string(sb.root.join(p)).unwrap();
        assert_eq!(read("sys/class/dmi/id/product_name"), "Laptop 16\n");
        assert_eq!(read("sys/class/leds/chromeos:white:power/max_brightness"), "100\n");
        assert_eq!(read("sys/bus/iio/devices/iio:device0/in_illuminance_scale"), "0.5\n");
    }
}

mod gpu_parity {
    use super::*;

    #[test]
    fn test_suspended_discrete_gpu() {
        let s = Sandbox::new();
        s.pci("0000:c1:00.0", Pci { boot_vga: Some("1"), ..Pci::default() });
        s.pci("0000:c2:00.0", Pci { boot_vga: Some("0"), runtime: "suspended", ..Pci::default() });
        replay(&s, "GpuParity.test_suspended_discrete_gpu", &["gpu"], D);
        assert_eq!(j(&s.rust(&["gpu"])), json!({"present": true, "asleep": true}));
    }

    #[test]
    fn test_optimus_card_without_boot_vga() {
        let s = Sandbox::new();
        s.pci("0000:c2:00.0", Pci { cls: "0x030200", driver: Some("nvidia"), runtime: "active", ..Pci::default() });
        replay(&s, "GpuParity.test_optimus_card_without_boot_vga", &["gpu"], D);
    }

    #[test]
    fn test_no_gpu() {
        let s = Sandbox::new();
        replay(&s, "GpuParity.test_no_gpu", &["gpu"], D);
    }

    /// Expected values were recorded from the old implementation on the same trees.
    #[test]
    fn test_which_gpu_counts_and_whether_it_sleeps() {
        let cases = [
            ("active discrete", vec![("0000:c1:00.0", Pci { boot_vga: Some("1"), runtime: "suspended", ..Pci::default() }),
                                     ("0000:c2:00.0", Pci { boot_vga: Some("0"), ..Pci::default() })], json!({"present": true, "asleep": false})),
            ("nvidia boot gpu is skipped", vec![("0000:01:00.0", Pci { boot_vga: Some("1"), driver: Some("nvidia"), ..Pci::default() }),
                                                ("0000:02:00.0", Pci { cls: "0x030200", driver: Some("nvidia"), runtime: "suspended", ..Pci::default() })],
             json!({"present": true, "asleep": true})),
            ("integrated only", vec![("0000:00:02.0", Pci { boot_vga: Some("1"), ..Pci::default() }),
                                     ("0000:00:03.0", Pci { cls: "0x038000", driver: Some("amdgpu"), ..Pci::default() }),
                                     ("0000:01:00.0", Pci { cls: "0x020000", boot_vga: Some("0"), ..Pci::default() })], json!({"present": false, "asleep": false})),
            ("optimus 3d controller", vec![("0000:00:02.0", Pci { boot_vga: Some("1"), ..Pci::default() }),
                                           ("0000:01:00.0", Pci { cls: "0x030200", driver: Some("nvidia"), runtime: "suspended", ..Pci::default() })],
             json!({"present": true, "asleep": true})),
        ];
        for (label, devices, want) in cases {
            let s = Sandbox::new();
            for (addr, pci) in devices {
                s.pci(addr, pci);
            }
            assert_eq!(j(&s.rust(&["gpu"])), want, "{label}");
        }
    }
}

mod config_parity {
    use super::*;

    const CASES: &[&str] = &[
        "",
        "{",
        r#"{"ac": 5}"#,
        r#"{"ac": {"profile": "auto", "lid": "system-default"}}"#,
        r#"{"chargeLimit": "abc"}"#,
        r#"{"chargeLimit": 90, "boost": true, "battery": {"lock": 60}}"#,
    ];

    #[test]
    fn test_config_matches_for_every_shape() {
        for raw in CASES {
            let s = Sandbox::new();
            s.profiles(PROFILES);
            s.discharging(70);
            s.write(CONFIG_FILE, raw);
            let rust = if *raw == r#"{"ac": 5}"# {
                s.rust(&["config"])
            } else {
                replay(&s, "ConfigParity.test_config_matches_for_every_shape", &["config"], D)
            };
            assert_eq!(rust.code, 0, "{raw}");
            j(&rust);
        }
    }

    #[test]
    fn test_default_ac_profile_needs_performance_to_be_offered() {
        for names in [&PROFILES[..2], PROFILES] {
            let s = Sandbox::new();
            s.profiles(names);
            replay(&s, "ConfigParity.test_default_ac_profile_needs_performance_to_be_offered", &["config"], D);
        }
    }

    #[test]
    fn test_missing_power_profiles_daemon_defaults_ac_to_balanced() {
        let s = Sandbox::new();
        s.stub("omarchy-powerprofiles-list", "", 0, "");
        let rust = replay(&s, "ConfigParity.test_missing_power_profiles_daemon_defaults_ac_to_balanced", &["config"], D);
        assert_eq!(j(&rust)["config"]["ac"]["profile"], "balanced");
    }

    #[test]
    fn test_saved_config_is_loaded() {
        let s = Sandbox::new();
        s.write(CONFIG_FILE, r#"{"chargeLimit": 90}"#);
        assert_eq!(j(&s.rust(&["config"]))["config"]["chargeLimit"], 90);
    }
}

mod power_source_parity {
    use super::*;

    fn same(s: &Sandbox, name: &str, expected_ac: u32, side: &str, expected_drain: u32) {
        for verb in ["ac", "drain-to", "config"] {
            replay(s, name, &[verb], D);
        }
        assert_eq!(text(&s.rust(&["ac"])), expected_ac.to_string());
        assert_eq!(text(&s.rust(&["drain-to"])), expected_drain.to_string());
        assert_eq!(j(&s.rust(&["config"]))["side"], side);
    }

    #[test]
    fn test_ucsi_under_15_watts_counts_as_battery() {
        let s = Sandbox::new();
        s.profiles(&["balanced"]);
        s.ucsi(USB, true, 5.0, 2.0);
        same(&s, "PowerSourceParity.test_ucsi_under_15_watts_counts_as_battery", 0, "battery", 0);
    }

    #[test]
    fn test_ucsi_65_watt_source_is_ac() {
        let s = Sandbox::new();
        s.profiles(&["balanced"]);
        s.ucsi(USB, true, 20.0, 3.25);
        s.ucsi("USBC000:002", false, 5.0, 0.0);
        same(&s, "PowerSourceParity.test_ucsi_65_watt_source_is_ac", 1, "ac", 0);
    }

    #[test]
    fn test_no_ucsi_falls_back_to_upower() {
        for (flag, ac, side) in [("true", 0, "battery"), ("false", 1, "ac")] {
            let s = Sandbox::new();
            s.profiles(&["balanced"]);
            s.stub("busctl", "OnBattery", 0, &format!("b {flag}\n"));
            same(&s, "PowerSourceParity.test_no_ucsi_falls_back_to_upower", ac, side, 0);
        }
    }

    #[test]
    fn test_framework_draining_to_limit_stays_on_ac_side() {
        let s = Sandbox::new();
        s.profiles(&["balanced"]);
        s.framework(true);
        s.discharging(90);
        s.ucsi(USB, true, 20.0, 3.25);
        s.write(CONFIG_FILE, r#"{"chargeLimit": 85}"#);
        same(&s, "PowerSourceParity.test_framework_draining_to_limit_stays_on_ac_side", 1, "ac", 85);
    }

    #[test]
    fn test_discharging_below_limit_is_battery_side() {
        let s = Sandbox::new();
        s.profiles(&["balanced"]);
        s.framework(true);
        s.discharging(50);
        s.ucsi(USB, true, 20.0, 3.25);
        same(&s, "PowerSourceParity.test_discharging_below_limit_is_battery_side", 1, "battery", 0);
    }

    /// Expected values were recorded from the old implementation on the same supplies.
    #[test]
    fn test_which_plan_applies_while_the_pack_discharges() {
        let brick: &[(&str, f64, f64)] = &[("USBC000:003", 20.0, 5.0)];
        let monitor: &[(&str, f64, f64)] = &[("USBC000:005", 12.0, 0.42)];
        // (label, supplies, status, percent, framework, boost, ac verb, side, drain-to)
        let cases = [
            ("EC drains to the limit on a brick", brick, "Discharging", 98, true, false, "1", "ac", "80"),
            ("not charging on a brick", brick, "Not charging", 98, true, false, "1", "ac", "0"),
            ("discharging at the limit is the 800 mA fault", brick, "Discharging", 80, true, false, "1", "battery", "0"),
            ("boost has no limit to drain to", brick, "Discharging", 98, true, true, "1", "battery", "0"),
            ("other vendors keep the battery plan", brick, "Discharging", 98, false, false, "1", "battery", "0"),
            ("a monitor feed above the limit", monitor, "Discharging", 98, true, false, "0", "battery", "0"),
        ];
        for (label, supplies, status, percent, framework, boost, ac, side, drain) in cases {
            let s = Sandbox::new();
            s.profiles(&["balanced"]);
            if framework {
                s.framework(true);
            }
            s.discharging(percent);
            s.write("/sys/class/power_supply/BAT0/status", &format!("{status}\n"));
            for (name, volts, amps) in supplies {
                s.ucsi(name, true, *volts, *amps);
            }
            s.write(CONFIG_FILE, &json!({"chargeLimit": 80, "boost": boost}).to_string());
            assert_eq!(text(&s.rust(&["ac"])), ac, "{label}");
            assert_eq!(j(&s.rust(&["config"]))["side"], side, "{label}");
            assert_eq!(text(&s.rust(&["drain-to"])), drain, "{label}");
        }
    }
}

pub(crate) const UPOWER_LIST: &str = "/org/freedesktop/UPower/devices/line_power_AC\n/org/freedesktop/UPower/devices/battery_BAT1\n/org/freedesktop/UPower/devices/DisplayDevice\n";

mod charge_parity {
    use super::*;

    const TEE_RULE: &str = "(root) NOPASSWD: /usr/bin/tee /sys/class/leds/chromeos:white:power/brightness\n";

    fn helper_rule(s: &Sandbox) -> String {
        format!("    (root) NOPASSWD: {}/ops-priv\n", s.helper_dir().display())
    }

    fn ops_priv(s: &Sandbox, args: &str) -> String {
        format!("sudo -n {}/ops-priv {args}", s.helper_dir().display())
    }

    #[test]
    fn test_helper_needed_when_installed_copy_differs() {
        for tampered in [true, false] {
            let s = Sandbox::new();
            if tampered {
                s.install_helper(true);
            }
            s.stub("sudo", "-n -l", 0, &helper_rule(&s));
            let rs = replay(&s, "ChargeParity.test_helper_needed_when_installed_copy_differs", &["helper-needed"], D);
            assert_eq!(text(&rs), "needed");
            assert!(!rs.calls_text().contains("sudo -n -l"));
        }
    }

    #[test]
    fn test_helper_ok_when_bytes_match_and_sudo_rule_present() {
        const T: &str = "ChargeParity.test_helper_ok_when_bytes_match_and_sudo_rule_present";
        let s = Sandbox::new();
        s.install_helper(false);
        s.stub("sudo", "-n -l", 0, &helper_rule(&s));
        assert_eq!(text(&replay(&s, T, &["helper-needed"], D)), "ok");
        s.stub("sudo", "-n -l", 0, "    (root) NOPASSWD: /somewhere/else\n");
        assert_eq!(text(&replay(&s, T, &["helper-needed"], D)), "needed");
    }

    #[test]
    fn test_boost_off_prints_ok_and_forks_nothing() {
        let s = Sandbox::new();
        s.config(json!({"boost": false}));
        let r = s.rust(&["boost-check"]);
        assert_eq!((text(&r), r.calls.len()), ("ok", 0));
    }

    #[test]
    fn test_boost_off_forks_nothing_without_valid_saved_profiles() {
        for raw in [None, Some(""), Some("{"), Some(r#"{"boost": false}"#), Some(r#"{"boost": 0, "ac": 5}"#)] {
            let s = Sandbox::new();
            if let Some(raw) = raw {
                s.write(CONFIG_FILE, raw);
            }
            let r = if raw == Some(r#"{"boost": 0, "ac": 5}"#) { // the golden is the old crash on a non-object side
                s.rust(&["boost-check"])
            } else {
                replay(&s, "ChargeParity.test_boost_off_forks_nothing_without_valid_saved_profiles", &["boost-check"], D)
            };
            assert_eq!((r.code, text(&r), r.calls.len()), (0, "ok", 0), "{raw:?}");
        }
    }

    #[test]
    fn test_boost_restores_saved_limit_when_full() {
        let s = Sandbox::new();
        s.framework(true);
        s.install_helper(false);
        s.stub("sudo", "charge-limit 75", 0, "");
        s.stub("upower", "-i", 0, "state: fully-charged\npercentage: 100%\n");
        s.config(json!({"boost": true, "boostSavedLimit": 75}));
        let rs = replay(&s, "ChargeParity.test_boost_restores_saved_limit_when_full", &["boost-check"], D);
        assert_eq!(text(&rs), "restored");
        assert!(rs.calls_text().contains(&ops_priv(&s, "charge-limit 75")));
        assert_eq!(s.read_config()["boost"], false);
    }

    #[test]
    fn test_boost_holds_below_full_and_pkexec_failure_is_reported() {
        const T: &str = "ChargeParity.test_boost_holds_below_full_and_pkexec_failure_is_reported";
        let s = Sandbox::new();
        s.framework(true);
        s.stub("upower", "-i", 0, "state: charging\npercentage: 60%\n");
        s.config(json!({"boost": true}));
        replay(&s, T, &["boost-check"], D);
        assert_eq!(text(&s.rust(&["boost-check"])), "holding");
        s.stub("upower", "-i", 0, "state: fully-charged\n");
        s.config(json!({"boost": true, "boostSavedLimit": 70}));
        let rs = replay(&s, T, &["boost-check"], D);
        assert_eq!((rs.code, text(&rs)), (1, "privilege required"));
        assert!(rs.calls_text().contains("charge-limit 70"));
    }

    #[test]
    fn test_boost_full_battery_spellings_follow_power_source() {
        let cases = [("percentage: 100%\n", true, "restored"), ("percentage: 100%\n", false, "holding"), ("state: fully charged\n", false, "restored")];
        for (out, ac, expected) in cases {
            let s = Sandbox::new();
            s.framework(true);
            s.install_helper(false);
            s.stub("sudo", "charge-limit 75", 0, "");
            s.stub("upower", "-i", 0, out);
            if ac {
                s.ucsi(USB, true, 20.0, 3.25);
            } else {
                s.ucsi(USB, true, 5.0, 2.0);
            }
            s.config(json!({"boost": true, "boostSavedLimit": 75}));
            let rs = replay(&s, "ChargeParity.test_boost_full_battery_spellings_follow_power_source", &["boost-check"], D);
            assert_eq!(text(&rs), expected);
            assert_eq!(rs.calls_text().contains(&ops_priv(&s, "charge-limit 75")), expected == "restored");
            assert_eq!(s.read_config()["boost"], expected == "holding");
        }
    }

    #[test]
    fn test_led_sudo_skip_when_not_framework() {
        let s = Sandbox::new();
        s.write("/sys/class/leds/chromeos:white:power/brightness", "0\n");
        s.stub("sudo", "-n -l", 0, TEE_RULE);
        let rs = replay(&s, "ChargeParity.test_led_sudo_skip_when_not_framework", &["led-sudo-needed"], D);
        assert_eq!((text(&rs), rs.calls.len()), ("skip", 0));
    }

    #[test]
    fn test_led_sudo_ok_or_needed_on_framework_with_led() {
        const T: &str = "ChargeParity.test_led_sudo_ok_or_needed_on_framework_with_led";
        let s = Sandbox::new();
        s.framework(true);
        replay(&s, T, &["led-sudo-needed"], D);
        assert_eq!(text(&s.rust(&["led-sudo-needed"])), "skip");
        s.write("/sys/class/leds/chromeos:white:power/brightness", "0\n");
        replay(&s, T, &["led-sudo-needed"], D);
        assert_eq!(text(&s.rust(&["led-sudo-needed"])), "needed");
        s.stub("sudo", "-n -l", 0, TEE_RULE);
        replay(&s, T, &["led-sudo-needed"], D);
        assert_eq!(text(&s.rust(&["led-sudo-needed"])), "ok");
    }

    #[test]
    fn test_battery_history_parses_upower_rows() {
        let s = Sandbox::new();
        s.stub("upower", "-e", 0, UPOWER_LIST);
        s.stub("busctl", "GetHistory", 0, "a(udu) 2 1730000000 85.5 1 1730000600 84.000000 2\n");
        replay(&s, "ChargeParity.test_battery_history_parses_upower_rows", &["battery-history"], D);
        assert_eq!(j(&s.rust(&["battery-history"])), json!([
            {"t": 1730000000, "pct": 85.5, "charging": true}, {"t": 1730000600, "pct": 84.0, "charging": false}]));
        assert!(s.rust(&["battery-history"]).calls_text().contains(
            "busctl call org.freedesktop.UPower /org/freedesktop/UPower/devices/battery_BAT1 org.freedesktop.UPower.Device GetHistory suu charge 172800 400"));
    }

    #[test]
    fn test_battery_history_empty_without_battery() {
        let s = Sandbox::new();
        s.stub("upower", "-e", 0, "/org/freedesktop/UPower/devices/line_power_AC\n/org/freedesktop/UPower/devices/DisplayDevice\n");
        s.stub("busctl", "GetHistory", 0, "a(udu) 1 1 2 1\n");
        let rs = replay(&s, "ChargeParity.test_battery_history_empty_without_battery", &["battery-history"], D);
        assert_eq!(j(&rs), json!([]));
        assert!(!rs.calls_text().contains("GetHistory"));
    }
}

pub(crate) const CPU: &str = "/sys/devices/system/cpu";
pub(crate) const TRAVEL_FILE: &str = "run/omarchy-power-hub/travel.json";
pub(crate) const DIM_FILE: &str = "run/omarchy-power-hub/quick-dim.json";
pub(crate) const TWEAKS_FILE: &str = "home/.config/omarchy/power-hub-tweaks.json";
pub(crate) const TRAVEL_MALFORMED: &str = "Travel restore point is malformed; restore ownership manually";
pub(crate) const UNREADABLE: &str = "Saved settings are unreadable; restore ownership manually";

pub(crate) fn laptop_row() -> Value {
    json!({"name": "eDP-1", "width": 1920, "height": 1200, "refreshRate": 165.004, "transform": 0,
           "availableModes": ["1920x1200@165.00Hz", "1920x1200@59.94Hz", "1920x1200@48.00Hz", "1920x1080@60.00Hz"]})
}

pub(crate) fn snapshot_monitor() -> Value {
    json!({"output": "eDP-1", "name": "eDP-1", "width": 1920, "height": 1200, "x": 0, "y": 0,
           "refreshRate": 165.004, "scale": 1.25, "transform": 0})
}

pub(crate) fn frozen() -> Value {
    json!({"profile": {"available": true, "detail": "Frozen profile"}, "turbo": {"available": true, "detail": "Frozen turbo"},
           "epp": {"available": true, "detail": "Frozen epp"}, "brightness": {"available": true, "detail": "Frozen brightness"},
           "refresh": {"available": false, "detail": "Laptop refresh capped at 59.94 Hz"}})
}

/// `{**base, **changes}`: a changed key keeps its place.
pub(crate) fn merged(base: Value, changes: Value) -> Value {
    let (Value::Object(mut base), Value::Object(changes)) = (base, changes) else { panic!("merged takes objects") };
    base.extend(changes);
    Value::Object(base)
}

pub(crate) fn travel_snapshot(changes: Value) -> Value {
    merged(json!({"on": true, "complete": true, "reduceCpu": true, "source": "battery", "error": "",
                  "pending": ["profile", "turbo", "brightness", "refresh"], "profile": "balanced",
                  "cpu": {"turbo": true, "epp": []}, "brightness": {"monitor": "eDP-1", "before": 70},
                  "monitor": snapshot_monitor(), "controls": frozen()}), changes)
}

fn status_of(s: &Sandbox, name: &str) -> Value {
    let d = j(&replay(s, name, &["status"], D));
    assert!(d.get("wifiFix").is_none(), "rust data carries the removed wifiFix key");
    d
}

pub(crate) fn laptop(s: &Sandbox, brightness: &str) {
    s.monitors(&[laptop_row()]);
    s.stub("omarchy", "brightness display --monitor eDP-1", 0, &format!("{brightness}\n"));
}

mod status_parity {
    use super::*;

    const NO_DIM: fn() -> Value = || json!({"on": false, "available": false, "brightness": null, "error": ""});

    #[test]
    fn test_idle_laptop() {
        let s = Sandbox::new();
        s.profiles(PROFILES);
        s.config(json!({"chargeLimit": 75}));
        s.discharging(70);
        laptop(&s, "55");
        s.write(&format!("{CPU}/intel_pstate/no_turbo"), "0\n");
        s.write(&format!("{CPU}/cpu0/cpufreq/energy_performance_preference"), "balance_performance\n");
        s.write(&format!("{CPU}/cpu0/cpufreq/energy_performance_available_preferences"), "default performance balance_power power\n");
        s.stub("busctl", "CanHibernate", 0, "s \"yes\"\n");
        let st = status_of(&s, "StatusParity.test_idle_laptop");
        assert_eq!((&st["side"], &st["boost"], &st["framework"], &st["config"]["chargeLimit"]),
                   (&json!("battery"), &json!(false), &json!(false), &json!(75)));
        assert_eq!(st["travel"]["phase"], "off");
        assert_eq!(st["travel"]["controls"], json!({
            "profile": {"available": true, "detail": "Power saver"},
            "turbo": {"available": true, "detail": "CPU turbo off"},
            "epp": {"available": true, "detail": "Energy-saving CPU preference"},
            "brightness": {"available": true, "detail": "Laptop brightness capped at 40%"},
            "refresh": {"available": true, "detail": "Laptop refresh capped at 59.94 Hz"}}));
        assert_eq!(st["quickDim"], json!({"on": false, "available": true, "brightness": 55, "error": ""}));
        assert_eq!(st["hibernate"], json!({"available": true, "reason": ""}));
        assert_eq!(st["tweaks"], json!({}));
    }

    #[test]
    fn test_idle_laptop_without_cpu_controls_or_fast_panel() {
        const T: &str = "StatusParity.test_idle_laptop_without_cpu_controls_or_fast_panel";
        let s = Sandbox::new();
        s.monitors(&[merged(laptop_row(), json!({"refreshRate": 60.0}))]);
        let st = status_of(&s, T);
        let controls = &st["travel"]["controls"];
        assert_eq!(controls["refresh"], json!({"available": true, "detail": "Laptop refresh is 60 Hz (already at or below 60 Hz)"}));
        assert_eq!(controls["brightness"], json!({"available": false, "detail": "Brightness cannot be read"}));
        assert!(["profile", "turbo", "epp"].iter().all(|k| controls[k]["available"] == false));
        s.monitors(&[merged(laptop_row(), json!({"availableModes": ["1920x1200@165.00Hz"]}))]);
        assert_eq!(status_of(&s, T)["travel"]["controls"]["refresh"], json!({"available": false, "detail": "No supported mode at or below 60 Hz"}));
    }

    #[test]
    fn test_travel_on_valid_snapshot_reports_frozen_controls() {
        const T: &str = "StatusParity.test_travel_on_valid_snapshot_reports_frozen_controls";
        let s = Sandbox::new();
        laptop(&s, "55");
        s.write(TRAVEL_FILE, &travel_snapshot(json!({})).to_string());
        let st = status_of(&s, T);
        assert_eq!((&st["travel"]["phase"], &st["travel"]["reduceCpu"], &st["travel"]["error"]), (&json!("on"), &json!(true), &json!("")));
        assert_eq!(st["travel"]["controls"], frozen());
        let error = "Power saver was replaced by balanced";
        s.write(TRAVEL_FILE, &travel_snapshot(json!({"complete": false, "error": error})).to_string());
        let st = status_of(&s, T);
        assert_eq!((&st["travel"]["phase"], &st["travel"]["error"]), (&json!("incomplete"), &json!(error)));
        assert_eq!(st["travel"]["controls"], frozen());
    }

    #[test]
    fn test_legacy_travel_file_and_partial_controls() {
        const T: &str = "StatusParity.test_legacy_travel_file_and_partial_controls";
        let s = Sandbox::new();
        s.write(TRAVEL_FILE, r#"{"on": true, "monitors": [], "brightness": null, "controls": {"turbo": {"available": "yes"}, "epp": {"available": false}}}"#);
        let st = status_of(&s, T);
        assert_eq!((&st["travel"]["phase"], &st["travel"]["reduceCpu"]), (&json!("incomplete"), &json!(true)));
        assert_eq!(st["travel"]["controls"]["epp"], json!({"available": false, "detail": "Energy-saving CPU preference"}));
        assert_eq!(st["travel"]["controls"]["turbo"], json!({"available": false, "detail": "CPU turbo off"}));
        s.write(TRAVEL_FILE, r#"{"on": true, "complete": true, "reduceCpu": 0, "monitors": []}"#);
        assert_eq!(status_of(&s, T)["travel"]["reduceCpu"], false);
    }

    #[test]
    fn test_malformed_travel_file_reports_restore_error() {
        let bad = |changes: Value| (travel_snapshot(changes).to_string(), TRAVEL_MALFORMED);
        let snapshots = [
            ("{".to_string(), UNREADABLE),
            ("[]".to_string(), "Saved settings are malformed"),
            (r#"{"on": "yes"}"#.to_string(), TRAVEL_MALFORMED),
            (r#"{"on": true}"#.to_string(), TRAVEL_MALFORMED),
            bad(json!({"pending": ["profile", "bogus"]})),
            bad(json!({"pending": ["profile"]})),
            bad(json!({"monitor": merged(snapshot_monitor(), json!({"name": "eDP 1"}))})),
            bad(json!({"monitor": merged(snapshot_monitor(), json!({"scale": 0}))})),
            bad(json!({"monitor": merged(snapshot_monitor(), json!({"width": true}))})),
            bad(json!({"controls": merged(frozen(), json!({"turbo": {"available": true}}))})),
            bad(json!({"cpu": {"turbo": true, "epp": [{"cpu": 0, "before": "power", "target": null}, {"cpu": 0, "before": "power", "target": null}]}})),
            bad(json!({"tweaks": ["bogus"]})),
            bad(json!({"reduceCpu": false})),
        ];
        for (raw, error) in snapshots {
            let s = Sandbox::new();
            s.write(TRAVEL_FILE, &raw);
            let st = status_of(&s, "StatusParity.test_malformed_travel_file_reports_restore_error");
            assert_eq!((&st["travel"]["phase"], &st["travel"]["error"]), (&json!("incomplete"), &json!(error)), "{raw}");
        }
    }

    #[test]
    fn test_valid_snapshots_with_epp_and_tweaks() {
        const T: &str = "StatusParity.test_valid_snapshots_with_epp_and_tweaks";
        let s = Sandbox::new();
        let epp = json!([{"cpu": 0, "before": "balance_performance", "target": "power"}, {"cpu": 1, "before": "performance", "target": null}]);
        let state = travel_snapshot(json!({"cpu": {"turbo": null, "epp": epp}, "tweaks": ["abm"], "monitor": null, "brightness": null, "profile": null,
                                           "pending": ["epp:0", "epp:1", "tweak:abm"],
                                           "controls": merged(frozen(), json!({"refresh": {"available": false, "detail": "none"}}))}));
        s.write(TRAVEL_FILE, &state.to_string());
        assert_eq!(status_of(&s, T)["travel"]["error"], "");
        s.write(TRAVEL_FILE, &merged(state, json!({"pending": ["epp:0", "tweak:abm"]})).to_string());
        assert_eq!(status_of(&s, T)["travel"]["error"], TRAVEL_MALFORMED);
    }

    /// Expected values were recorded from the old implementation.
    #[test]
    fn test_a_tweak_the_plugin_does_not_know_is_malformed_even_when_pending_agrees() {
        let point = |tweaks: Value, pending: Value| travel_snapshot(json!({"cpu": {"turbo": null, "epp": []}, "tweaks": tweaks, "pending": pending,
            "monitor": null, "brightness": null, "profile": null, "controls": merged(frozen(), json!({"refresh": {"available": false, "detail": "none"}}))}));
        for (tweaks, pending, error) in [(json!(["abm"]), json!(["tweak:abm"]), ""), (json!(["rm"]), json!(["tweak:rm"]), TRAVEL_MALFORMED),
                                         (json!([]), json!(["tweak:abm"]), TRAVEL_MALFORMED)] {
            let s = Sandbox::new();
            s.write(TRAVEL_FILE, &point(tweaks.clone(), pending).to_string());
            assert_eq!(j(&s.rust(&["status"]))["travel"]["error"], error, "{tweaks}");
        }
    }

    #[test]
    fn test_a_performance_only_epp_is_not_a_saving_capability() {
        let s = Sandbox::new();
        s.write(&format!("{CPU}/cpu0/cpufreq/energy_performance_preference"), "performance\n");
        s.write(&format!("{CPU}/cpu0/cpufreq/energy_performance_available_preferences"), "performance\n");
        assert_eq!(j(&s.rust(&["status"]))["travel"]["controls"]["epp"]["available"], false);
    }

    #[test]
    fn test_unreadable_quick_dim_file() {
        let s = Sandbox::new();
        laptop(&s, "55");
        fs::create_dir_all(s.run.join("omarchy-power-hub/quick-dim.json")).unwrap();
        assert_eq!(status_of(&s, "StatusParity.test_unreadable_quick_dim_file")["quickDim"],
                   json!({"on": true, "available": true, "brightness": null, "error": UNREADABLE}));
        assert_eq!(s.rust(&["status"]).calls_text().matches("omarchy brightness").count(), 1); // Travel's read only
    }

    #[test]
    fn test_quick_dim_state_files() {
        let message = "Quick dim restore point is malformed; restore ownership manually";
        let cases = [
            (json!({"on": true, "monitor": "eDP-1", "brightness": 82, "error": "stuck"}), "40", json!(40), "stuck"),
            (json!({"on": true, "monitor": "DP-9", "brightness": 82, "error": ""}), "40", json!(null), ""),
            (json!({"on": false}), "55", json!(55), ""),
            (json!({"on": true, "monitor": "eDP-1", "brightness": 101}), "40", json!(null), message),
            (json!({"on": true, "monitor": "eDP-1", "brightness": 82.0}), "40", json!(null), message),
            (json!({"on": true, "monitor": "a b", "brightness": 82}), "40", json!(null), message),
            (json!({"on": "yes"}), "40", json!(null), message),
        ];
        for (state, shown, brightness, error) in cases {
            let s = Sandbox::new();
            laptop(&s, shown);
            s.write(DIM_FILE, &state.to_string());
            let dim = status_of(&s, "StatusParity.test_quick_dim_state_files")["quickDim"].clone();
            assert_eq!((&dim["brightness"], &dim["error"]), (&brightness, &json!(error)), "{state}");
            assert_eq!(dim["on"], state["on"] == true || state["on"] == "yes");
        }
    }

    #[test]
    fn test_brightness_must_be_a_plain_percentage() {
        const T: &str = "StatusParity.test_brightness_must_be_a_plain_percentage";
        for out in ["101", "-1", "40%", "", "abc", "1234"] {
            let s = Sandbox::new();
            laptop(&s, out);
            assert_eq!(status_of(&s, T)["quickDim"], NO_DIM(), "{out}");
        }
        let s = Sandbox::new();
        laptop(&s, "55");
        s.stub("omarchy", "brightness display --monitor eDP-1", 1, "55\n");
        assert!(status_of(&s, T)["quickDim"]["brightness"].is_null());
    }

    fn framework_limit(s: &Sandbox, keys: Value) {
        s.framework(true);
        s.install_helper(false);
        s.config(keys);
    }

    #[test]
    fn test_stale_framework_limit_is_reapplied() {
        let s = Sandbox::new();
        framework_limit(&s, json!({"chargeLimit": 80}));
        s.charge_limit(90);
        replay(&s, "StatusParity.test_stale_framework_limit_is_reapplied", &["status"], D);
        assert!(s.rust(&["status"]).calls_text().contains("charge-limit 80"));
        assert_eq!(j(&s.rust(&["status"]))["charge"], json!({
            "kind": "framework", "available": true, "reason": "", "limit": 80, "tool": true, "health": null, "policy": null}));
    }

    #[test]
    fn test_failed_reapply_keeps_the_stale_limit() {
        let s = Sandbox::new();
        framework_limit(&s, json!({"chargeLimit": 80}));
        s.stub("sudo", "charge-limit 80", 1, "");
        s.charge_limit(90);
        let st = status_of(&s, "StatusParity.test_failed_reapply_keeps_the_stale_limit");
        assert_eq!((&st["charge"]["limit"], &st["charge"]["available"]), (&json!(90), &json!(true)));
        assert!(s.rust(&["status"]).calls_text().contains(&format!("sudo -n {}/ops-priv charge-limit 80", s.helper_dir().display())));
    }

    #[test]
    fn test_reapplied_limit_is_clamped_to_40_100() {
        for (saved, sent) in [(30, 40), (120, 100)] {
            let s = Sandbox::new();
            framework_limit(&s, json!({"chargeLimit": saved}));
            s.charge_limit(90);
            assert_eq!(status_of(&s, "StatusParity.test_reapplied_limit_is_clamped_to_40_100")["charge"]["limit"], sent);
            assert!(s.rust(&["status"]).calls_text().contains(&format!("charge-limit {sent}")));
        }
    }

    #[test]
    fn test_framework_limit_that_matches_is_left_alone() {
        const T: &str = "StatusParity.test_framework_limit_that_matches_is_left_alone";
        let s = Sandbox::new();
        framework_limit(&s, json!({"chargeLimit": 85}));
        s.charge_limit(85);
        assert_eq!(status_of(&s, T)["charge"]["limit"], 85);
        assert!(!s.rust(&["status"]).calls_text().contains("charge-limit 85"));
        s.config(json!({"chargeLimit": 70, "boost": true}));
        let st = status_of(&s, T);
        assert_eq!((&st["charge"]["limit"], &st["boost"]), (&json!(85), &json!(true)));
    }

    #[test]
    fn test_framework_without_helper_needs_permission() {
        let s = Sandbox::new();
        s.framework(true);
        s.config(json!({"chargeLimit": 80}));
        assert_eq!(status_of(&s, "StatusParity.test_framework_without_helper_needs_permission")["charge"], json!({
            "kind": "framework", "available": false, "reason": "permission", "limit": null, "tool": true, "health": null, "policy": null}));
        assert!(!s.rust(&["status"]).calls_text().contains("sudo"));
    }

    #[test]
    fn test_framework_with_unreadable_limit() {
        const T: &str = "StatusParity.test_framework_with_unreadable_limit";
        let s = Sandbox::new();
        s.framework(true);
        s.install_helper(false);
        s.stub("sudo", "charge-limit", 1, "sudo: a password is required\n");
        assert_eq!(status_of(&s, T)["charge"]["reason"], "permission");
        s.stub("sudo", "charge-limit", 0, "EC said no\n");
        assert_eq!(status_of(&s, T)["charge"]["reason"], "EC said no");
    }

    #[test]
    fn test_non_framework_reports_upower_thresholds() {
        const T: &str = "StatusParity.test_non_framework_reports_upower_thresholds";
        let s = Sandbox::new();
        s.stub("upower", "-e", 0, UPOWER_LIST);
        s.stub("busctl", "ChargeThresholdSupported", 0, "b true\n");
        s.stub("busctl", "ChargeThresholdEnabled", 0, "b true\n");
        s.stub("busctl", "ChargeThresholdSettingsSupported", 0, "u 3\n");
        s.stub("busctl", "ChargeStartThreshold", 0, "u 40\n");
        s.stub("busctl", "ChargeEndThreshold", 0, "u 80\n");
        assert_eq!(status_of(&s, T)["charge"], json!({
            "kind": "upower", "available": true, "reason": "", "limit": null, "tool": false, "health": true, "policy": "40-80"}));
        s.stub("busctl", "ChargeThresholdEnabled", 0, "b false\n");
        s.stub("busctl", "ChargeThresholdSettingsSupported", 0, "u 4\n");
        let charge = status_of(&s, T)["charge"].clone();
        assert_eq!((&charge["health"], &charge["policy"]), (&json!(false), &json!("firmware")));
    }

    /// Expected values were recorded from the old implementation on the same replies.
    #[test]
    fn test_upower_thresholds_unsupported_or_end_only() {
        let charge = |supported: &str, enabled: &str| {
            let s = Sandbox::new();
            s.stub("upower", "-e", 0, UPOWER_LIST);
            s.stub("busctl", "ChargeThresholdSupported", 0, supported);
            s.stub("busctl", "ChargeThresholdEnabled", 0, enabled);
            s.stub("busctl", "ChargeThresholdSettingsSupported", 0, "u 2\n");
            s.stub("busctl", "ChargeEndThreshold", 0, "u 80\n");
            j(&s.rust(&["status"]))["charge"].clone()
        };
        assert_eq!(charge("b false\n", "b false\n"), json!({
            "kind": "none", "available": false, "reason": "no charge threshold", "limit": null, "tool": false, "health": null, "policy": null}));
        for (enabled, health) in [("b false\n", false), ("b true\n", true)] {
            assert_eq!(charge("b true\n", enabled), json!({
                "kind": "upower", "available": true, "reason": "", "limit": null, "tool": false, "health": health, "policy": "80"}));
        }
    }

    #[test]
    fn test_non_framework_without_thresholds() {
        let s = Sandbox::new();
        assert_eq!(status_of(&s, "StatusParity.test_non_framework_without_thresholds")["charge"], json!({
            "kind": "none", "available": false, "reason": "no charge threshold", "limit": null, "tool": false, "health": null, "policy": null}));
    }

    #[test]
    fn test_hibernate_blocked_by_lockdown_nohibernate_bitwarden_and_swap() {
        const T: &str = "StatusParity.test_hibernate_blocked_by_lockdown_nohibernate_bitwarden_and_swap";
        let secret = "an application holds secret memory, which blocks hibernation";
        let bitwarden = "Bitwarden holds secret memory that blocks hibernation. Set SECURE_KEY_CONTAINER_BACKEND=mlock and restart Bitwarden.";
        let cases = [
            ("/sys/kernel/security/lockdown", "none [integrity] confidentiality\n", "kernel lockdown is blocking hibernation"),
            ("/sys/kernel/security/lockdown", "[none] integrity confidentiality\n", secret),
            ("/proc/cmdline", "quiet nohibernate resume=x\n", "the kernel was booted with nohibernate"),
            ("/proc/cmdline", "quiet nohibernated\n", secret),
            ("/proc/4242/cmdline", "/opt/bitwarden/bitwarden\0--flag\0", bitwarden),
            ("/proc/4242/cmdline", "/usr/bin/bash\0", secret),
            ("/proc/self/cmdline", "/opt/bitwarden/bitwarden\0", secret),
        ];
        for (path, content, reason) in cases {
            let s = Sandbox::new();
            s.write("/sys/power/disk", "[disabled]\n");
            s.write(path, content);
            assert_eq!(status_of(&s, T)["hibernate"], json!({"available": false, "reason": reason}), "{path}");
            assert!(!s.rust(&["status"]).calls_text().contains("CanHibernate"));
        }
        let swap = "hibernate is not available: check swap and a resume= kernel parameter";
        for (answer, ok) in [("s \"yes\"\n", true), ("s \"no\"\n", false), ("s \"challenge\"\n", false), ("s \"yes\"\ns \"no\"\n", false), ("", false)] {
            let s = Sandbox::new();
            s.write("/sys/power/disk", "[platform] shutdown reboot suspend\n");
            s.stub("busctl", "CanHibernate", 0, answer);
            assert_eq!(status_of(&s, T)["hibernate"], json!({"available": ok, "reason": if ok { "" } else { swap }}), "{answer}");
        }
    }

    #[test]
    fn test_no_hyprland_socket() {
        let s = Sandbox::new();
        s.profiles(&["power-saver"]);
        laptop(&s, "55");
        fs::remove_file(&s.sock_path).unwrap();
        let st = status_of(&s, "StatusParity.test_no_hyprland_socket");
        assert_eq!(st["quickDim"], NO_DIM());
        assert_eq!(st["travel"]["controls"]["refresh"], json!({"available": false, "detail": "No supported mode at or below 60 Hz"}));
        assert_eq!(st["travel"]["controls"]["profile"]["available"], true);
        assert!(!s.rust(&["status"]).calls_text().contains("omarchy brightness"));
    }

    #[test]
    fn test_tweaks_file_is_returned_as_parsed() {
        let cases = [("[1, 2]", json!([1, 2])), (r#"{"abm": true}"#, json!({"abm": true})), ("5", json!(5)), ("null", json!(null)),
                     (r#""x""#, json!("x")), ("{", json!({})), ("", json!({}))];
        for (raw, expected) in cases {
            let s = Sandbox::new();
            s.write(TWEAKS_FILE, raw);
            assert_eq!(status_of(&s, "StatusParity.test_tweaks_file_is_returned_as_parsed")["tweaks"], expected, "{raw}");
        }
    }

    #[test]
    fn test_status_runs_each_probe_once() {
        let s = Sandbox::new();
        s.profiles(&PROFILES[..2]);
        s.config(json!({}));
        laptop(&s, "55");
        let r = s.rust(&["status"]);
        let calls = r.calls_text();
        assert_eq!(calls.matches("omarchy brightness").count(), 1);
        assert_eq!(r.requests, ["j/monitors"]);
        assert_eq!(r.calls.iter().filter(|c| c.contains("net.hadess")).count(), 1);
        for gone in ["powerprofilesctl", "omarchy-powerprofiles-list", "hyprctl"] {
            assert!(!calls.contains(gone), "{gone}");
        }
    }
}

fn data_of(s: &Sandbox, name: &str) -> Value {
    j(&replay(s, name, &["data"], D))
}

mod data_parity {
    use super::*;

    #[test]
    fn test_discharging_energy_battery() {
        let s = Sandbox::new();
        s.battery(false, &[]);
        s.framework(true);
        s.discharging(90);
        s.ucsi(USB, true, 20.0, 3.25);
        s.config(json!({"chargeLimit": 85}));
        let d = data_of(&s, "DataParity.test_discharging_energy_battery");
        same(&d["battery"], json!({"present": true, "percent": 90, "status": "Discharging", "watts": 10.0,
                                   "health": 91, "whFull": 50.0, "whDesign": 55.0, "cycles": 123, "minutes": 240,
                                   "acOnline": 1, "fraction": 0.9, "drainTo": 85}));
        same(&d["rapl"], json!({"available": false, "package": 0, "core": 0, "gpu": 0}));
    }

    #[test]
    fn test_charging_charge_units_battery() {
        const T: &str = "DataParity.test_charging_charge_units_battery";
        let s = Sandbox::new();
        s.battery(true, &[("status", Some("Charging")), ("capacity", Some("80")), ("current_now", Some("-1000000"))]);
        s.stub("busctl", "OnBattery", 0, "b false\n");
        let d = data_of(&s, T);
        let b = &d["battery"];
        assert_eq!((&b["whFull"], &b["whDesign"], &b["watts"]), (&json!(47.9), &json!(51.3), &json!(12.0)));
        assert_eq!((&b["minutes"], &b["acOnline"], &b["drainTo"]), (&json!(39), &json!(1), &json!(0)));
        s.battery(true, &[("status", Some("Discharging")), ("current_now", None), ("current_avg", Some("500000")), ("power_now", None)]);
        s.stub("busctl", "OnBattery", 0, "b true\n");
        let d = data_of(&s, T);
        let b = &d["battery"];
        assert_eq!((&b["watts"], &b["minutes"], &b["acOnline"]), (&json!(6.0), &json!(399), &json!(0)));
    }

    #[test]
    fn test_no_battery_desktop() {
        let s = Sandbox::new();
        let r = replay(&s, "DataParity.test_no_battery_desktop", &["data"], D);
        assert_eq!(j(&r)["battery"]["present"], false);
    }

    #[test]
    fn test_garbage_in_capacity_gives_valid_json() {
        let s = Sandbox::new();
        s.battery(false, &[("capacity", Some("n/a"))]);
        let r = s.rust(&["data"]);
        assert_eq!(r.code, 0);
        let d = j(&r);
        let b = &d["battery"];
        assert_eq!((&b["present"], &b["percent"], &b["whFull"], &b["status"]), (&json!(true), &json!(0), &json!(50.0), &json!("Discharging")));
        s.battery(false, &[("energy_now", Some("x")), ("power_now", Some("")), ("voltage_now", Some("1e9")), ("cycle_count", Some("?"))]);
        let r = s.rust(&["data"]);
        assert_eq!(r.code, 0);
        assert_eq!(j(&r)["battery"]["present"], true);
    }

    #[test]
    fn test_data_has_no_tlp_and_never_asks_systemd() {
        let s = Sandbox::new();
        s.write_stub("tlp");
        s.stub("systemctl", "is-active", 0, "");
        let r = s.rust(&["data"]);
        assert!(j(&r).get("tlp").is_none(), "data still carries tlp");
        assert!(!r.calls.iter().any(|c| c.starts_with("systemctl")), "{:?}", r.calls);
    }

    #[test]
    fn test_tweaks_counts_and_pins() {
        let s = Sandbox::new();
        s.write("/sys/class/net/wlan0/address", "00\n");
        s.stub("iw", "dev wlan0 get power_save", 0, "Power save: on\n");
        for (n, mode) in ["auto", "auto", "on"].iter().enumerate() {
            s.write(&format!("/sys/bus/pci/devices/0000:00:0{n}.0/power/control"), &format!("{mode}\n"));
        }
        for (n, mode) in ["auto", "on"].iter().enumerate() {
            s.write(&format!("/sys/bus/usb/devices/1-{n}/power/control"), &format!("{mode}\n"));
        }
        s.write("/sys/module/snd_hda_intel/parameters/power_save", "1\n");
        s.write("/sys/class/drm/card1/amdgpu/panel_power_savings", "3\n");
        s.reply("getoption animations:enabled", "bool: false\nset: true\n");
        s.write(TWEAKS_FILE, r#"{"wifiPinned": true, "usbPinned": false, "audioPinned": 3, "other": 1, "wifi": "no"}"#);
        let r = s.rust(&["data"]);
        let d = data_of(&s, "DataParity.test_tweaks_counts_and_pins");
        assert_eq!(d["tweaks"], json!({"wifi": true, "usb": false, "pcie": true, "audio": true, "abm": true,
                                       "abmAvailable": true, "effects": true, "daylight": false, "daylightAvailable": false,
                                       "wifiPinned": true, "usbPinned": false, "audioPinned": 3}));
        assert_eq!(r.requests, ["getoption animations:enabled"]);
        assert!(r.calls_text().contains("iw dev wlan0 get power_save"));
    }

    #[test]
    fn test_tweaks_default_off() {
        let s = Sandbox::new();
        s.write("/sys/class/net/wlan0/address", "00\n");
        s.write("/sys/class/drm/card1/amdgpu/panel_power_savings", "0\n");
        s.write("/sys/module/snd_hda_intel/parameters/power_save", "0\n");
        assert_eq!(data_of(&s, "DataParity.test_tweaks_default_off")["tweaks"], json!({
            "wifi": false, "usb": false, "pcie": false, "audio": false, "abm": false, "abmAvailable": true, "effects": false,
            "daylight": false, "daylightAvailable": false}));
    }

    #[test]
    fn test_daylight_option_needs_a_framework_with_a_sensor() {
        let s = Sandbox::new();
        let tweaks = || j(&s.rust(&["data"]))["tweaks"].clone();
        s.write("/sys/bus/iio/devices/iio:device0/in_illuminance_raw", "37\n");
        assert_eq!(tweaks()["daylightAvailable"], false);
        s.write("/sys/class/dmi/id/sys_vendor", "Framework\n");
        assert_eq!(tweaks()["daylightAvailable"], true);
        s.write(TWEAKS_FILE, r#"{"daylight": true}"#);
        assert_eq!(tweaks()["daylight"], true);
    }

    #[test]
    fn test_tweaks_file_holding_a_list() {
        let s = Sandbox::new();
        s.write(TWEAKS_FILE, "[1, 2]");
        replay(&s, "DataParity.test_tweaks_file_holding_a_list", &["data"], D);
        assert!(!s.rust(&["data"]).stdout.contains("Pinned"));
    }

    #[test]
    fn test_sleep_log_rolling_average() {
        let s = Sandbox::new();
        let watts = [0.9, 1.1, 1.7, 2.2, 2.9, 3.4, 4.1];
        let pph = [-3.1, -2.2, -1.9, -1.4, -1.1, -0.8, -0.6];
        let mut rows: Vec<String> = watts.iter().zip(pph).enumerate().map(|(i, (w, p))| {
            format!("2026-11-0{}T10:00:00+00:00 duration={} pre_cap={} post_cap={} drop_wh=0.5 watts={w} pct_per_hour={p}", i + 1, 600 + i, 80 - i, 79 - i)
        }).collect();
        rows.insert(1, String::new());
        s.write("/var/log/battery-plus-sleep.log", &(rows.join("\n") + "\n"));
        let p = s.write("/usr/lib/systemd/system-sleep/battery-plus-sleep", "#!/bin/sh\n");
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        let d = data_of(&s, "DataParity.test_sleep_log_rolling_average");
        let sleep = &d["sleep"];
        assert_eq!((&sleep["installed"], &sleep["samples"], &sleep["lastAt"]), (&json!(true), &json!(7), &json!("2026-11-07T10:00:00+00:00")));
        assert_eq!((&sleep["lastDurationSec"], &sleep["lastPreCap"], &sleep["lastPostCap"]), (&json!(606), &json!(74), &json!(73)));
        assert!((sleep["avgWatts"].as_f64().unwrap() - 2.86).abs() < 1e-7);
        assert!((sleep["avgPctPerHour"].as_f64().unwrap() + 1.16).abs() < 1e-7);
        assert_eq!((&sleep["lastWatts"], &sleep["lastPctPerHour"]), (&json!(4.1), &json!(-0.6)));
    }

    #[test]
    fn test_sleep_log_absent_or_blank() {
        const T: &str = "DataParity.test_sleep_log_absent_or_blank";
        let s = Sandbox::new();
        assert_eq!(data_of(&s, T)["sleep"], json!({"installed": false}));
        s.write("/var/log/battery-plus-sleep.log", "\n\n");
        assert_eq!(data_of(&s, T)["sleep"], json!({"installed": false}));
    }

    #[test]
    fn test_rapl_readable() {
        const T: &str = "DataParity.test_rapl_readable";
        const RAPL: &str = "sys/class/powercap/intel-rapl:0";
        let s = Sandbox::new();
        s.write(&format!("/{RAPL}/energy_uj"), "5000000\n");
        s.write(&format!("/{RAPL}:0/energy_uj"), "100\n");
        s.write(&format!("/{RAPL}/max_energy_range_uj"), "10000000\n");
        same(&data_of(&s, T)["rapl"], json!({"available": true, "package": 0, "core": 0, "gpu": 0}));

        s.write(&format!("/{RAPL}/energy_uj"), "9000000\n");
        s.write(&format!("/{RAPL}:0/energy_uj"), "1000000\n");
        s.write(&format!("/{RAPL}:0/max_energy_range_uj"), "0\n");
        let root = s.root.clone();
        let timer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(250));
            fs::write(root.join(RAPL).join("energy_uj"), "1000000\n").unwrap();
            fs::write(root.join(format!("{RAPL}:0/energy_uj")), "3000000\n").unwrap();
        });
        let rs = replay(&s, T, &["data"], D);
        timer.join().unwrap();
        // package wrapped past max_energy_range (+2 J over 0.4 s); core rose 2 J with no wrap range
        same(&j(&rs)["rapl"], json!({"available": true, "package": 5.0, "core": 5.0, "gpu": 0}));
    }

    #[test]
    fn test_data_never_calls_lspci_or_python() {
        let s = Sandbox::new();
        let forbidden = ["lspci", "python3", "hyprctl", "jq", "awk"];
        for tool in forbidden {
            s.write_stub(tool);
        }
        s.battery(false, &[]);
        let r = s.rust(&["data"]);
        assert_eq!(r.code, 0);
        for tool in forbidden {
            assert!(!r.calls_text().contains(tool), "{tool}");
        }
        assert!(!r.calls.is_empty());
        assert!(r.calls.iter().all(|c| ["busctl", "iw", "systemctl"].contains(&c.split(' ').next().unwrap())), "{:?}", r.calls);
    }
}

const IDLE_FILE: &str = "run/omarchy-power-hub/idle-wake.json";
const STATE_FILE: &str = "run/omarchy-power-hub/display-off.json";

fn seed_cursor(s: &Sandbox, cursor: &str, age: u64) {
    let path = s.write(IDLE_FILE, &format!("{}\n", json!({"cursor": cursor})));
    let file = fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
}

/// Replays `idle-wake-poll` from a seeded cursor file; returns the result and the file it left.
fn poll(s: &Sandbox, name: &str, args: &[&str], prev: Option<&str>, age: u64) -> (Outcome, String) {
    let _ = fs::remove_file(s.target(IDLE_FILE));
    if let Some(prev) = prev {
        seed_cursor(s, prev, age);
    }
    let r = replay(s, name, &[&["idle-wake-poll"], args].concat(), D);
    let file = fs::read_to_string(s.target(IDLE_FILE)).unwrap();
    assert_eq!(file, format!("{{\"cursor\": {}}}\n", parsed(&file)["cursor"]), "goldens hold the parsed file, not its bytes");
    (r, file)
}

fn poll_says(s: &Sandbox, name: &str, args: &[&str], prev: &str) -> String {
    text(&poll(s, name, args, Some(prev), 0).0).to_string()
}

fn edp() -> Value {
    json!({"name": "eDP-1", "dpmsStatus": true})
}

fn device_eval(name: &str, enabled: &str) -> String {
    format!("hl.device({{ name = \"{name}\", enabled = {enabled} }})")
}

fn device_evals(names: &[&str], enabled: &str) -> Vec<String> {
    names.iter().map(|n| device_eval(n, enabled)).collect()
}

mod idle_parity {
    use super::*;

    const CURSOR: &str = "{\"cursor\": \"500, 300\"}\n";

    #[test]
    fn test_cursor_moved_wakes() {
        const T: &str = "IdleParity.test_cursor_moved_wakes";
        let s = Sandbox::new();
        s.monitors(&[json!({"name": "eDP-1", "dpmsStatus": false})]);
        s.devices(&[json!("m")], &[json!("kb")]);
        s.reply("cursorpos", "500, 300\n");
        let (r, f) = poll(&s, T, &["blanked"], Some("10, 10"), 0);
        assert_eq!(text(&r), "wake");
        assert_eq!(r.evals, device_evals(&["m", "kb"], "true"));
        assert_eq!(f, CURSOR);
        s.reply("cursorpos", "10, 10\n");
        assert_eq!(poll_says(&s, T, &["blanked"], "10, 10"), "hold");
    }

    #[test]
    fn test_partly_lit_displays_ask_to_blank_again() {
        const T: &str = "IdleParity.test_partly_lit_displays_ask_to_blank_again";
        let s = Sandbox::new();
        s.monitors(&[json!({"name": "eDP-1", "dpmsStatus": false}), json!({"name": "DP-1", "dpmsStatus": true})]);
        s.reply("cursorpos", "10, 10\n");
        assert_eq!(poll_says(&s, T, &["blanked"], "10, 10"), "blank");
        assert_eq!(poll_says(&s, T, &[], "10, 10"), "hold");
    }

    #[test]
    fn test_stale_cursor_file_does_not_wake() {
        const T: &str = "IdleParity.test_stale_cursor_file_does_not_wake";
        let s = Sandbox::new();
        s.monitors(&[json!({"name": "eDP-1", "dpmsStatus": false})]);
        s.reply("cursorpos", "500, 300\n");
        assert_eq!(text(&poll(&s, T, &["blanked"], Some("10, 10"), 1).0), "wake");
        let (r, f) = poll(&s, T, &["blanked"], Some("10, 10"), 10);
        assert_eq!(text(&r), "hold");
        assert_eq!(f, CURSOR);
        assert_eq!(text(&poll(&s, T, &["blanked"], None, 0).0), "hold");
    }

    #[test]
    fn test_lid_closed_disables_pointers_and_keyboards() {
        const T: &str = "IdleParity.test_lid_closed_disables_pointers_and_keyboards";
        let names = ["m1", "m2", "kb"];
        let s = Sandbox::new();
        s.lid("closed");
        s.devices(&[json!("m1"), json!("m2")], &[json!("kb")]);
        let (r, _) = poll(&s, T, &["blanked"], None, 0);
        assert_eq!(text(&r), "closed");
        assert_eq!(r.evals, device_evals(&names, "false"));
        // Not blanked: the pointers stay enabled even with the lid shut.
        let (r, _) = poll(&s, T, &[], None, 0);
        assert_eq!((text(&r), &r.evals), ("closed", &device_evals(&names, "true")));
        s.lid("open");
        let (r, _) = poll(&s, T, &["off"], Some("1, 1"), 0);
        assert_eq!(r.evals, device_evals(&names, "true"));
    }

    #[test]
    fn test_blanked_but_monitors_awake_wakes() {
        const T: &str = "IdleParity.test_blanked_but_monitors_awake_wakes";
        let s = Sandbox::new();
        s.reply("cursorpos", "5, 5\n");
        s.monitors(&[edp(), json!({"name": "DP-1", "disabled": true, "dpmsStatus": false})]);
        assert_eq!(poll_says(&s, T, &["blanked"], "5, 5"), "wake");
        assert_eq!(poll_says(&s, T, &[], "5, 5"), "hold");
        s.monitors(&[edp(), json!({"name": "DP-2", "dpmsStatus": false})]);
        assert_eq!(poll_says(&s, T, &["blanked"], "5, 5"), "blank");
        s.monitors(&[]);
        assert_eq!(poll_says(&s, T, &["blanked"], "5, 5"), "hold");
    }

    #[test]
    fn test_socket_gone_still_answers_from_lid() {
        let s = Sandbox::without_hypr();
        s.lid("closed");
        let r = s.rust(&["idle-wake-poll", "blanked"]);
        assert_eq!((r.code, text(&r)), (0, "closed"));
    }

    #[test]
    fn test_unsafe_device_name_never_reaches_eval() {
        let s = Sandbox::new();
        s.devices(&[json!("ok-mouse"), json!("bad\"name")], &[json!("ok-kb")]);
        replay(&s, "IdleParity.test_unsafe_device_name_never_reaches_eval", &["idle-wake-poll"], D);
        assert!(!s.rust(&["idle-wake-poll"]).evals.iter().any(|e| e.contains("bad")));
        assert_eq!(s.rust(&["idle-wake-poll"]).evals, device_evals(&["ok-mouse", "ok-kb"], "true"));
    }

    #[test]
    fn test_power_button_keyboard_skipped() {
        let s = Sandbox::new();
        s.devices(&[json!("power-button"), json!("video-bus")],
                  &[json!("power-button"), json!("video-bus"), json!("x-gpio-keys"), json!("my-power-button"), json!("AT:kb.0_1-x")]);
        let (r, _) = poll(&s, "IdleParity.test_power_button_keyboard_skipped", &["blanked"], None, 0);
        assert_eq!(r.evals, device_evals(&["power-button", "video-bus", "my-power-button", "AT:kb.0_1-x"], "true"));
    }

    #[test]
    fn test_non_string_device_names_follow_python_str() {
        let s = Sandbox::new();
        s.devices(&[json!(5), json!(true), json!(false), json!(null), json!(""), json!(1.5), json!(["a"]), json!({"name": "x"}), json!("nl\n"), json!("sp ace")],
                  &[json!(0), json!(7)]);
        let (r, _) = poll(&s, "IdleParity.test_non_string_device_names_follow_python_str", &[], None, 0);
        assert_eq!(r.evals, device_evals(&["5", "True", "1.5", "x", "nl\n", "7"], "true"));
    }

    #[test]
    fn test_no_forks() {
        let s = Sandbox::new();
        assert!(s.rust(&["idle-wake-poll", "blanked"]).calls.is_empty());
    }
}

fn park(s: &Sandbox, specs: &[Value]) {
    s.write(STATE_FILE, &format!("{}\n", json!({"disabled": specs})));
}

fn sync(s: &Sandbox, name: &str, specs: &[Value]) -> (Outcome, String) {
    park(s, specs);
    let r = replay(s, name, &["displays", "sync"], D);
    (r, fs::read_to_string(s.target(STATE_FILE)).unwrap())
}

mod sync_parity {
    use super::*;

    #[test]
    fn test_idle_when_nothing_parked() {
        const T: &str = "SyncParity.test_idle_when_nothing_parked";
        let s = Sandbox::new();
        s.monitors(&[edp()]);
        let (r, _) = sync(&s, T, &[]);
        assert_eq!((text(&r), r.evals.len()), ("idle", 0));
        fs::remove_file(s.target(STATE_FILE)).unwrap();
        replay(&s, T, &["displays", "sync"], D);
        assert_eq!(text(&s.rust(&["displays", "sync"])), "idle");
    }

    #[test]
    fn test_waiting_while_a_monitor_sleeps() {
        let s = Sandbox::new();
        let spec = json!({"name": "DP-1", "width": 1920, "height": 1080});
        let waiting = format!("{}\n", json!({"disabled": [spec]})); // what park wrote: the file is left alone
        for mons in [vec![edp(), json!({"name": "DP-2", "dpmsStatus": false})], vec![]] {
            s.monitors(&mons);
            let (r, f) = sync(&s, "SyncParity.test_waiting_while_a_monitor_sleeps", std::slice::from_ref(&spec));
            assert_eq!((text(&r), r.evals.len(), f.as_str()), ("waiting", 0, waiting.as_str()));
        }
    }

    #[test]
    fn test_restored_sends_same_lua_and_clears_state() {
        let s = Sandbox::new();
        s.monitors(&[edp(), json!({"name": "DP-9", "disabled": true})]);
        let specs = [
            json!({"name": "DP-1", "output": "desc:BOE NE160 (1)+x", "width": 2560, "height": 1600, "refreshRate": 59.951, "x": 0, "y": 0, "scale": 1.25}),
            json!({"name": "DP-2", "width": 1920, "height": 1080, "refreshRate": 60.0, "x": -1920, "y": 0, "scale": 1}),
            json!({"name": "DP-3", "width": 3840, "height": 2160, "refreshRate": 144, "x": 2560, "y": 100, "scale": 2.0}),
            json!({"name": "DP-4", "width": 800, "height": 600}),
            json!({"name": "DP-5", "width": 0, "height": 600, "refreshRate": 75}),
            json!({"name": "DP-6", "width": 100, "height": 100, "refreshRate": 1e16, "scale": 0.00001}),
            json!({"name": "DP-7", "width": 100, "height": 100, "refreshRate": 0.1, "scale": 1.5e300}),
            json!({"name": "bad name"}), json!({"name": "DP-8", "output": "desc:a;b"}), json!({"name": ""}), json!({"name": 0}), json!({"output": "x"}),
            json!("HDMI-A-1"), json!(12), json!(null),
        ];
        let (r, f) = sync(&s, "SyncParity.test_restored_sends_same_lua_and_clears_state", &specs);
        assert_eq!((text(&r), parsed(&f)), ("restored", json!({"disabled": []})));
        for lua in [
            r#"hl.monitor({ output = "desc:BOE NE160 (1)+x", mode = "2560x1600@59.951", position = "0x0", scale = 1.25, disabled = false })"#,
            r#"hl.monitor({ output = "DP-2", mode = "1920x1080@60.0", position = "-1920x0", scale = 1, disabled = false })"#,
            r#"hl.monitor({ output = "DP-3", mode = "3840x2160@144", position = "2560x100", scale = 2.0, disabled = false })"#,
            r#"hl.monitor({ output = "DP-4", mode = "800x600@60", position = "0x0", scale = 1, disabled = false })"#,
            r#"hl.monitor({ output = "DP-5", disabled = false })"#,
        ] {
            assert!(r.evals.iter().any(|e| e == lua), "{lua}");
        }
        let joined = r.evals.concat();
        for part in ["@1e+16\"", "scale = 1e-05", "scale = 1.5e+300"] {
            assert!(joined.contains(part), "{part}");
        }
    }

    #[test]
    fn test_unopenable_lock_fails_closed() {
        let s = Sandbox::new();
        s.monitors(&[edp()]);
        park(&s, &[json!({"name": "DP-1"})]);
        let lock = s.target("run/omarchy-power-hub/display-off.lock");
        let _ = fs::remove_file(&lock);
        fs::create_dir(&lock).unwrap();
        let r = replay(&s, "SyncParity.test_unopenable_lock_fails_closed", &["displays", "sync"], D);
        fs::remove_dir(&lock).unwrap();
        assert_eq!((r.code, r.stdout.as_str(), r.evals.len(), s.read_state()), (1, "", 0, json!({"disabled": [{"name": "DP-1"}]})));
    }

    #[test]
    fn test_waits_for_python_holding_the_lock() {
        let s = Sandbox::new();
        s.monitors(&[edp()]);
        park(&s, &[json!({"name": "DP-1"})]);
        let held = s.hold_flock("run/omarchy-power-hub/display-off.lock");
        let mut p = s.rust_start(&["displays", "sync"]);
        thread::sleep(Duration::from_millis(200));
        assert!(p.try_wait().unwrap().is_none());
        assert_eq!(s.read_state(), json!({"disabled": [{"name": "DP-1"}]}));
        drop(held);
        let out = p.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "restored");
        assert_eq!(s.read_state(), json!({"disabled": []}));
        assert_eq!(s.hypr().evals, [r#"hl.monitor({ output = "DP-1", disabled = false })"#]);
    }
}

mod harness_tests {
    use super::*;

    fn fails(expected: &str, differ: impl FnOnce()) {
        let panic = catch_unwind(AssertUnwindSafe(differ)).expect_err("the harness missed a change");
        let message = panic.downcast_ref::<String>().map(String::as_str).or(panic.downcast_ref::<&str>().copied()).unwrap_or_default();
        assert!(message.contains(expected), "failed for another reason: {message}");
    }

    #[test]
    fn test_harness_sees_a_changed_file() {
        let (a, b) = (Sandbox::new(), Sandbox::new());
        a.write(TWEAKS_FILE, "{}");
        fails("files differ", || assert_same_files(&a, &b, None));
    }

    #[test]
    fn test_harness_ignores_locks_temp_files_and_the_base_path() {
        let (a, b) = (Sandbox::new(), Sandbox::new());
        for s in [&a, &b] {
            s.write("run/omarchy-power-hub/energy-controls.lock", "");
            s.write("run/x.tmp", &s.base.display().to_string());
            s.write("home/y.json", &json!({"p": s.base.join("z")}).to_string());
        }
        assert_same_files(&a, &b, None);
    }

    #[test]
    fn test_harness_json_key_order_and_types_matter() {
        for (left, right) in [(r#"{"z":1,"a":2}"#, r#"{"a":2,"z":1}"#), (r#"{"a":true}"#, r#"{"a":1}"#), (r#"{"a":1}"#, r#"{"a":1.0}"#)] {
            let (a, b) = (Sandbox::new(), Sandbox::new());
            a.write("home/s.json", left);
            b.write("home/s.json", right);
            fails("files differ", || assert_same_files(&a, &b, None));
        }
        let (a, b) = (Sandbox::new(), Sandbox::new());
        a.write("home/s.json", r#"{"a": 1}"#);
        b.write("home/s.json", "{\"a\":1}\n");
        assert_same_files(&a, &b, None);
    }

    #[test]
    fn test_harness_sees_a_symlink_target() {
        let (a, b) = (Sandbox::new(), Sandbox::new());
        symlink("x", a.home.join("l")).unwrap();
        symlink("y", b.home.join("l")).unwrap();
        fails("files differ", || assert_same_files(&a, &b, None));
    }

    #[test]
    fn test_harness_normalise_hook_fixes_file_content() {
        let (a, b) = (Sandbox::new(), Sandbox::new());
        a.write("home/u.service", "ExecStart=python\n");
        b.write("home/u.service", "ExecStart=rust\n");
        fails("files differ", || assert_same_files(&a, &b, None));
        assert_same_files(&a, &b, Some(|_, text| text.split('=').next().unwrap().to_string()));
    }

    #[test]
    fn test_harness_both_runs_a_verb_in_each_sandbox() {
        const T: &str = "HarnessTests.test_harness_both_runs_a_verb_in_each_sandbox";
        let s = Sandbox::new();
        s.config(json!({}));
        let opts = Replay { stdout: false, ..Replay::default() };
        replay(&s, T, &["config"], opts);
        replay(&s, T, &["ac"], opts);
    }

    #[test]
    fn test_harness_both_fails_on_different_stdout() {
        let s = Sandbox::new();
        s.config(json!({}));
        fails("stdout differs", || { replay(&s, "HarnessTests.test_harness_both_fails_on_different_stdout", &["config"], Replay::default()); });
    }

    #[test]
    fn test_harness_stateful_stubs_remember_the_last_set() {
        let s = Sandbox::new();
        s.stateful_profile("balanced");
        s.stateful_brightness(70);
        s.stub("sudo", "", 0, "");
        let stub = |tool: &str, args: &[&str]| run(Command::new(s.stubs.join(tool)).args(args).env_clear().envs(s.env()));
        assert!(stub("powerprofilesctl", &["set", "power-saver"]).status.success());
        assert!(stub("omarchy", &["brightness", "display", "--no-osd", "--monitor", "eDP-1", "40%"]).status.success());
        let get = |tool: &str, args: &[&str]| String::from_utf8_lossy(&stub(tool, args).stdout).trim().to_string();
        assert_eq!((get("powerprofilesctl", &["get"]), get("omarchy", &["brightness", "display", "--monitor", "eDP-1"])), ("power-saver".into(), "40".into()));
    }

    #[test]
    fn test_harness_hypr_on_eval_runs_before_the_reply() {
        let s = Sandbox::new();
        s.monitors(&[json!({"name": "eDP-1"})]);
        s.hypr().on_eval = Some(Box::new(|hypr, lua| hypr.monitors(&[json!({"name": lua})])));
        assert!(run(Command::new(s.stubs.join("hyprctl")).args(["eval", "X"]).env_clear().envs(s.env())).status.success());
        assert_eq!(parsed(&s.hypr().replies["j/monitors"]), json!([{"name": "X"}]));
    }

    #[test]
    fn test_rust_waits_for_the_energy_lock_python_holds() {
        for argv in [&["quick-dim", "off"][..], &["tweak", "turbo", "on"], &["cpu", "turbo", "on"]] {
            let s = Sandbox::new();
            s.config(json!({}));
            let held = s.hold_flock("run/omarchy-power-hub/energy-controls.lock");
            let mut p = s.rust_start(argv);
            thread::sleep(Duration::from_millis(200));
            assert!(p.try_wait().unwrap().is_none(), "{argv:?}");
            drop(held);
            p.wait_with_output().unwrap();
        }
    }

    /// A real recorded step and the unchanged run it passes on, so a failure below comes from the one change.
    fn recorded() -> (Sandbox, Value, Outcome) {
        let s = Sandbox::new();
        s.lid("closed");
        s.devices(&[json!("m1"), json!("m2")], &[json!("kb")]);
        let got = s.rust(&["idle-wake-poll", "blanked"]);
        let step = golden_step("IdleParity.test_lid_closed_disables_pointers_and_keyboards", 0);
        verify(&s, &step, &got, D).unwrap();
        (s, step, got)
    }

    #[test]
    fn replay_fails_on_a_changed_file() {
        let (s, step, got) = recorded();
        s.write(TWEAKS_FILE, "{}");
        assert!(verify(&s, &step, &got, D).unwrap_err().contains("files differ"));
    }

    #[test]
    fn replay_fails_on_a_changed_exit_code() {
        let (s, step, mut got) = recorded();
        got.code = 1;
        assert!(verify(&s, &step, &got, D).unwrap_err().contains("exit code differs"));
    }

    #[test]
    fn replay_fails_on_a_changed_eval_order() {
        let (s, step, mut got) = recorded();
        got.evals.reverse();
        assert!(verify(&s, &step, &got, D).unwrap_err().contains("hyprland evals differ"));
    }

    #[test]
    fn replay_fails_on_changed_calls() {
        let (s, step, mut got) = recorded();
        verify(&s, &step, &got, Replay::default()).unwrap();
        got.calls.push("systemctl suspend".into());
        assert!(verify(&s, &step, &got, Replay::default()).unwrap_err().contains("calls differ"));
    }

    #[test]
    fn replay_fails_on_changed_requests() {
        let (s, step, mut got) = recorded();
        let opts = Replay { requests: true, ..D };
        verify(&s, &step, &got, opts).unwrap();
        got.requests.reverse();
        assert!(verify(&s, &step, &got, opts).unwrap_err().contains("requests differ"));
    }

    #[test]
    fn replay_fails_on_changed_side_state() {
        let (s, step, got) = recorded();
        s.write("/sys/class/leds/chromeos:white:power/brightness", "0\n");
        assert!(verify(&s, &step, &got, D).unwrap_err().contains("side files differ"));
    }

    #[test]
    fn replay_names_a_golden_that_stopped_short() {
        let steps = Arc::new(vec![json!({}), json!({})]);
        let cursors = Cursors::from([("A.test_done".to_string(), (steps.clone(), 2)), ("B.test_short".to_string(), (steps, 1))]);
        assert_eq!(stopped_short(&cursors), ["B.test_short stopped at step 1 of 2"]);
    }

    #[test]
    #[should_panic(expected = "no golden for HarnessTests.test_never_recorded")]
    fn replay_fails_when_a_golden_is_missing() {
        replay(&Sandbox::new(), "HarnessTests.test_never_recorded", &["gpu"], D);
    }
}
