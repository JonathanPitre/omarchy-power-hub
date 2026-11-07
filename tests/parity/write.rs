//! The verbs that change settings, ported from `tests/test_parity.py` class by class under the same test names.
use crate::read::{CONFIG_FILE, PROFILES, TRAVEL_FILE, TWEAKS_FILE, frozen, j, merged, parsed, same, text, travel_snapshot};
use crate::support::{Outcome, RUST_BIN, Replay, Sandbox, replay, run};
use serde_json::{Value, json};
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

/// What `assert_same` compares, plus files.
pub(crate) const D: Replay = Replay::direct();
/// What `both()` compares.
pub(crate) const BOTH: Replay = Replay { calls: true, ..Replay::direct() };
/// An uncaught error ends the verb with exit 1 and the message on stdout; the recorded run printed a traceback on stderr, so only the exit code is compared.
const UNCAUGHT: Replay = Replay { stdout: false, ..BOTH };

/// The old failed apply left bindings.lua as it was; the binary rewrites the lid block so it never names a gone backend.
fn without_bindings(rel: &str, text: &str) -> String {
    if rel.ends_with("hypr/bindings.lua") { String::new() } else { text.to_string() }
}

pub(crate) const MASKED: Replay = Replay { normalise: Some(mask_exec), ..BOTH };

const SHELL_FILE: &str = "home/.config/omarchy/shell.json";
const BINDINGS_FILE: &str = "home/.config/hypr/bindings.lua";
const HIBERNATE_DROPIN: &str = "/etc/systemd/sleep.conf.d/90-omarchy-power-hub.conf";
const SHELL_IDLE: &str = r#"{"idle": {"screensaver": 150, "lock": 300}, "other": 1}"#;
const UNIT_FILE: &str = "home/.config/systemd/user/omarchy-power-hub-tweaks.service";
const BATTERY: &str = "/org/freedesktop/UPower/devices/battery_BAT0";
pub(crate) const OLDER: &str = "Travel restore point is from an older version; reboot to clear it";

/// A fake system a test starts from; `Fn(&Sandbox)` closures are setups too.
pub(crate) trait Setup {
    fn setup(&self, s: &Sandbox);
}

impl<F: Fn(&Sandbox)> Setup for F {
    fn setup(&self, s: &Sandbox) {
        self(s);
    }
}

/// `re.sub(r"ExecStart=.*", "ExecStart=<masked>", text)`.
pub(crate) fn mask_exec(_path: &str, text: &str) -> String {
    text.split_inclusive('\n').map(|line| match line.find("ExecStart=") {
        Some(at) => format!("{}ExecStart=<masked>{}", &line[..at], if line.ends_with('\n') { "\n" } else { "" }),
        None => line.to_string(),
    }).collect()
}

/// The binary must match the recorded run on every step: replays each step in a fresh sandbox, calls `after` with the sandbox and
/// the result of each, and returns the sandbox with the last result.
pub(crate) fn checked_each(name: &str, setup: &impl Setup, steps: &[&[&str]], opts: Replay, after: &mut dyn FnMut(&Sandbox, &Outcome)) -> (Sandbox, Outcome) {
    let s = Sandbox::new();
    setup.setup(&s);
    let mut last = None;
    for step in steps {
        let r = replay(&s, name, step, opts);
        after(&s, &r);
        last = Some(r);
    }
    (s, last.unwrap())
}

pub(crate) fn checked(name: &str, setup: &impl Setup, steps: &[&[&str]], opts: Replay) -> (Sandbox, Outcome) {
    checked_each(name, setup, steps, opts, &mut |_, _| {})
}

pub(crate) fn tweak_both(name: &str, setup: &impl Setup, steps: &[&[&str]]) -> (Sandbox, Outcome) {
    checked(name, setup, steps, MASKED)
}

/// Whether any call of the run contains `needle`.
pub(crate) fn called(r: &Outcome, needle: &str) -> bool {
    r.calls.iter().any(|c| c.contains(needle))
}

/// Calls that start with `prefix`.
pub(crate) fn starting(r: &Outcome, prefix: &str) -> Vec<String> {
    r.calls.iter().filter(|c| c.starts_with(prefix)).cloned().collect()
}

/// Whether `r.calls` holds exactly this call.
pub(crate) fn has_call(r: &Outcome, call: &str) -> bool {
    r.calls.iter().any(|c| c == call)
}

pub(crate) fn read(s: &Sandbox, path: &str) -> String {
    fs::read_to_string(s.target(path)).unwrap()
}

/// Restores a directory's mode when dropped, so a failed test leaves nothing undeletable.
pub(crate) struct Restore(pub PathBuf);

impl Drop for Restore {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.0, Permissions::from_mode(0o755));
    }
}

pub(crate) fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
}

/// Snapshots skip *.tmp, so a temp file left behind by a failed write is only visible here.
pub(crate) fn no_leftovers(s: &Sandbox) {
    fn walk(dir: &Path, left: &mut Vec<String>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if entry.file_type().unwrap().is_dir() {
                walk(&p, left);
            } else if p.extension().is_some_and(|e| e == "tmp") {
                left.push(p.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut left = vec![];
    walk(&s.run, &mut left);
    assert!(left.is_empty(), "temp files left behind: {left:?}");
}

pub(crate) fn root_user() -> bool {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() == 0 }
}

pub(crate) struct Settings {
    pub shell: Option<String>,
    pub bindings: Option<String>,
    pub battery: bool,
    pub keys: Value,
}

impl Default for Settings {
    fn default() -> Self {
        Self { shell: Some(SHELL_IDLE.into()), bindings: None, battery: false, keys: json!({}) }
    }
}

impl Setup for Settings {
    fn setup(&self, s: &Sandbox) {
        s.config(self.keys.clone());
        s.profiles(PROFILES);
        s.write("home/.config/hypr/.keep", "");
        if let Some(shell) = &self.shell {
            s.write(SHELL_FILE, shell);
        }
        if let Some(bindings) = &self.bindings {
            s.write(BINDINGS_FILE, bindings);
        }
        if self.battery {
            s.stub("busctl", "OnBattery", 0, "b true\n");
        }
    }
}

fn shell_of(s: &Sandbox) -> Value {
    parsed(&read(s, SHELL_FILE))
}

mod settings_parity {
    use super::*;

    #[test]
    fn test_active_side_rewrites_only_idle() {
        let (s, _) = checked("SettingsParity.test_active_side_rewrites_only_idle", &Settings { battery: true, ..Settings::default() }, &[&["set", "battery", "lock", "600"]], BOTH);
        same(&shell_of(&s), json!({"idle": {"screensaver": 150, "lock": 600}, "other": 1}));
    }

    #[test]
    fn test_inactive_side_changes_the_config_only() {
        let (s, _) = checked("SettingsParity.test_inactive_side_changes_the_config_only", &Settings::default(), &[&["set", "battery", "lock", "600"]], BOTH);
        assert_eq!(read(&s, SHELL_FILE), SHELL_IDLE);
        assert_eq!(s.read_config()["battery"]["lock"], 600);
    }

    #[test]
    fn test_matching_shell_json_is_not_written() {
        let shell = r#"{"idle":{"screensaver":150.0,"lock":300}}"#;
        let (s, _) = checked("SettingsParity.test_matching_shell_json_is_not_written", &Settings { shell: Some(shell.into()), ..Settings::default() }, &[&["set", "ac", "lock", "300"]], BOTH);
        assert_eq!(read(&s, SHELL_FILE), shell);
    }

    #[test]
    fn test_saver_hidden_by_displays_off_maps_to_a_week() {
        let setup = Settings { keys: json!({"ac": {"profile": "balanced", "displaysOff": 200}}), ..Settings::default() };
        let (s, _) = checked("SettingsParity.test_saver_hidden_by_displays_off_maps_to_a_week", &setup, &[&["set", "ac", "screensaver", "300"]], BOTH);
        same(&shell_of(&s)["idle"]["screensaver"], json!(7 * 24 * 3600));
    }

    #[test]
    fn test_negative_values_clamp_to_zero() {
        let (s, _) = checked("SettingsParity.test_negative_values_clamp_to_zero", &Settings::default(), &[&["set", "ac", "sleep", "-5"]], BOTH);
        assert_eq!(s.read_config()["ac"]["sleep"], 0);
    }

    /// The old `test_idle_set.py` and `test_profiles.py` cases: its battery on, `WEEK` already in the shell file.
    fn idle_setup() -> Settings {
        let week = 7 * 24 * 3600;
        let side = |saver, displays_off, lock, sleep, hibernate, profile| json!({"screensaver": saver, "displaysOff": displays_off, "lock": lock,
            "sleep": sleep, "hibernateAfter": hibernate, "lid": "nothing", "profile": profile});
        Settings {
            shell: Some(json!({"idle": {"screensaver": week, "lock": 60}}).to_string()),
            battery: true,
            keys: json!({"ac": side(60, 300, 300, 0, 0, "performance"), "battery": side(0, 60, 60, 300, 60, "balanced")}),
            ..Settings::default()
        }
    }

    /// The recorded run stubbed which side is active; the binary asks UPower, and that read is its only call.
    const ON_BATTERY_READ: &str = "busctl get-property org.freedesktop.UPower /org/freedesktop/UPower org.freedesktop.UPower OnBattery";

    #[test]
    fn test_timer_clicks_on_the_active_side_touch_nothing_else() {
        let s = Sandbox::new();
        idle_setup().setup(&s);
        let before = read(&s, SHELL_FILE);
        for (key, value) in [("displaysOff", "300"), ("sleep", "1800")] {
            let r = s.rust(&["set", "battery", key, value]);
            assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"), "{key}");
            assert_eq!(r.calls, [ON_BATTERY_READ], "{key}");
        }
        assert_eq!(read(&s, SHELL_FILE), before);
        assert_eq!((s.read_config()["battery"]["displaysOff"].clone(), s.read_config()["battery"]["sleep"].clone()), (json!(300), json!(1800)));
    }

    #[test]
    fn test_clicks_on_the_inactive_side_apply_nothing() {
        let s = Sandbox::new();
        idle_setup().setup(&s);
        let before = read(&s, SHELL_FILE);
        for key in ["lock", "hibernateAfter"] {
            let r = s.rust(&["set", "ac", key, "600"]);
            assert_eq!(r.code, 0, "{key}");
            assert_eq!(r.calls, [ON_BATTERY_READ], "{key}");
        }
        assert_eq!(read(&s, SHELL_FILE), before);
    }

    #[test]
    fn test_a_saver_the_displays_would_pre_empt_never_starts_and_returns_when_they_move() {
        let s = Sandbox::new();
        idle_setup().setup(&s);
        let saver = |s: &Sandbox| parsed(&read(s, SHELL_FILE))["idle"]["screensaver"].clone();
        for value in ["60", "300"] {
            s.rust(&["set", "battery", "screensaver", value]);
            assert_eq!(saver(&s), json!(7 * 24 * 3600), "{value}");
        }
        assert_eq!(j(&s.rust(&["config"]))["config"]["battery"]["screensaver"], 300);
        s.rust(&["set", "battery", "displaysOff", "600"]);
        assert_eq!(saver(&s), json!(300));
        s.rust(&["set", "battery", "displaysOff", "0"]);
        assert_eq!(saver(&s), json!(300));
    }

    #[test]
    fn test_system_default_is_no_longer_a_lid_action() {
        let s = Sandbox::new();
        idle_setup().setup(&s);
        let r = s.rust(&["set", "ac", "lid", "system-default"]);
        assert_eq!((r.code, text(&r)), (1, "bad lid action"));
    }

    #[test]
    fn test_legacy_profile_names_map_to_real_profiles() {
        // (ac, battery) -> (ac, battery)
        let cases = [(("auto", "saver"), ("balanced", "power-saver")), (("full", "balanced"), ("performance", "balanced")),
                     (("power-saver", "performance"), ("power-saver", "performance")), (("auto", "auto"), ("balanced", "balanced"))];
        for ((ac, battery), want) in cases {
            let s = Sandbox::new();
            s.profiles(PROFILES);
            s.config(json!({"ac": {"profile": ac}, "battery": {"profile": battery}}));
            let config = &j(&s.rust(&["config"]))["config"];
            assert_eq!((config["ac"]["profile"].as_str().unwrap(), config["battery"]["profile"].as_str().unwrap()), want, "{ac} {battery}");
        }
        let s = Sandbox::new();
        s.profiles(PROFILES);
        s.config(json!({"ac": {"screensaver": 10}, "battery": {"profile": "auto"}}));
        let config = &j(&s.rust(&["config"]))["config"];
        assert_eq!((&config["ac"]["profile"], &config["battery"]["profile"]), (&json!("performance"), &json!("balanced")));
    }

    #[test]
    fn test_lid_writes_one_block_and_reloads_once() {
        let step: &[&str] = &["set", "ac", "lid", "display-off"];
        let setup = Settings { bindings: Some("bind = x".into()), ..Settings::default() };
        let (s, last) = checked("SettingsParity.test_lid_writes_one_block_and_reloads_once", &setup, &[step, step], BOTH);
        let bindings = read(&s, BINDINGS_FILE);
        assert_eq!((bindings.matches("-- BEGIN").count(), bindings.matches("-- END").count()), (1, 1));
        assert!(bindings.contains(&format!("{}/power-hub lid-closed", s.bin.display())));
        assert!(bindings.contains(&format!("{}/power-hub lid-opened", s.bin.display())));
        assert!(!bindings.contains("python3"));
        assert!(last.requests.is_empty());
        let fresh = Sandbox::new();
        setup.setup(&fresh);
        assert_eq!(fresh.rust(step).requests, ["reload"]);
    }

    #[test]
    fn test_lid_block_cases() {
        const STALE: &str = "a = 1\n-- BEGIN Omarchy Power Hub lid action override\nold\n-- END Omarchy Power Hub lid action override\nb = 2\n";
        let cases = [None, Some(String::new()), Some("a = 1".to_string()), Some(STALE.to_string()), Some(STALE.repeat(2)),
                     Some("a = 1\n-- BEGIN Omarchy Power Hub lid action override\nold\n".to_string())];
        for bindings in cases {
            for action in ["display-off", "nothing"] {
                let setup = Settings { bindings: bindings.clone(), ..Settings::default() };
                checked("SettingsParity.test_lid_block_cases", &setup, &[&["set", "ac", "lid", action]], BOTH);
            }
        }
    }

    #[test]
    fn test_matching_hibernate_delay_needs_no_sudo() {
        let setup = |s: &Sandbox| {
            Settings::default().setup(s);
            s.write(HIBERNATE_DROPIN, "[Sleep]\nHibernateDelaySec=1800\n");
        };
        let (_, last) = checked("SettingsParity.test_matching_hibernate_delay_needs_no_sudo", &setup, &[&["set", "ac", "hibernateAfter", "1800"]], BOTH);
        assert!(starting(&last, "sudo").is_empty());
    }

    #[test]
    fn test_hibernate_delay_is_pushed_through_the_helper() {
        let setup = |s: &Sandbox| {
            Settings::default().setup(s);
            s.install_helper(false);
            s.stub("busctl", "CanHibernate", 0, "s \"yes\"\n");
        };
        let (_, last) = checked("SettingsParity.test_hibernate_delay_is_pushed_through_the_helper", &setup, &[&["set", "ac", "hibernateAfter", "900"]], BOTH);
        assert!(called(&last, "hibernate-delay 900"));
    }

    #[test]
    fn test_unavailable_hibernation_gives_the_reason() {
        let setup = |s: &Sandbox| {
            Settings::default().setup(s);
            s.write("/sys/power/disk", "[disabled]\n");
        };
        let (_, last) = checked("SettingsParity.test_unavailable_hibernation_gives_the_reason", &setup, &[&["set", "ac", "hibernateAfter", "1800"]], BOTH);
        assert_eq!((last.code, last.stdout.contains("secret memory")), (1, true));
    }

    #[test]
    fn test_bad_arguments_give_the_error_strings() {
        let steps: [&[&str]; 4] = [&["set", "ac", "nope", "1"], &["set", "x", "lock", "1"], &["set", "ac", "lid", "bad"], &["set", "ac", "profile", "bad"]];
        let s = Sandbox::new();
        Settings::default().setup(&s);
        let said: Vec<String> = steps.iter().map(|step| replay(&s, "SettingsParity.test_bad_arguments_give_the_error_strings", step, BOTH).stdout.trim().to_string()).collect();
        assert_eq!(said, ["bad key", "bad source", "bad lid action", "bad profile"]);
    }

    #[test]
    fn test_non_numbers_are_a_bad_value() {
        let s = Sandbox::new();
        Settings::default().setup(&s);
        let r = s.rust(&["set", "ac", "lock", "abc"]);
        assert_eq!((r.code, text(&r)), (1, "bad value"));
    }

    #[test]
    fn test_travel_holding_the_cpu_only_saves_the_profile() {
        let setup = |s: &Sandbox| {
            Settings::default().setup(s);
            s.write(TRAVEL_FILE, &travel_snapshot(json!({})).to_string());
        };
        let (s, last) = checked("SettingsParity.test_travel_holding_the_cpu_only_saves_the_profile", &setup, &[&["set", "ac", "profile", "saver"]], BOTH);
        assert_eq!(s.read_config()["ac"]["profile"], "power-saver");
        assert!(!called(&last, "powerprofiles-set"));
    }

    #[test]
    fn test_profile_on_the_active_side_is_applied() {
        let (_, last) = checked("SettingsParity.test_profile_on_the_active_side_is_applied", &Settings::default(), &[&["profile", "saver"]], BOTH);
        assert!(has_call(&last, "omarchy-powerprofiles-set ac power-saver"));
    }

    #[test]
    fn test_a_named_profile_never_asks_the_daemon() {
        for step in [&["profile", "saver"][..], &["set", "ac", "profile", "saver"]] {
            let (_, last) = checked("SettingsParity.test_a_named_profile_never_asks_the_daemon", &Settings::default(), &[step], BOTH);
            assert!(!called(&last, "net.hadess"));
        }
    }

    #[test]
    fn test_profile_on_the_battery_side_is_applied() {
        let (_, last) = checked("SettingsParity.test_profile_on_the_battery_side_is_applied", &Settings { battery: true, ..Settings::default() }, &[&["profile", "full"]], BOTH);
        assert!(has_call(&last, "omarchy-powerprofiles-set battery performance"));
    }

    #[test]
    fn test_profile_for_the_inactive_side_is_only_remembered() {
        let (s, last) = checked("SettingsParity.test_profile_for_the_inactive_side_is_only_remembered", &Settings::default(), &[&["set", "battery", "profile", "saver"]], BOTH);
        assert_eq!(read(&s, "home/.local/state/omarchy/powerprofiles/battery"), "power-saver\n");
        assert!(!called(&last, "powerprofiles-set"));
    }

    #[test]
    fn test_failed_profile_change_reports_the_tool_output() {
        let setup = |s: &Sandbox| {
            Settings::default().setup(s);
            s.stub("omarchy-powerprofiles-set", "", 1, "no daemon\n");
        };
        let (_, last) = checked("SettingsParity.test_failed_profile_change_reports_the_tool_output", &setup, &[&["profile", "saver"]], BOTH);
        assert_eq!((last.code, text(&last)), (1, "no daemon"));
    }

    #[test]
    fn test_missing_arguments_are_usage() {
        let s = Sandbox::new();
        assert_eq!([&["set", "ac", "lock"][..], &["profile"]].map(|a| s.rust(a).code), [2, 2]);
    }

    #[test]
    fn test_shell_json_keeps_everything_but_idle() {
        let raw = r#"{"zeta": "café", "idle": {"screensaver": 1, "other": 0.30000000000000004}, "alpha": [1]}"#;
        let s = Sandbox::new();
        Settings { shell: Some(raw.into()), ..Settings::default() }.setup(&s);
        assert_eq!(s.rust(&["set", "ac", "lock", "600"]).code, 0);
        let written = read(&s, SHELL_FILE);
        let shell = parsed(&written);
        assert_eq!(shell.as_object().unwrap().keys().collect::<Vec<_>>(), ["zeta", "idle", "alpha"]);
        same(&shell["idle"], json!({"screensaver": 150, "other": 0.30000000000000004, "lock": 600}));
        assert!(written.contains("café"));
        assert!(written.contains("0.30000000000000004"));
    }

    #[test]
    fn test_malformed_shell_json_is_left_alone() {
        let (s, last) = checked("SettingsParity.test_malformed_shell_json_is_left_alone", &Settings { shell: Some("{broken".into()), ..Settings::default() }, &[&["set", "ac", "lock", "600"]], BOTH);
        assert_eq!((last.code, text(&last), read(&s, SHELL_FILE).as_str()), (1, "shell.json unreadable", "{broken"));
    }

    #[test]
    fn test_load_asks_for_profiles_only_when_ac_lacks_one() {
        let setup = |s: &Sandbox| {
            s.write(CONFIG_FILE, r#"{"ac": {"profile": "saver"}, "battery": {}}"#);
            s.profiles(PROFILES);
        };
        let (_, last) = checked("SettingsParity.test_load_asks_for_profiles_only_when_ac_lacks_one", &setup, &[&["config"]], BOTH);
        assert!(!called(&last, "net.hadess"));
    }

    #[test]
    fn test_status_asks_for_profiles_once() {
        let s = Sandbox::new();
        s.profiles(PROFILES);
        assert_eq!(s.rust(&["status"]).calls.iter().filter(|c| c.contains("net.hadess")).count(), 1);
    }
}

pub(crate) fn framework_setup(limit: u32, keys: Value) -> impl Fn(&Sandbox) {
    move |s| {
        s.framework(true);
        s.install_helper(false);
        s.config(keys.clone());
        s.charge_limit(limit);
    }
}

fn battery_setup(devices: &[&str], fail: bool) -> impl Fn(&Sandbox) {
    let listing: String = devices.iter().map(|d| format!("{d}\n")).collect();
    move |s| {
        s.config(json!({}));
        s.stub("upower", "-e", 0, &listing);
        s.stub("busctl", "ChargeThresholdSupported", 0, "b true\n");
        s.stub("busctl", "EnableChargeThreshold", i32::from(fail), "");
    }
}

fn no_helper_setup(keys: Value) -> impl Fn(&Sandbox) {
    move |s| {
        s.framework(true);
        s.config(keys.clone());
    }
}

fn no_tool_setup(keys: Value) -> impl Fn(&Sandbox) {
    move |s| {
        s.framework(false);
        s.install_helper(false);
        s.config(keys.clone());
    }
}

mod charge_verb_parity {
    use super::*;

    #[test]
    fn test_missing_framework_tool() {
        const T: &str = "ChargeVerbParity.test_missing_framework_tool";
        let message = "framework_tool is not installed";
        for step in [&["charge", "70"][..], &["boost", "on"], &["boost", "off"]] {
            let (_, r) = checked(T, &no_tool_setup(json!({"chargeLimit": 80})), &[step], BOTH);
            assert_eq!((r.code, text(&r)), (1, message), "{step:?}");
        }
        let s = Sandbox::new();
        no_tool_setup(json!({}))(&s);
        let rs = replay(&s, T, &["status"], D);
        assert_eq!(j(&rs)["charge"]["reason"], message);
    }

    #[test]
    fn test_charge_without_the_helper_goes_through_pkexec() {
        for (code, out) in [(0, "ok"), (1, "privilege required")] {
            let setup = move |s: &Sandbox| {
                no_helper_setup(json!({"chargeLimit": 80}))(s);
                s.stub("pkexec", "charge-limit", code, "");
            };
            let (s, r) = checked("ChargeVerbParity.test_charge_without_the_helper_goes_through_pkexec", &setup, &[&["charge", "70"], &["boost", "on"]], BOTH);
            let cfg = s.read_config();
            assert_eq!((r.code, text(&r)), (code, out));
            assert_eq!((&cfg["boostSavedLimit"], &cfg["boost"]), (&json!(if code == 0 { 70 } else { 80 }), &json!(true)));
            assert!(r.calls_text().contains("ops-priv charge-limit 100"));
            assert!(!r.calls_text().contains("sudo"));
        }
    }

    #[test]
    fn test_boost_off_without_the_helper() {
        for code in [0, 1] {
            let setup = move |s: &Sandbox| {
                no_helper_setup(json!({"boost": true, "boostSavedLimit": 75}))(s);
                s.stub("pkexec", "charge-limit", code, "");
            };
            let (s, r) = checked("ChargeVerbParity.test_boost_off_without_the_helper", &setup, &[&["boost", "off"]], BOTH);
            assert_eq!((r.code, s.read_config()["boost"].clone()), (code, json!(false)));
            assert!(r.calls_text().contains("ops-priv charge-limit 75"));
        }
    }

    #[test]
    fn test_charge_saves_the_limit_and_the_boost_restore_point() {
        for (keys, saved) in [(json!({}), 70), (json!({"boost": true, "boostSavedLimit": 60}), 60)] {
            let (s, r) = checked("ChargeVerbParity.test_charge_saves_the_limit_and_the_boost_restore_point", &framework_setup(90, keys), &[&["charge", "70"]], BOTH);
            let cfg = s.read_config();
            assert_eq!((text(&r), &cfg["chargeLimit"], &cfg["boostSavedLimit"]), ("ok", &json!(70), &json!(saved)));
        }
    }

    #[test]
    fn test_charge_clamps_the_tool_argument_and_saves_the_raw_value() {
        for (raw, sent) in [(30, 40), (110, 100)] {
            let (s, r) = checked("ChargeVerbParity.test_charge_clamps_the_tool_argument_and_saves_the_raw_value", &framework_setup(90, json!({})), &[&["charge", &raw.to_string()]], BOTH);
            assert_eq!(s.read_config()["chargeLimit"], raw);
            assert!(r.calls_text().contains(&format!("ops-priv charge-limit {sent}")));
        }
    }

    #[test]
    fn test_charge_failure_leaves_the_config_alone() {
        let setup = |s: &Sandbox| {
            s.stub("sudo", "charge-limit 70", 1, "");
            framework_setup(90, json!({"chargeLimit": 80}))(s);
        };
        let (s, r) = checked("ChargeVerbParity.test_charge_failure_leaves_the_config_alone", &setup, &[&["charge", "70"]], BOTH);
        assert_eq!((r.code, text(&r), s.read_config()["chargeLimit"].clone()), (1, "privilege required", json!(80)));
    }

    #[test]
    fn test_charge_without_a_framework() {
        let (s, r) = checked("ChargeVerbParity.test_charge_without_a_framework", &|s: &Sandbox| s.config(json!({})), &[&["charge", "70"]], BOTH);
        assert_eq!((r.code, text(&r), s.read_config().get("chargeLimit").is_some()), (1, "not a Framework machine", false));
    }

    #[test]
    fn test_charge_with_a_bad_value_changes_nothing() {
        for bad in ["abc", "", "7.5"] {
            let s = Sandbox::new();
            framework_setup(90, json!({"chargeLimit": 80}))(&s);
            let before = s.snapshot(None);
            let r = s.rust(&["charge", bad]);
            assert_eq!((r.code, text(&r), r.calls.as_slice()), (1, "bad value", &[][..]), "{bad:?}");
            assert_eq!(s.snapshot(None), before, "{bad:?}");
        }
    }

    #[test]
    fn test_boost_on_stores_the_live_limit_then_goes_to_100() {
        type Case = (Box<dyn Fn(&Sandbox)>, u32);
        let cases: Vec<Case> = vec![
            (Box::new(framework_setup(90, json!({"chargeLimit": 80}))), 90), (Box::new(framework_setup(0, json!({"chargeLimit": 65}))), 65),
            (Box::new(framework_setup(0, json!({}))), 80), (Box::new(|s| s.config(json!({"chargeLimit": 65}))), 65),
            (Box::new(|s| s.config(json!({}))), 80)];
        for (setup, stored) in cases {
            let (s, _) = checked("ChargeVerbParity.test_boost_on_stores_the_live_limit_then_goes_to_100", &setup, &[&["boost", "on"]], BOTH);
            let cfg = s.read_config();
            assert_eq!((&cfg["boost"], &cfg["boostSavedLimit"]), (&json!(true), &json!(stored)));
        }
    }

    #[test]
    fn test_boost_off_restores_the_saved_limit() {
        for (keys, sent) in [(json!({"boostSavedLimit": 75}), 75), (json!({"boostSavedLimit": 0, "chargeLimit": 65}), 65), (json!({}), 80)] {
            let keys = merged(json!({"boost": true}), keys);
            let (s, r) = checked("ChargeVerbParity.test_boost_off_restores_the_saved_limit", &framework_setup(100, keys), &[&["boost", "off"]], BOTH);
            assert_eq!(s.read_config()["boost"], false);
            assert!(r.calls_text().contains(&format!("ops-priv charge-limit {sent}")));
        }
    }

    #[test]
    fn test_boost_spellings_round_trip() {
        for (on, off) in [("1", "0"), ("true", "false"), ("on", "off"), ("on", "x")] {
            let (s, _) = checked("ChargeVerbParity.test_boost_spellings_round_trip", &framework_setup(90, json!({"chargeLimit": 80})), &[&["boost", on], &["boost", off]], BOTH);
            assert_eq!(s.read_config()["boost"], false);
        }
    }

    #[test]
    fn test_failing_privilege_keeps_boost_saved_and_exits_1() {
        let setup = |s: &Sandbox| {
            s.stub("sudo", "charge-limit 100", 1, "");
            framework_setup(90, json!({"chargeLimit": 80}))(s);
        };
        let (s, r) = checked("ChargeVerbParity.test_failing_privilege_keeps_boost_saved_and_exits_1", &setup, &[&["boost", "on"]], BOTH);
        let cfg = s.read_config();
        assert_eq!((r.code, text(&r), &cfg["boost"], &cfg["boostSavedLimit"]), (1, "privilege required", &json!(true), &json!(90)));
    }

    #[test]
    fn test_health_follows_its_spellings() {
        for arg in ["on", "off", "1", "true", "0", "x"] {
            let (_, r) = checked("ChargeVerbParity.test_health_follows_its_spellings", &battery_setup(&[BATTERY], false), &[&["health", arg]], BOTH);
            let desired = if ["on", "1", "true"].contains(&arg) { "true" } else { "false" };
            assert!(r.calls_text().contains(&format!("EnableChargeThreshold b {desired}")), "{arg}");
        }
    }

    #[test]
    fn test_health_updates_every_battery_and_reports_one_failure() {
        let second = BATTERY.replace("BAT0", "BAT1");
        for fail in [false, true] {
            let (_, r) = checked("ChargeVerbParity.test_health_updates_every_battery_and_reports_one_failure", &battery_setup(&[BATTERY, &second], fail), &[&["health", "on"]], BOTH);
            let want = if fail { (2, "Failed to update charge limits", 1) } else { (2, "ok", 0) };
            assert_eq!((r.calls_text().matches("EnableChargeThreshold").count(), text(&r), r.code), want);
        }
    }

    #[test]
    fn test_health_without_batteries() {
        let (_, r) = checked("ChargeVerbParity.test_health_without_batteries", &|s: &Sandbox| s.config(json!({})), &[&["health", "on"]], BOTH);
        assert_eq!((r.code, text(&r)), (1, "Battery charge limits are not supported"));
    }

    #[test]
    fn test_helper_install() {
        type Case = (Box<dyn Fn(&Sandbox)>, i32, &'static str);
        let cases: Vec<Case> = vec![
            (Box::new(|s| s.stub("pkexec", "helper-install", 0, "")), 0, "ok"),
            (Box::new(|s| s.stub("pkexec", "helper-install", 1, "denied\n")), 1, "denied"),
            (Box::new(|_| {}), 1, "helper install failed")];
        for (setup, code, out) in cases {
            let (s, r) = checked("ChargeVerbParity.test_helper_install", &setup, &[&["helper-install"]], BOTH);
            assert_eq!((r.code, text(&r)), (code, out));
            assert!(r.calls_text().contains(&format!("pkexec {}/ops-priv helper-install", s.bin.display())));
        }
    }
}

/// What a trailing `data` adds to an action: reads the recorded run made and the binary does not fork the same way.
const READS: &[&str] = &["busctl get-property org.freedesktop.UPower", "systemctl is-active"];
const ACTION_DROP: &[&str] = &["hyprctl", "busctl get-property net.hadess", "omarchy-powerprofiles-list", READS[0], READS[1]];

#[derive(Clone)]
pub(crate) struct Tweaks {
    pub prefs: Option<String>,
    pub travel: Option<String>,
    pub effects: (bool, bool),
}

impl Default for Tweaks {
    fn default() -> Self {
        Self { prefs: None, travel: None, effects: (true, true) }
    }
}

impl Tweaks {
    pub fn prefs(self, prefs: Value) -> Self {
        Self { prefs: Some(prefs.to_string()), ..self }
    }

    pub fn travel(self, travel: Value) -> Self {
        Self { travel: Some(travel.to_string()), ..self }
    }

    pub fn effects(self, animations: bool, shadow: bool) -> Self {
        Self { effects: (animations, shadow), ..self }
    }
}

impl Setup for Tweaks {
    fn setup(&self, s: &Sandbox) {
        s.config(json!({}));
        s.stub("pkexec", "", 0, "");
        s.reply("getoption animations:enabled", &format!("bool: {}\nset: true\n", self.effects.0));
        s.reply("getoption decoration:shadow:enabled", &format!("int: {}\nset: true\n", i32::from(self.effects.1)));
        if let Some(prefs) = &self.prefs {
            s.write(TWEAKS_FILE, prefs);
        }
        if let Some(travel) = &self.travel {
            s.write(TRAVEL_FILE, travel);
        }
    }
}

mod action_parity {
    use super::*;

    const ROWS: [&[&str]; 7] = [
        &["action", "mode", "saver"], &["action", "turbo", "off"], &["action", "epp", "power"], &["action", "wifi", "on"],
        &["action", "persist", "wifi", "on"], &["action", "unpersist", "wifi"], &["action", "daylight", "on"],
    ];

    #[test]
    fn test_action_changes_then_prints_data() {
        let opts = Replay { drop: ACTION_DROP, stdout: false, ..MASKED };
        for rs in ROWS {
            checked("ActionParity.test_action_changes_then_prints_data", &Tweaks::default(), &[rs], opts);
            let s = Sandbox::new();
            Tweaks::default().setup(&s);
            s.stub("omarchy-powerprofiles-list", "", 0, "");
            let out = s.rust(rs);
            assert_eq!((out.code, j(&out).get("battery").is_some()), (0, true), "{rs:?}");
        }
    }

    #[test]
    fn test_action_with_a_failing_write_still_prints_data() {
        let s = Sandbox::new();
        s.config(json!({}));
        for argv in [&["action", "wifi", "on"][..], &["action", "nonsense"], &["action", "epp", ""]] {
            let out = s.rust(argv);
            assert_eq!((out.code, j(&out).get("battery").is_some()), (0, true), "{argv:?}");
        }
    }

    #[test]
    fn test_removed_install_verbs_are_unknown_verbs() {
        let s = Sandbox::new();
        s.config(json!({}));
        for verb in ["install", "install-wifi-fix"] {
            let out = s.rust(&["action", verb]);
            assert_eq!((out.code, j(&out).get("battery").is_some()), (0, true), "{verb}");
        }
    }

    #[test]
    fn test_terminal_action_quotes_the_path() {
        let s = Sandbox::new();
        s.config(json!({}));
        let bin_dir = s.base.join("a b/bin");
        let script = s.base.join("a b/extras/sleep-tracking/apply.sh");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "").unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(&s.calls_log, "").unwrap();
        let mut command = Command::new(RUST_BIN);
        command.args(["action", "install-sleep-tracking"]).env_clear().envs(s.env()).env("POWER_HUB_BIN", &bin_dir).current_dir(&s.base);
        let p = run(&mut command);
        let mut calls = String::new();
        for _ in 0..100 {
            calls = fs::read_to_string(&s.calls_log).unwrap().trim().to_string();
            if !calls.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!((p.status.code(), p.stdout.as_slice()), (Some(0), &b""[..]));
        assert_eq!(calls, format!("omarchy-launch-floating-terminal-with-presentation bash '{}'", script.display()));
    }

    #[test]
    fn test_python_free_panel_actions() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(!fs::read_to_string(repo.join("PowerService.qml")).unwrap().contains("battery-plus-action"));
        for gone in ["battery-plus-action", "battery-plus-priv"] {
            assert!(!repo.join("bin").join(gone).exists(), "{gone}");
        }
    }
}

mod tweak_parity {
    use super::*;

    #[test]
    fn test_privileged_tweaks_follow_their_spellings() {
        tweak_both("TweakParity.test_privileged_tweaks_follow_their_spellings", &Tweaks::default(),
                   &[&["tweak", "wifi", "on"], &["tweak", "wifi", "off"], &["tweak", "usb", "true"], &["tweak", "pcie", "x"]]);
    }

    #[test]
    fn test_failing_privilege_reports_and_still_saves() {
        tweak_both("TweakParity.test_failing_privilege_reports_and_still_saves", &|s: &Sandbox| s.config(json!({})), &[&["tweak", "wifi", "on"]]);
    }

    #[test]
    fn test_persist_writes_the_unit_and_enables_it() {
        for spelling in ["persist", "1", "pin", "no"] {
            let (s, r) = tweak_both("TweakParity.test_persist_writes_the_unit_and_enables_it", &Tweaks::default(), &[&["tweak", "wifi", "on", spelling]]);
            let enabled = r.calls_text().contains("systemctl --user enable --now omarchy-power-hub-tweaks.service");
            assert_eq!((s.target(UNIT_FILE).exists(), enabled), (spelling != "no", spelling != "no"), "{spelling}");
        }
    }

    #[test]
    fn test_eval_error_reply_is_reported_by_both() {
        let setup = |s: &Sandbox| {
            Tweaks::default().setup(s);
            s.hypr().eval_reply = "error: no such monitor".into();
        };
        tweak_both("TweakParity.test_eval_error_reply_is_reported_by_both", &setup, &[&["tweak", "effects", "on"]]);
        let s = Sandbox::new();
        setup(&s);
        assert!(s.rust(&["tweak", "effects", "on"]).stdout.contains("error: no such monitor"));
    }

    #[test]
    fn test_effects_on_saves_the_previous_values_and_sends_both_evals() {
        for (animations, shadow) in [(true, true), (false, true)] {
            let (s, r) = tweak_both("TweakParity.test_effects_on_saves_the_previous_values_and_sends_both_evals", &Tweaks::default().effects(animations, shadow), &[&["tweak", "effects", "on"]]);
            assert_eq!(parsed(&read(&s, TWEAKS_FILE))["effectsBefore"], json!({"animations:enabled": animations, "decoration:shadow:enabled": shadow}));
            assert_eq!(r.evals, ["hl.config({ animations = { enabled = false } })", "hl.config({ decoration = { shadow = { enabled = false } } })"]);
        }
    }

    #[test]
    fn test_effects_off_restores_from_the_saved_values() {
        const T: &str = "TweakParity.test_effects_off_restores_from_the_saved_values";
        let before = json!({"effectsBefore": {"animations:enabled": false, "decoration:shadow:enabled": true}});
        tweak_both(T, &Tweaks::default().prefs(before), &[&["tweak", "effects", "off"]]);
        tweak_both(T, &Tweaks::default().prefs(json!({"effectsBefore": {"animations:enabled": null}})), &[&["tweak", "effects", "off"]]);
    }

    #[test]
    fn test_effects_off_without_a_record_defaults_to_enabled() {
        let (_, r) = tweak_both("TweakParity.test_effects_off_without_a_record_defaults_to_enabled", &Tweaks::default(), &[&["tweak", "effects", "off"]]);
        assert_eq!(r.evals, ["hl.config({ animations = { enabled = true } })", "hl.config({ decoration = { shadow = { enabled = true } } })"]);
    }

    #[test]
    fn test_second_effects_on_keeps_the_first_record() {
        tweak_both("TweakParity.test_second_effects_on_keeps_the_first_record", &Tweaks::default(), &[&["tweak", "effects", "on"], &["tweak", "effects", "on"]]);
        let s = Sandbox::new();
        Tweaks::default().setup(&s);
        s.rust(&["tweak", "effects", "on"]);
        s.reply("getoption animations:enabled", "bool: false\n");
        s.rust(&["tweak", "effects", "on"]);
        assert_eq!(parsed(&read(&s, TWEAKS_FILE))["effectsBefore"]["animations:enabled"], true);
    }

    #[test]
    fn test_effects_unreachable_hyprland_is_an_error() {
        let s = Sandbox::without_hypr();
        s.config(json!({}));
        let r = s.rust(&["tweak", "effects", "on"]);
        assert_eq!(r.code, 1);
        assert!(r.stdout.contains("Hyprland is not reachable"));
    }

    #[test]
    fn test_turbo_is_refused_while_travel_owns_the_cpu() {
        const T: &str = "TweakParity.test_turbo_is_refused_while_travel_owns_the_cpu";
        tweak_both(T, &Tweaks::default().travel(travel_snapshot(json!({}))), &[&["tweak", "turbo", "on"]]);
        tweak_both(T, &Tweaks::default().travel(travel_snapshot(json!({"reduceCpu": false}))), &[&["tweak", "turbo", "on"]]);
    }

    #[test]
    fn test_unpin_daylight_and_cpu_keep_every_other_key() {
        let prefs = json!({"z": 1, "wifi": true, "wifiPinned": true, "usbPinned": true, "daylight": false, "a": [1]});
        tweak_both("TweakParity.test_unpin_daylight_and_cpu_keep_every_other_key", &Tweaks::default().prefs(prefs),
                   &[&["unpin", "wifi"], &["unpin", "missing"], &["daylight", "on"], &["daylight", "off"], &["daylight", "x"]]);
    }

    #[test]
    fn test_cpu_pins_epp_and_leaves_turbo_alone() {
        const T: &str = "TweakParity.test_cpu_pins_epp_and_leaves_turbo_alone";
        tweak_both(T, &Tweaks::default().prefs(json!({"z": 1})), &[&["cpu", "epp", "power"], &["cpu", "turbo", "off"]]);
        tweak_both(T, &Tweaks::default(), &[&["cpu", "epp", "power"]]);
    }

    #[test]
    fn test_failed_cpu_change_is_not_saved() {
        tweak_both("TweakParity.test_failed_cpu_change_is_not_saved", &|s: &Sandbox| s.config(json!({})), &[&["cpu", "epp", "power"]]);
    }

    #[test]
    fn test_cpu_is_refused_while_travel_owns_it() {
        let setup = Tweaks::default().prefs(json!({"z": 1})).travel(travel_snapshot(json!({})));
        tweak_both("TweakParity.test_cpu_is_refused_while_travel_owns_it", &setup, &[&["cpu", "epp", "power"], &["cpu", "turbo", "off"]]);
    }

    #[test]
    fn test_apply_tweaks_replays_the_pinned_keys() {
        const T: &str = "TweakParity.test_apply_tweaks_replays_the_pinned_keys";
        let mut prefs = json!({});
        for k in ["wifi", "pcie", "usb", "audio", "abm", "effects", "turbo"] {
            prefs[format!("{k}Pinned")] = json!(true);
        }
        let prefs = merged(prefs, json!({"wifi": true, "usb": false, "effects": true, "turbo": true, "pcie": 0, "epp": "power", "eppPinned": true}));
        tweak_both(T, &Tweaks::default().prefs(prefs), &[&["apply-tweaks"]]);
        tweak_both(T, &Tweaks::default().prefs(json!({"wifi": true, "usbPinned": true})), &[&["apply-tweaks"]]);
        tweak_both(T, &Tweaks::default().prefs(json!({})), &[&["apply-tweaks"]]);
        tweak_both(T, &|s: &Sandbox| s.config(json!({})), &[&["apply-tweaks"]]);
    }

    #[test]
    fn test_apply_tweaks_skips_the_cpu_while_travel_owns_it() {
        let prefs = json!({"turboPinned": true, "turbo": true, "wifiPinned": true, "wifi": true, "eppPinned": true, "epp": "power"});
        tweak_both("TweakParity.test_apply_tweaks_skips_the_cpu_while_travel_owns_it", &Tweaks::default().prefs(prefs).travel(travel_snapshot(json!({}))), &[&["apply-tweaks"]]);
    }

    #[test]
    fn test_apply_tweaks_reports_the_last_failure() {
        for (pins, last) in [(&["wifiPinned", "usbPinned", "eppPinned"][..], "epp failed"), (&["wifiPinned", "usbPinned"], "usb failed")] {
            let setup = |s: &Sandbox| {
                s.config(json!({}));
                let mut prefs = json!({});
                for k in pins {
                    prefs[*k] = json!(true);
                }
                prefs["epp"] = json!("power");
                s.write(TWEAKS_FILE, &prefs.to_string());
                for key in ["wifi", "usb", "epp"] {
                    s.stub("pkexec", key, 1, &format!("{key} failed\n"));
                }
            };
            let (_, r) = tweak_both("TweakParity.test_apply_tweaks_reports_the_last_failure", &setup, &[&["apply-tweaks"]]);
            assert_eq!(text(&r), last);
        }
    }

    #[test]
    fn test_tweaks_unit_runs_the_binary() {
        let s = Sandbox::new();
        Tweaks::default().setup(&s);
        s.rust(&["tweak", "wifi", "on", "persist"]);
        let unit = read(&s, UNIT_FILE);
        let bin = s.bin.display().to_string();
        assert!(unit.contains(&format!("ExecStart={bin}/power-hub apply-tweaks\n")));
        assert_eq!(unit.replace(&format!("{bin}/power-hub"), "X"), "[Unit]\nDescription=Omarchy Power Hub device tweaks\n\n[Service]\nType=oneshot\nExecStart=X apply-tweaks\n\n[Install]\nWantedBy=default.target\n");
    }

    #[test]
    fn test_list_in_tweaks_file_is_replaced_not_a_crash() {
        let s = Sandbox::new();
        Tweaks { prefs: Some("[1]".into()), ..Tweaks::default() }.setup(&s);
        let r = s.rust(&["tweak", "wifi", "on"]);
        assert_eq!((r.code, parsed(&read(&s, TWEAKS_FILE))), (0, json!({"wifi": true})));
    }
}

fn apply_keys() -> Value {
    json!({"ac": {"profile": "performance", "screensaver": 600, "lock": 900, "lid": "display-off"},
           "battery": {"profile": "balanced", "screensaver": 120, "lock": 240, "lid": "nothing"}})
}

pub(crate) fn cpu_new() -> Value {
    json!({"cpu": {"turbo": null, "epp": []}, "pending": ["profile", "brightness", "refresh"]})
}

/// A restore point that holds only the display.
pub(crate) fn display_only() -> Value {
    travel_snapshot(json!({"reduceCpu": false, "profile": null, "cpu": {"turbo": null, "epp": []}, "pending": ["brightness"], "monitor": null,
                           "controls": merged(frozen(), json!({"refresh": {"available": false, "detail": "x"}}))}))
}

fn no_profile_control() -> Value {
    travel_snapshot(json!({"profile": null, "pending": ["brightness", "refresh"], "cpu": cpu_new()["cpu"],
                           "controls": merged(frozen(), json!({"profile": {"available": false, "detail": "No profile"}}))}))
}

pub(crate) struct ApplyEnv {
    pub travel: Option<Value>,
    pub lid: &'static str,
    pub on_battery: bool,
    pub profile: &'static str,
    pub keys: Value,
}

impl Default for ApplyEnv {
    fn default() -> Self {
        Self { travel: None, lid: "open", on_battery: false, profile: "balanced", keys: apply_keys() }
    }
}

impl Setup for ApplyEnv {
    fn setup(&self, s: &Sandbox) {
        Settings { bindings: Some("bind = x".into()), battery: self.on_battery, ..Settings::default() }.setup(s);
        s.config(self.keys.clone());
        s.lid(self.lid);
        s.stub("pkexec", "", 0, "");
        s.stub("omarchy-powerprofiles-set", "", 0, "");
        s.stateful_profile(self.profile);
        if let Some(travel) = &self.travel {
            s.write(TRAVEL_FILE, &travel.to_string());
        }
    }
}

/// `both(..., normalise=mask_exec, inspect=no_leftovers)`.
fn apply_check(name: &str, setup: &impl Setup, steps: &[&[&str]]) -> (Sandbox, Outcome) {
    checked_each(name, setup, steps, MASKED, &mut |s, _| no_leftovers(s))
}

mod apply_parity {
    use super::*;

    #[test]
    fn test_apply_profile_writes_profile_idle_and_lid_on_either_source() {
        for (battery, side, profile, saver) in [(false, "ac", "performance", 600), (true, "battery", "balanced", 120)] {
            let (s, r) = apply_check("ApplyParity.test_apply_profile_writes_profile_idle_and_lid_on_either_source", &ApplyEnv { on_battery: battery, ..ApplyEnv::default() }, &[&["apply-profile"]]);
            assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"));
            assert!(has_call(&r, &format!("omarchy-powerprofiles-set {side} {profile}")));
            same(&shell_of(&s)["idle"]["screensaver"], json!(saver));
            assert!(read(&s, BINDINGS_FILE).contains("lid-closed"));
        }
    }

    #[test]
    fn test_hibernate_delay_is_applied_only_above_zero() {
        for (after, expected) in [(900, true), (0, false)] {
            let setup = move |s: &Sandbox| {
                let mut ac = apply_keys()["ac"].clone();
                ac["hibernateAfter"] = json!(after);
                ApplyEnv { keys: json!({"ac": ac}), ..ApplyEnv::default() }.setup(s);
                s.install_helper(false);
                s.stub("busctl", "CanHibernate", 0, "s \"yes\"\n");
            };
            let (_, r) = apply_check("ApplyParity.test_hibernate_delay_is_applied_only_above_zero", &setup, &[&["apply-profile"]]);
            assert_eq!(called(&r, "hibernate-delay"), expected, "{after}");
        }
    }

    #[test]
    fn test_travel_owning_the_cpu_keeps_the_profile_and_reapplies_the_overlay() {
        let setup = ApplyEnv { travel: Some(travel_snapshot(cpu_new())), profile: "performance", ..ApplyEnv::default() };
        let (_, r) = apply_check("ApplyParity.test_travel_owning_the_cpu_keeps_the_profile_and_reapplies_the_overlay", &setup, &[&["apply-profile"]]);
        assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"));
        assert!(has_call(&r, "powerprofilesctl set power-saver"));
        assert!(!called(&r, "omarchy-powerprofiles-set"));
    }

    #[test]
    fn test_travel_without_a_profile_control_applies_the_normal_profile_first() {
        let (_, r) = apply_check("ApplyParity.test_travel_without_a_profile_control_applies_the_normal_profile_first", &ApplyEnv { travel: Some(no_profile_control()), ..ApplyEnv::default() }, &[&["apply-profile"]]);
        assert!(has_call(&r, "omarchy-powerprofiles-set ac performance"));
        assert!(!called(&r, "powerprofilesctl set"));
    }

    #[test]
    fn test_normal_profile_that_is_not_offered_is_skipped() {
        let setup = |s: &Sandbox| {
            ApplyEnv { travel: Some(no_profile_control()), ..ApplyEnv::default() }.setup(s);
            s.profiles(&["power-saver", "balanced"]);
        };
        let (_, r) = apply_check("ApplyParity.test_normal_profile_that_is_not_offered_is_skipped", &setup, &[&["apply-profile"]]);
        assert!(!called(&r, "omarchy-powerprofiles-set"));
    }

    #[test]
    fn test_incomplete_travel_shows_its_saved_error_and_still_writes_idle() {
        for error in ["Power saver was replaced by balanced", ""] {
            let point = travel_snapshot(merged(json!({"complete": false, "error": error}), cpu_new()));
            let (s, r) = apply_check("ApplyParity.test_incomplete_travel_shows_its_saved_error_and_still_writes_idle", &ApplyEnv { travel: Some(point), ..ApplyEnv::default() }, &[&["apply-profile"]]);
            let start = if error.is_empty() { "Travel mode restoration is incomplete" } else { error };
            assert_eq!((r.code, r.stdout.starts_with(start)), (1, true));
            same(&shell_of(&s)["idle"]["screensaver"], json!(600));
            assert!(!called(&r, "powerprofilesctl set"));
        }
    }

    #[test]
    fn test_overlay_failure_marks_travel_incomplete() {
        let setup = |s: &Sandbox| {
            ApplyEnv { travel: Some(travel_snapshot(cpu_new())), ..ApplyEnv::default() }.setup(s);
            s.stateful("powerprofilesctl", r#"case "$1" in get) echo balanced;; *) echo "authorization denied"; exit 1;; esac"#);
        };
        let (s, r) = apply_check("ApplyParity.test_overlay_failure_marks_travel_incomplete", &setup, &[&["apply-profile"]]);
        let saved = parsed(&read(&s, TRAVEL_FILE));
        assert_eq!((r.code, r.stdout.as_str(), &saved["complete"], &saved["error"]), (1, "authorization denied\n", &json!(false), &json!("authorization denied")));
    }

    #[test]
    fn test_overlay_that_is_replaced_reports_the_replacement() {
        let setup = |s: &Sandbox| {
            ApplyEnv { travel: Some(travel_snapshot(cpu_new())), ..ApplyEnv::default() }.setup(s);
            s.stateful("powerprofilesctl", r#"case "$1" in get) echo balanced;; *) ;; esac"#);
        };
        let (_, r) = apply_check("ApplyParity.test_overlay_that_is_replaced_reports_the_replacement", &setup, &[&["apply-profile"]]);
        assert_eq!(r.stdout, "Power saver was replaced by balanced\n");
    }

    #[test]
    fn test_unwritable_run_dir_ends_the_verb_with_nothing_to_record() {
        let s = Sandbox::new();
        ApplyEnv { travel: Some(travel_snapshot(cpu_new())), ..ApplyEnv::default() }.setup(&s);
        let dir = s.target(TRAVEL_FILE).parent().unwrap().to_path_buf();
        chmod(&dir, 0o555);
        let _restore = Restore(dir);
        let r = replay(&s, "ApplyParity.test_unwritable_run_dir_ends_the_verb_with_nothing_to_record", &["apply-profile"], UNCAUGHT);
        assert_eq!(r.code, 1);
    }

    #[test]
    fn test_display_only_source_transition() {
        let (s, r) = apply_check("ApplyParity.test_display_only_source_transition", &ApplyEnv { travel: Some(display_only()), on_battery: true, ..ApplyEnv::default() }, &[&["apply-profile"]]);
        assert!(has_call(&r, "omarchy-powerprofiles-set battery balanced"));
        assert_eq!(parsed(&read(&s, TRAVEL_FILE))["on"], true);
    }

    #[test]
    fn test_apply_wakes_the_keyboards_hid_only_with_the_lid_open() {
        for (lid, expected) in [("open", true), ("closed", false)] {
            let (_, r) = apply_check("ApplyParity.test_apply_wakes_the_keyboards_hid_only_with_the_lid_open", &ApplyEnv { lid, ..ApplyEnv::default() }, &[&["apply"]]);
            assert_eq!(called(&r, "hid-wakeup on"), expected, "{lid}");
        }
    }

    #[test]
    fn test_apply_joins_each_stage_error_with_spaces() {
        let setup = |s: &Sandbox| {
            ApplyEnv { travel: Some(travel_snapshot(merged(json!({"complete": false, "error": "E"}), cpu_new()))), lid: "open", ..ApplyEnv::default() }.setup(s);
            s.write(SHELL_FILE, "{broken");
            s.write(TWEAKS_FILE, &json!({"wifi": true, "wifiPinned": true}).to_string());
            s.stub("pkexec", "", 1, "denied\n");
        };
        let (_, r) = apply_check("ApplyParity.test_apply_joins_each_stage_error_with_spaces", &setup, &[&["apply"]]);
        assert_eq!((r.code, r.stdout.as_str()), (1, "E  shell.json unreadable denied denied\n"));
    }

    #[test]
    fn test_apply_is_ok_when_every_stage_is() {
        let (_, r) = apply_check("ApplyParity.test_apply_is_ok_when_every_stage_is", &ApplyEnv::default(), &[&["apply"]]);
        assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"));
    }

    #[test]
    fn test_unwritable_lid_file_ends_the_verb_like_an_uncaught_error() {
        for verb in ["apply", "apply-profile"] {
            let s = Sandbox::new();
            ApplyEnv::default().setup(&s);
            let file = s.target(BINDINGS_FILE);
            chmod(&file, 0o444);
            let dir = file.parent().unwrap().to_path_buf();
            chmod(&dir, 0o555);
            let _restore = Restore(dir);
            let r = replay(&s, "ApplyParity.test_unwritable_lid_file_ends_the_verb_like_an_uncaught_error", &[verb], UNCAUGHT);
            assert_eq!(r.code, 1, "{verb}");
        }
    }

    #[test]
    fn test_unwritable_shell_json_ends_the_verb_like_an_uncaught_error() {
        let s = Sandbox::new();
        ApplyEnv::default().setup(&s);
        chmod(&s.target(SHELL_FILE), 0o444);
        let r = replay(&s, "ApplyParity.test_unwritable_shell_json_ends_the_verb_like_an_uncaught_error", &["apply"], Replay { normalise: Some(without_bindings), ..UNCAUGHT });
        assert_eq!(r.code, 1);
        assert!(!called(&r, "hid-wakeup") && !called(&r, "pkexec"));
    }

    #[test]
    fn test_older_restore_points_are_left_alone_by_apply() {
        let monitors = json!([{"name": "eDP-1", "width": 1920, "height": 1080, "refresh": 165, "x": 0, "y": 0, "scale": 1}]);
        let epp = travel_snapshot(json!({"cpu": {"turbo": null, "epp": [{"cpu": 0, "before": "power", "target": null}]},
                                         "pending": ["profile", "epp:0", "brightness", "refresh"]}));
        let points = [json!({"on": true, "brightness": 80, "monitors": monitors}), json!({"on": true, "monitors": [], "complete": true, "controls": []}),
                      travel_snapshot(json!({})), epp];
        for point in points {
            let s = Sandbox::new();
            ApplyEnv { travel: Some(point), ..ApplyEnv::default() }.setup(&s);
            let remembered = s.target("home/.local/state/omarchy/powerprofiles");
            let files = || {
                let saved: Vec<(String, String)> = fs::read_dir(&remembered).into_iter().flatten().flatten()
                    .map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read_to_string(e.path()).unwrap())).collect();
                (read(&s, TRAVEL_FILE), saved.into_iter().collect::<std::collections::BTreeMap<_, _>>())
            };
            let before = files();
            for verb in ["apply-profile", "apply"] {
                let r = s.rust(&[verb]);
                assert!([0, 1].contains(&r.code), "{verb}");
                assert!(!called(&r, "powerprofilesctl set"), "{verb}");
                assert_eq!(files(), before, "{verb}");
            }
            let r = s.rust(&["travel", "off"]);
            assert_eq!((r.stdout, r.code), (format!("{OLDER}\n"), 1));
        }
    }
}
