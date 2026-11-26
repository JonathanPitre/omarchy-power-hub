//! Displays, lights, the lid, idle commands and recovery, ported from `tests/test_parity.py` class by class under the
//! same test names.
use crate::read::{TWEAKS_FILE, merged, parsed};
use crate::support::{Outcome, RUST_BIN, Replay, Sandbox, replay, spawn_with};
use crate::write::{ApplyEnv, BOTH, Setup, Settings, called, checked_each, chmod, has_call, read, starting};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const KBLIGHT_FILE: &str = "run/omarchy-power-hub/kblight.json";
const LOCK_FILE: &str = "run/omarchy-power-hub/display-off.lock";
const FLIP: &str = "RTK FlipGo-A2 8R33926O00QS";
const SAVER_CLEARED: &str = "hl.config({ cursor = { invisible = false } })";

fn head(name: &str, lit: bool, extra: Value) -> Value {
    merged(json!({"name": name, "dpmsStatus": lit, "width": 2560, "height": 1600, "refreshRate": 60, "x": 0, "y": 0, "scale": 1.6}), extra)
}

fn lit(name: &str) -> Value {
    head(name, true, json!({}))
}

fn dark(name: &str) -> Value {
    head(name, false, json!({}))
}

fn described(name: &str, description: &str) -> Value {
    head(name, true, json!({"description": description}))
}

/// A non-Framework machine, so the lights are a no-op.
struct Displays {
    monitors: Vec<Value>,
    keys: Value,
}

fn displays(monitors: Vec<Value>) -> Displays {
    Displays { monitors, keys: json!({}) }
}

impl Displays {
    fn keys(self, keys: Value) -> Self {
        Self { keys, ..self }
    }
}

impl Setup for Displays {
    fn setup(&self, s: &Sandbox) {
        Settings { keys: self.keys.clone(), ..Settings::default() }.setup(s);
        s.monitors(&self.monitors);
        s.lid("open");
    }
}

fn dpms(action: &str, names: &[&str]) -> Vec<String> {
    names.iter().map(|n| format!("dispatch hl.dsp.dpms({{ action = \"{action}\", monitor = \"{n}\" }})")).collect()
}

fn dispatched(r: &Outcome) -> Vec<String> {
    r.requests.iter().filter(|q| q.starts_with("dispatch")).cloned().collect()
}

fn log_lines(s: &Sandbox) -> Vec<String> {
    fs::read_to_string(&s.calls_log).unwrap().split('\n').map(String::from).collect()
}

fn log_starting(s: &Sandbox, prefix: &str) -> Vec<String> {
    log_lines(s).into_iter().filter(|c| c.starts_with(prefix)).collect()
}

/// The cursor eval and the pkill calls logged when it arrived.
type Seen = Arc<Mutex<Vec<(String, Vec<String>)>>>;

mod display_parity {
    use super::*;

    /// Only DPMS: which heads exist and where they sit is hyprmoncfg's job, so no head is ever disabled or re-added.
    #[test]
    fn test_on_and_off_only_switch_dpms_on_safe_heads() {
        let mons = vec![dark("eDP-1"), described("DP-1", FLIP), head("DP-2", true, json!({"disabled": true})), lit("bad name")];
        let s = Sandbox::new();
        displays(mons).setup(&s);
        for (mode, action, names, evals) in [("off", "disable", &["eDP-1", "DP-1"][..], vec![]), ("on", "enable", &["eDP-1", "DP-1", "DP-2"], vec![SAVER_CLEARED.to_string()])] {
            let r = s.rust(&["displays", mode]);
            assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"), "{mode}");
            assert_eq!((r.evals.clone(), dispatched(&r)), (evals, dpms(action, names)), "{mode}");
            assert!(!s.target("run/omarchy-power-hub/display-off.json").exists(), "{mode}");
        }
    }

    #[test]
    fn test_a_lock_or_saver_that_is_not_a_number_leaves_the_idle_timers_alone() {
        let shell = r#"{"idle": {"screensaver": 150, "lock": 300}}"#;
        for keys in [json!({"lock": "soon", "screensaver": 150}), json!({"lock": 60, "screensaver": "soon"})] {
            let s = Sandbox::new();
            displays(vec![lit("eDP-1")]).keys(json!({"ac": merged(json!({"profile": "balanced"}), keys)})).setup(&s);
            s.write(SHELL_FILE, shell);
            for mode in ["off", "on"] {
                let r = s.rust(&["displays", mode]);
                assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"), "{mode}");
                assert_eq!(read(&s, SHELL_FILE), shell, "{mode}");
                assert!(r.requests.contains(&dpms(if mode == "off" { "disable" } else { "enable" }, &["eDP-1"])[0]), "{mode}");
            }
        }
    }

    #[test]
    fn test_screensaver_is_dismissed_before_anything_else() {
        let pkills = ["pkill -x ttfx", "pkill -f [o]rg.omarchy.screensaver"];
        let seen: Seen = Arc::default();
        let s = Sandbox::new();
        displays(vec![dark("eDP-1")]).setup(&s);
        // What the pkill stub had logged when the cursor eval arrived.
        let (log, seen2) = (s.calls_log.clone(), seen.clone());
        s.hypr().on_eval = Some(Box::new(move |_, lua| {
            let logged: Vec<String> = fs::read_to_string(&log).unwrap().split('\n').filter(|c| c.starts_with("pkill")).map(String::from).collect();
            seen2.lock().unwrap().push((lua.to_string(), logged));
        }));
        let r = s.rust(&["displays", "on"]);
        assert_eq!(r.evals[0], SAVER_CLEARED);
        assert_eq!(log_starting(&s, "pkill"), pkills);
        assert_eq!(seen.lock().unwrap()[0], (SAVER_CLEARED.to_string(), pkills.map(String::from).to_vec()));
    }

    /// Omarchy reads the saver closing as the user returning and relights every head, so off leaves it up.
    #[test]
    fn test_off_leaves_the_screensaver_up() {
        let s = Sandbox::new();
        displays(vec![lit("eDP-1")]).setup(&s);
        let r = s.rust(&["displays", "off"]);
        assert_eq!((log_starting(&s, "pkill"), r.evals.clone(), dispatched(&r)), (Vec::<String>::new(), vec![], dpms("disable", &["eDP-1"])));
    }

    #[test]
    fn test_dpms_is_an_alias() {
        let s = Sandbox::new();
        displays(vec![lit("eDP-1"), lit("DP-1")]).setup(&s);
        for mode in ["off", "on"] {
            let (a, b) = (s.rust(&["dpms", mode]), s.rust(&["displays", mode]));
            assert_eq!((a.code, a.stdout, a.requests), (b.code, b.stdout, b.requests), "{mode}");
        }
    }

    #[test]
    fn test_unopenable_lock_fails_closed() {
        let s = Sandbox::new();
        displays(vec![lit("eDP-1")]).setup(&s);
        for mode in ["on", "off"] {
            let lock = s.target(LOCK_FILE);
            fs::create_dir_all(&lock).unwrap();
            let r = s.rust(&["displays", mode]);
            fs::remove_dir(&lock).unwrap();
            assert_eq!((r.code, r.stdout.as_str(), r.evals.as_slice()), (1, "", &[][..]), "{mode}");
        }
    }
}

mod display_hardening {
    use super::*;

    /// A read that gave up at 2 s would see no monitors and send no enable: the display would stay dark.
    #[test]
    fn test_replies_held_past_two_seconds_still_light_and_blank_every_head() {
        let s = Sandbox::new();
        displays(vec![dark("eDP-1")]).setup(&s);
        s.hypr().reply_delay = Duration::from_millis(2200);
        let r = s.rust(&["displays", "on"]);
        assert_eq!((r.code, r.stdout.trim(), r.evals.as_slice()), (0, "ok", &[SAVER_CLEARED.to_string()][..]));
        assert_eq!(dispatched(&r), dpms("enable", &["eDP-1"]));
        displays(vec![lit("eDP-1"), lit("DP-1")]).setup(&s);
        let r = s.rust(&["displays", "off"]);
        assert_eq!((r.code, r.stdout.trim(), r.evals.as_slice()), (0, "ok", &[][..]));
        assert_eq!(dispatched(&r), dpms("disable", &["eDP-1", "DP-1"]));
    }

    #[test]
    fn test_bad_modes_print_usage_and_touch_nothing() {
        let s = Sandbox::new();
        displays(vec![dark("eDP-1")]).setup(&s);
        for argv in [&["displays"][..], &["displays", "bogus"], &["displays", ""], &["displays", "park", "DP-1"], &["displays", "sync"], &["displays", "disable"], &["dpms", "ON"]] {
            let r = s.rust(argv);
            assert_eq!((r.code, r.evals.as_slice(), r.calls.as_slice()), (2, &[][..], &[][..]), "{argv:?}");
            assert!(r.stdout.starts_with("usage: displays"), "{argv:?}");
        }
    }
}

type Tool = Box<dyn Fn(&Sandbox)>;

/// A Laptop 16 keyboard whose backlight rounds down to 0, 33, 67 or 100.
fn qmk_keys(level: u32) -> Tool {
    Box::new(move |s| {
        let state = s.base.join("keys.state");
        fs::write(&state, format!("{level}\n")).unwrap();
        let body = r#"case "$*" in
  "via --backlight") echo "Brightness: $(<STATE)%";;
  "via --backlight "*) v=$3; if [ "$v" -ge 100 ]; then n=100; elif [ "$v" -ge 67 ]; then n=67; elif [ "$v" -ge 34 ]; then n=33; else n=0; fi; echo $n > STATE;;
  *) exit 1;;
esac"#;
        s.stateful("qmk_hid", &body.replace("STATE", &state.display().to_string()));
    })
}

fn ec_tool_with(keys: u32, fp: &'static str) -> Tool {
    Box::new(move |s| {
        s.stub("framework_tool", "--fp-led-level", 0, "");
        s.stub("framework_tool", "--fp-brightness", 0, fp);
        s.stub("framework_tool", "--kblight", 0, &format!("Keyboard backlight: {keys}%\n"));
    })
}

fn ec_tool() -> Tool {
    ec_tool_with(30, "Requested: Auto\nBrightness: 10%\n")
}

/// A tool that exists and fails every call.
fn failing(tool: &'static str) -> Tool {
    Box::new(move |s| s.write_stub(tool))
}

struct Lights {
    tools: Vec<Tool>,
    model: &'static str,
    vendor: &'static str,
    led: Option<(&'static str, &'static str, &'static str)>,
    lux: Option<u32>,
    scale: Option<&'static str>,
    auto: bool,
    devices: Option<&'static str>,
}

fn lights(tools: Vec<Tool>) -> Lights {
    Lights { tools, model: "Laptop 16", vendor: "Framework", led: None, lux: None, scale: None, auto: false, devices: None }
}

impl Setup for Lights {
    fn setup(&self, s: &Sandbox) {
        displays(vec![lit("eDP-1")]).setup(s);
        s.set_path(&s.stubs.display().to_string()); // the host's own light tools must not be found
        s.dmi(self.vendor, self.model);
        for tool in &self.tools {
            tool(s);
        }
        if let Some((trigger, brightness, max)) = self.led {
            s.power_led(trigger, brightness, max);
        }
        if let Some(lux) = self.lux {
            s.light_sensor(lux, self.scale);
        }
        if self.auto {
            s.write(TWEAKS_FILE, &json!({"keyboard": "auto"}).to_string());
        }
        if let Some(devices) = self.devices {
            s.write("proc/bus/input/devices", devices);
        }
    }
}

fn keys_level(s: &Sandbox) -> i64 {
    fs::read_to_string(s.base.join("keys.state")).unwrap().trim().parse().unwrap()
}

fn led_file(s: &Sandbox, name: &str) -> String {
    fs::read_to_string(s.root.join("sys/class/leds/chromeos:white:power").join(name)).unwrap().trim().to_string()
}

fn kblight_saved(s: &Sandbox) -> Option<Value> {
    let p = s.target(KBLIGHT_FILE);
    p.exists().then(|| parsed(&fs::read_to_string(p).unwrap()))
}

fn light_calls(s: &Sandbox) -> Vec<String> {
    fs::read_to_string(&s.calls_log).unwrap().lines().filter(|c| ["qmk_hid", "framework_tool", "sudo"].iter().any(|t| c.starts_with(t))).map(String::from).collect()
}

const OFF: &[&str] = &["displays", "off"];
const ON: &[&str] = &["displays", "on"];
const AUTO_LED: Option<(&str, &str, &str)> = Some(("chromeos-auto", "80", "100"));

/// Replays one step in the sandbox it is given.
type Go = Box<dyn Fn(&Sandbox, &[&str])>;

mod lights_parity {
    use super::*;

    /// Runs steps one at a time in the sandbox `setup` made, each compared with the next recorded step.
    fn start(setup: &impl Setup, name: &'static str) -> (Sandbox, Go) {
        let s = Sandbox::new();
        setup.setup(&s);
        (s, Box::new(move |s, argv| {
            replay(s, name, argv, BOTH);
        }))
    }

    #[test]
    fn test_laptop_16_levels_survive_a_sleep_because_the_wake_writes_one_more() {
        for step in [33, 67, 100] {
            let (s, go) = start(&lights(vec![qmk_keys(step)]), "LightsParity.test_laptop_16_levels_survive_a_sleep_because_the_wake_writes_one_more");
            go(&s, OFF);
            assert_eq!((keys_level(&s), kblight_saved(&s)), (0, Some(json!({"level": step}))), "{step}");
            go(&s, ON);
            assert_eq!((keys_level(&s), kblight_saved(&s)), (i64::from(step), None), "{step}");
        }
    }

    #[test]
    fn test_laptop_16_wake_writes_the_level_plus_one() {
        let (s, go) = start(&lights(vec![qmk_keys(67)]), "LightsParity.test_laptop_16_wake_writes_the_level_plus_one");
        go(&s, OFF);
        go(&s, ON);
        assert!(fs::read_to_string(&s.calls_log).unwrap().contains("qmk_hid via --backlight 68"));
    }

    #[test]
    fn test_laptop_12_and_13_use_the_ec_keys_and_fingerprint_led() {
        for model in ["Laptop 13 (Intel Core Ultra Series 1)", "Laptop 12"] {
            let (s, go) = start(&Lights { model, ..lights(vec![ec_tool()]) }, "LightsParity.test_laptop_12_and_13_use_the_ec_keys_and_fingerprint_led");
            go(&s, OFF);
            assert_eq!(kblight_saved(&s), Some(json!({"level": 30, "fp": {"requested": "auto", "brightness": 10}})), "{model}");
            go(&s, ON);
            assert_eq!(light_calls(&s), ["framework_tool --kblight 30", "framework_tool --fp-led-level auto"], "{model}");
        }
    }

    #[test]
    fn test_ec_power_button_restores_a_custom_brightness() {
        for (fp, want) in [("Requested: Custom\nBrightness: 55%\n", "framework_tool --fp-brightness 55"), ("Brightness: 0%\n", "framework_tool --fp-brightness 0")] {
            let (s, go) = start(&Lights { model: "Laptop 13", ..lights(vec![ec_tool_with(30, fp)]) }, "LightsParity.test_ec_power_button_restores_a_custom_brightness");
            go(&s, OFF);
            go(&s, ON);
            assert_eq!(light_calls(&s).last().unwrap(), want, "{fp:?}");
        }
    }

    #[test]
    fn test_a_failing_tool_is_retried_with_sudo_then_the_helper() {
        let fails: Tool = Box::new(|s| {
            s.stub("sudo", "ops-priv kblight", 0, "Keyboard backlight: 30%\n");
            s.stub("sudo", "ops-priv fp-brightness", 0, "Brightness: 10%\n");
        });
        let fails = Rc::new(fails);
        for installed in [false, true] {
            let fails = fails.clone();
            let setup = move |s: &Sandbox| {
                let inner = fails.clone();
                Lights { model: "Laptop 13", ..lights(vec![Box::new(move |s| inner(s)), failing("framework_tool")]) }.setup(s);
                if installed {
                    s.install_helper(false);
                }
            };
            let (s, go) = start(&setup, "LightsParity.test_a_failing_tool_is_retried_with_sudo_then_the_helper");
            go(&s, OFF);
            let dir = if installed { s.helper_dir() } else { s.bin.clone() };
            assert!(fs::read_to_string(&s.calls_log).unwrap().contains(&format!("sudo -n {}/ops-priv kblight 0", dir.display())), "{installed}");
            go(&s, ON);
        }
    }

    #[test]
    fn test_sudo_alone_is_enough() {
        let sudo: Tool = Box::new(|s| s.stub("sudo", "framework_tool --kblight", 0, "Keyboard backlight: 30%\n"));
        let (s, go) = start(&Lights { model: "Laptop 13", ..lights(vec![sudo, failing("framework_tool")]) }, "LightsParity.test_sudo_alone_is_enough");
        go(&s, OFF);
        assert_eq!(kblight_saved(&s), Some(json!({"level": 30})));
        go(&s, ON);
    }

    #[test]
    fn test_tool_errors_in_a_zero_exit_are_failures() {
        for text in ["[ERROR] no", "Failed to communicate", "Not a Framework Laptop"] {
            let tool: Tool = Box::new(move |s| s.stub("framework_tool", "--kblight", 0, text));
            let (s, go) = start(&Lights { model: "Laptop 13", ..lights(vec![tool]) }, "LightsParity.test_tool_errors_in_a_zero_exit_are_failures");
            go(&s, OFF);
            assert_eq!(kblight_saved(&s), None, "{text}");
            go(&s, ON);
        }
    }

    #[test]
    fn test_a_failed_read_does_not_invent_a_restore_value() {
        let (s, go) = start(&lights(vec![failing("qmk_hid")]), "LightsParity.test_a_failed_read_does_not_invent_a_restore_value");
        go(&s, OFF);
        assert_eq!(kblight_saved(&s), None);
        go(&s, ON);
        assert_eq!(light_calls(&s), Vec::<String>::new());
    }

    #[test]
    fn test_power_button_goes_dark_and_comes_back_with_its_trigger() {
        let (s, go) = start(&Lights { led: AUTO_LED, ..lights(vec![qmk_keys(40)]) }, "LightsParity.test_power_button_goes_dark_and_comes_back_with_its_trigger");
        go(&s, OFF);
        assert_eq!((led_file(&s, "brightness"), kblight_saved(&s).unwrap()["fp"].clone()), ("0".to_string(), json!({"sys": true, "trigger": "chromeos-auto", "brightness": 80})));
        go(&s, ON);
        assert_eq!((led_file(&s, "brightness"), led_file(&s, "trigger")), ("0".to_string(), "chromeos-auto".to_string()));
    }

    #[test]
    fn test_power_button_brightness_falls_back_to_the_maximum_then_100() {
        for (brightness, max, want) in [("junk", "60", Some(60)), ("junk", "junk", Some(100)), ("200", "100", None), ("junk", "200", None)] {
            let (s, go) = start(&Lights { led: Some(("chromeos-auto", brightness, max)), ..lights(vec![qmk_keys(40)]) }, "LightsParity.test_power_button_brightness_falls_back_to_the_maximum_then_100");
            go(&s, OFF);
            assert_eq!(kblight_saved(&s).unwrap()["fp"].get("brightness"), want.map(|w: i32| json!(w)).as_ref(), "{brightness} {max}");
            go(&s, ON);
        }
    }

    /// A root-only LED goes through the installed helper, never a wheel-wide sudoers rule.
    #[test]
    fn test_power_button_uses_the_helper_when_sysfs_is_root_only() {
        let s = Sandbox::new();
        Lights { led: AUTO_LED, ..lights(vec![qmk_keys(40)]) }.setup(&s);
        crate::write::chmod(&s.root.join("sys/class/leds/chromeos:white:power/brightness"), 0o444);
        s.install_helper(false);
        s.stateful("sudo", "");
        let helper = || log_starting(&s, "sudo");
        s.rust(OFF);
        assert_eq!(helper(), [format!("sudo -n {}/ops-priv power-led chromeos:white:power brightness 0", s.helper_dir().display())]);
        s.rust(ON);
        assert_eq!(helper(), Vec::<String>::new());
    }

    #[test]
    fn test_laptop_16_with_only_framework_tool_and_no_led_invents_nothing() {
        let (s, go) = start(&lights(vec![failing("framework_tool")]), "LightsParity.test_laptop_16_with_only_framework_tool_and_no_led_invents_nothing");
        go(&s, OFF);
        assert_eq!(kblight_saved(&s), None);
        go(&s, ON);
        assert_eq!(light_calls(&s), Vec::<String>::new());
    }

    #[test]
    fn test_the_first_power_button_in_name_order_wins() {
        let setup = |s: &Sandbox| {
            lights(vec![qmk_keys(40)]).setup(s);
            for (name, value) in [("chromeos:white:power", 11), ("chromeos:amber:power", 22), ("chromeos:blue:power", 33)] {
                s.write(&format!("sys/class/leds/{name}/brightness"), &format!("{value}\n"));
            }
        };
        let (s, go) = start(&setup, "LightsParity.test_the_first_power_button_in_name_order_wins");
        go(&s, OFF);
        assert_eq!(kblight_saved(&s).unwrap()["fp"]["brightness"], 22);
    }

    #[test]
    fn test_no_tools_and_no_power_button_touch_nothing() {
        const T: &str = "LightsParity.test_no_tools_and_no_power_button_touch_nothing";
        let (s, go) = start(&lights(vec![]), T);
        go(&s, OFF);
        go(&s, ON);
        assert_eq!(light_calls(&s), Vec::<String>::new());
        let (s, go) = start(&Lights { model: "Laptop 13", ..lights(vec![failing("qmk_hid")]) }, T);
        go(&s, OFF);
        go(&s, ON);
        assert_eq!(light_calls(&s), Vec::<String>::new());
    }

    #[test]
    fn test_machines_that_are_not_a_framework_laptop_are_left_alone() {
        let saved = json!({"level": 40, "fp": {"requested": "auto", "brightness": 55}});
        for (vendor, model) in [("LENOVO", "Laptop 16"), ("Framework", "Desktop"), ("Framework", "Laptop 15")] {
            let saved_text = saved.to_string();
            let setup = move |s: &Sandbox| {
                Lights { vendor, model, led: AUTO_LED, ..lights(vec![qmk_keys(40), failing("framework_tool")]) }.setup(s);
                s.write(KBLIGHT_FILE, &saved_text);
            };
            let (s, go) = start(&setup, "LightsParity.test_machines_that_are_not_a_framework_laptop_are_left_alone");
            go(&s, OFF);
            go(&s, ON);
            assert_eq!((light_calls(&s), led_file(&s, "brightness"), kblight_saved(&s)), (vec![], "80".to_string(), Some(saved.clone())), "{vendor} {model}");
        }
    }

    #[test]
    fn test_vendor_matches_framework_in_any_case() {
        let (s, go) = start(&Lights { vendor: "FRAMEWORK", ..lights(vec![qmk_keys(67)]) }, "LightsParity.test_vendor_matches_framework_in_any_case");
        go(&s, OFF);
        assert_eq!(keys_level(&s), 0);
        go(&s, ON);
    }

    const RECEIVER: &str = "I: Bus=0003 Vendor=046d Product=0001 Version=0111\nN: Name=\"receiver\"\nH: Handlers=sysrq kbd leds event2\nB: KEY=1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe\n\n";

    fn auto_keys(lux: Option<u32>) -> Lights {
        Lights { auto: true, lux, ..lights(vec![qmk_keys(100)]) }
    }

    fn kbauto(s: &Sandbox) -> Option<Value> {
        let p = s.target("run/omarchy-power-hub/kbauto.json");
        p.exists().then(|| parsed(&fs::read_to_string(p).unwrap()))
    }

    #[test]
    fn test_auto_wake_picks_the_level_from_the_room() {
        for (lux, want) in [(Some(3355), 0), (Some(40), 67), (Some(5), 33), (None, 33)] {
            let (s, go) = start(&auto_keys(lux), "LightsParity.test_auto_wake_picks_the_level_from_the_room");
            go(&s, OFF);
            go(&s, ON);
            assert_eq!((keys_level(&s), kblight_saved(&s)), (want, None), "{lux:?}");
        }
    }

    #[test]
    fn test_auto_wake_stays_dark_with_another_keyboard() {
        let (s, go) = start(&Lights { devices: Some(RECEIVER), ..auto_keys(Some(5)) }, "LightsParity.test_auto_wake_stays_dark_with_another_keyboard");
        go(&s, OFF);
        go(&s, ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (0, None));
    }

    #[test]
    fn test_auto_tick_respects_fn_space_until_the_next_wake() {
        let (s, go) = start(&auto_keys(Some(5)), "LightsParity.test_auto_tick_respects_fn_space_until_the_next_wake");
        let tick = &["lights", "auto"];
        go(&s, tick);
        assert_eq!((keys_level(&s), kbauto(&s)), (33, Some(json!({"wrote": 33, "hold": false}))));
        fs::write(s.base.join("keys.state"), "100\n").unwrap();
        go(&s, tick);
        assert_eq!((keys_level(&s), kbauto(&s)), (100, Some(json!({"wrote": 33, "hold": true}))));
        go(&s, OFF);
        go(&s, ON);
        assert_eq!((keys_level(&s), kbauto(&s)), (33, Some(json!({"wrote": 33, "hold": false}))));
    }

    /// The panel's tick and the 30 s tick overlap: the second must not read the first's own write as Fn+Space.
    #[test]
    fn test_overlapping_auto_ticks_do_not_hold_the_level() {
        let s = Sandbox::new();
        auto_keys(Some(5)).setup(&s);
        let state = s.base.join("keys.state").display().to_string();
        s.stateful("qmk_hid", &format!(r#"case "$*" in
  "via --backlight") echo "Brightness: $(<{state})%"; /usr/bin/sleep 0.6;;
  "via --backlight "*) echo 33 > {state};;
  *) exit 1;;
esac"#));
        let mut first = s.rust_start(&["lights", "auto"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut second = s.rust_start(&["lights", "auto"]);
        first.wait().unwrap();
        second.wait().unwrap();
        assert_eq!((keys_level(&s), kbauto(&s)), (33, Some(json!({"wrote": 33, "hold": false}))));
    }

    #[test]
    fn test_auto_tick_writes_nothing_when_the_keyboard_does_not_answer() {
        let (s, go) = start(&Lights { auto: true, lux: Some(5), ..lights(vec![failing("qmk_hid")]) }, "LightsParity.test_auto_tick_writes_nothing_when_the_keyboard_does_not_answer");
        go(&s, &["lights", "auto"]);
        assert_eq!((light_calls(&s).iter().any(|c| c.contains("--backlight ")), kbauto(&s)), (false, None));
    }

    #[test]
    fn test_auto_tick_records_nothing_when_the_write_fails() {
        let reads_only: Tool = Box::new(|s| s.stateful("qmk_hid", r#"[ "$*" = "via --backlight" ] && echo "Brightness: 0%" || exit 1"#));
        let (s, go) = start(&Lights { auto: true, lux: Some(5), ..lights(vec![reads_only]) }, "LightsParity.test_auto_tick_records_nothing_when_the_write_fails");
        go(&s, &["lights", "auto"]);
        assert_eq!(kbauto(&s), None);
    }

    #[test]
    fn test_auto_tick_does_nothing_in_manual_mode_or_with_displays_off() {
        const T: &str = "LightsParity.test_auto_tick_does_nothing_in_manual_mode_or_with_displays_off";
        let (s, go) = start(&Lights { lux: Some(5), ..lights(vec![qmk_keys(100)]) }, T);
        go(&s, &["lights", "auto"]);
        assert_eq!((keys_level(&s), kbauto(&s)), (100, None));
        let (s, go) = start(&auto_keys(Some(5)), T);
        go(&s, OFF);
        go(&s, &["lights", "auto"]);
        assert_eq!((keys_level(&s), kbauto(&s)), (0, None));
    }

    const SLEEP_OFF: &[&str] = &["lights", "off"];
    const SLEEP_ON: &[&str] = &["lights", "on"];
    const DISABLED: &str = r#"{"disabledPlugins": ["omarchy-power-hub"]}"#;

    #[test]
    fn test_sleep_darkens_and_restores_the_lights() {
        let (s, go) = start(&lights(vec![qmk_keys(67)]), "LightsParity.test_sleep_darkens_and_restores_the_lights");
        go(&s, SLEEP_OFF);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (0, Some(json!({"level": 67, "sleep": true}))));
        go(&s, SLEEP_ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (67, None));
    }

    #[test]
    fn test_sleep_after_displays_off_leaves_the_saved_levels() {
        let (s, go) = start(&lights(vec![qmk_keys(67)]), "LightsParity.test_sleep_after_displays_off_leaves_the_saved_levels");
        go(&s, OFF);
        go(&s, SLEEP_OFF);
        go(&s, SLEEP_ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (0, Some(json!({"level": 67}))));
        go(&s, ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (67, None));
    }

    #[test]
    fn test_sleep_does_nothing_when_the_plugin_is_disabled() {
        let (s, go) = start(&lights(vec![qmk_keys(67)]), "LightsParity.test_sleep_does_nothing_when_the_plugin_is_disabled");
        s.write(SHELL_FILE, DISABLED);
        go(&s, SLEEP_OFF);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (67, None));
    }

    #[test]
    fn test_disabling_between_pre_and_post_still_restores() {
        let (s, go) = start(&lights(vec![qmk_keys(67)]), "LightsParity.test_disabling_between_pre_and_post_still_restores");
        go(&s, SLEEP_OFF);
        s.write(SHELL_FILE, DISABLED);
        go(&s, SLEEP_ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (67, None));
    }

    #[test]
    fn test_a_wake_with_the_lid_closed_keeps_the_lights_dark() {
        let (s, go) = start(&lights(vec![qmk_keys(67)]), "LightsParity.test_a_wake_with_the_lid_closed_keeps_the_lights_dark");
        go(&s, SLEEP_OFF);
        s.lid("closed");
        go(&s, SLEEP_ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (0, Some(json!({"level": 67, "sleep": true}))));
        s.lid("open");
        go(&s, SLEEP_ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (67, None));
    }

    #[test]
    fn test_sleep_wake_applies_auto() {
        let (s, go) = start(&auto_keys(Some(5)), "LightsParity.test_sleep_wake_applies_auto");
        go(&s, SLEEP_OFF);
        go(&s, SLEEP_ON);
        assert_eq!((keys_level(&s), kblight_saved(&s)), (33, None));
    }

    /// A second off would read the darkened lights and lose the levels to restore.
    #[test]
    fn test_a_second_off_keeps_the_saved_light_levels() {
        let saved = Some(json!({"level": 67, "fp": {"sys": true, "trigger": "chromeos-auto", "brightness": 80}}));
        let (s, go) = start(&Lights { led: AUTO_LED, ..lights(vec![qmk_keys(67)]) }, "LightsParity.test_a_second_off_keeps_the_saved_light_levels");
        go(&s, OFF);
        assert_eq!(kblight_saved(&s), saved);
        go(&s, OFF);
        assert_eq!(kblight_saved(&s), saved);
        go(&s, ON);
        assert_eq!((keys_level(&s), led_file(&s, "brightness"), led_file(&s, "trigger")), (67, "0".to_string(), "chromeos-auto".to_string()));
    }

    #[test]
    fn test_a_wake_without_saved_levels_does_nothing() {
        let (s, go) = start(&Lights { led: AUTO_LED, ..lights(vec![qmk_keys(67)]) }, "LightsParity.test_a_wake_without_saved_levels_does_nothing");
        go(&s, ON);
        assert_eq!((light_calls(&s), led_file(&s, "brightness")), (vec![], "80".to_string()));
    }

    #[test]
    fn test_saved_files_of_any_shape_wake_the_lights_or_nothing() {
        let texts = ["", "nope", "[]", "5", "\"x\"", "null", r#"{"level": 150}"#, r#"{"level": 40.0}"#, r#"{"level": true}"#, r#"{"fp": 3}"#,
                     r#"{"level": 40, "fp": {"requested": 5, "trigger": 5, "brightness": 200, "sys": "yes"}}"#,
                     r#"{"level": 40, "held": 1, "fp": {"requested": "custom", "brightness": 7}}"#];
        for text in texts {
            let setup = move |s: &Sandbox| {
                Lights { led: AUTO_LED, ..lights(vec![qmk_keys(0)]) }.setup(s);
                s.write(KBLIGHT_FILE, text);
            };
            let (s, go) = start(&setup, "LightsParity.test_saved_files_of_any_shape_wake_the_lights_or_nothing");
            go(&s, ON);
        }
    }
}

const LID_BEGIN: &str = "-- BEGIN Omarchy Power Hub lid action override";
const LID_END: &str = "-- END Omarchy Power Hub lid action override";
/// The goldens show the old `pkexec` wakeup; the lid verbs never call it, so replays drop it and the tests below pin that.
const LID_DROP: &[&str] = &["hyprctl", "busctl get-property net.hadess", "omarchy-powerprofiles-list", "pkexec"];
const LID: Replay = Replay { drop: LID_DROP, ..BOTH };
const LID_REQ: Replay = Replay { requests: true, ..LID };
const SEEN_INPUTS: [&str; 3] = ["m1", "pixa3854:00-093a:0274-mouse", "kb"];

fn device_eval(name: &str, enabled: &str) -> String {
    format!("hl.device({{ name = \"{name}\", enabled = {enabled} }})")
}

fn lid_setup(action: &'static str, shown: Displays) -> impl Fn(&Sandbox) {
    move |s| {
        shown.setup(s);
        let side = json!({"profile": "balanced", "lid": action});
        s.config(json!({"ac": side, "battery": side}));
        s.devices(&[json!("m1"), json!("pixa3854:00-093a:0274-mouse"), json!("bad\"; os.execute(\"x\")")], &["kb", "power-button", "gpio-keys", "video-bus"].map(|k| json!(k)));
        // One ordered log: each Hyprland eval lands beside the systemctl calls.
        let log = s.calls_log.clone();
        s.hypr().on_eval = Some(Box::new(move |_, lua| {
            writeln!(OpenOptions::new().append(true).create(true).open(&log).unwrap(), "EVAL {lua}").unwrap();
        }));
    }
}

fn default_lid(action: &'static str) -> impl Fn(&Sandbox) {
    lid_setup(action, displays(vec![lit("eDP-1")]))
}

fn input_evals(r: &Outcome, enabled: &str) -> bool {
    r.evals.iter().filter(|e| e.starts_with("hl.device")).cloned().collect::<Vec<_>>() == SEEN_INPUTS.map(|n| device_eval(n, enabled))
}

/// The hid wakeup call, then every input, then whatever the lid does.
fn inputs_come_first(s: &Sandbox, r: &Outcome, enabled: &str) {
    assert!(input_evals(r, enabled));
    let last = r.requests.iter().rposition(|q| q.starts_with("eval hl.device")).unwrap();
    let later: Vec<usize> = r.requests.iter().enumerate().filter(|(_, q)| !q.starts_with("eval hl.device") && *q != "j/devices").map(|(i, _)| i).collect();
    assert!(later.iter().all(|i| *i > last), "{:?}", r.requests);
    let log = log_lines(s);
    let inputs: Vec<usize> = log.iter().enumerate().filter(|(_, c)| c.starts_with("EVAL hl.device")).map(|(i, _)| i).collect();
    let after: Vec<usize> = log.iter().enumerate().filter(|(_, c)| c.starts_with("systemctl")).map(|(i, _)| i).collect();
    assert!(!inputs.is_empty() && after.iter().all(|i| *i > *inputs.last().unwrap()), "{log:?}");
    assert!(log_starting(s, "pkexec").is_empty(), "{log:?}");
}

mod lid_parity {
    use super::*;

    #[test]
    fn test_close_runs_the_action_after_the_inputs_are_off() {
        let wanted: [(&'static str, &[&str]); 4] = [("nothing", &[]), ("display-off", &[]), ("suspend", &["systemctl suspend"]),
                                                    ("hibernate", &["systemctl hibernate", "systemctl suspend"])];
        for (action, systemctl) in wanted {
            let (_, r) = checked_each("LidParity.test_close_runs_the_action_after_the_inputs_are_off", &default_lid(action), &[&["lid-closed"]], LID_REQ, &mut |s, r| {
                inputs_come_first(s, r, "false");
                assert_eq!(log_starting(s, "systemctl"), systemctl, "{action}");
            });
            assert_eq!(r.evals[..3], SEEN_INPUTS.map(|n| device_eval(n, "false")), "{action}");
        }
    }

    #[test]
    fn test_no_verb_asks_for_a_password_and_a_current_helper_is_asked_once_first() {
        for (verb, enabled, word) in [("lid-closed", "false", "off"), ("lid-opened", "true", "on")] {
            let s = Sandbox::new();
            default_lid("nothing")(&s);
            s.rust(&[verb]);
            assert!(log_starting(&s, "pkexec").is_empty() && log_starting(&s, "sudo").is_empty(), "{verb} without a helper: {:?}", log_lines(&s));
            let s = Sandbox::new();
            default_lid("nothing")(&s);
            s.install_helper(false);
            let r = s.rust(&[verb]);
            let log = log_lines(&s);
            let call = format!("sudo -n {}/ops-priv hid-wakeup {word}", s.helper_dir().display());
            assert_eq!(log.iter().filter(|c| c.starts_with("sudo")).collect::<Vec<_>>(), [&call], "{verb}");
            assert!(log_starting(&s, "pkexec").is_empty(), "{verb}");
            assert!(log.iter().position(|c| *c == call) < log.iter().position(|c| c.starts_with("EVAL hl.device")), "{verb}: {log:?}");
            assert!(input_evals(&r, enabled), "{verb}");
        }
    }

    #[test]
    fn test_close_exits_0_and_prints_nothing() {
        for action in ["nothing", "display-off", "suspend", "hibernate"] {
            let s = Sandbox::new();
            default_lid(action)(&s);
            let r = s.rust(&["lid-closed"]);
            assert_eq!((r.code, r.stdout.as_str()), (0, ""), "{action}");
        }
    }

    #[test]
    fn test_a_working_hibernate_does_not_fall_back_to_suspend() {
        let setup = |s: &Sandbox| {
            default_lid("hibernate")(s);
            s.stub("systemctl", "hibernate", 0, "");
        };
        checked_each("LidParity.test_a_working_hibernate_does_not_fall_back_to_suspend", &setup, &[&["lid-closed"]], LID, &mut |s, _| {
            assert_eq!(log_starting(s, "systemctl"), ["systemctl hibernate"]);
        });
    }

    #[test]
    fn test_close_with_display_off_sends_dpms_only_and_disables_no_head() {
        let s = Sandbox::new();
        lid_setup("display-off", displays(vec![lit("eDP-1"), described("DP-1", FLIP)]))(&s);
        let r = s.rust(&["lid-closed"]);
        assert_eq!(dispatched(&r), dpms("disable", &["eDP-1", "DP-1"]));
        assert!(r.evals.iter().all(|e| !e.contains("disabled")));
    }

    #[test]
    fn test_close_with_display_off_darkens_the_lights_and_open_brings_them_back() {
        let setup = |s: &Sandbox| {
            lights(vec![qmk_keys(67)]).setup(s);
            default_lid("display-off")(s);
        };
        const T: &str = "LidParity.test_close_with_display_off_darkens_the_lights_and_open_brings_them_back";
        // Opening onto lit displays still restores the lights.
        for (i, monitors) in [lit("eDP-1"), dark("eDP-1")].into_iter().enumerate() {
            let s = Sandbox::new();
            setup(&s);
            if i == 0 { replay(&s, T, &["lid-closed"], LID); } else { s.rust(&["lid-closed"]); }
            assert_eq!((keys_level(&s), kblight_saved(&s)), (0, Some(json!({"level": 67}))));
            s.monitors(&[monitors]);
            s.rust(&["lid-opened"]);
            assert_eq!((keys_level(&s), kblight_saved(&s)), (67, None));
        }
    }

    #[test]
    fn test_open_wakes_dark_heads_after_the_inputs() {
        let s = Sandbox::new();
        lid_setup("display-off", displays(vec![dark("eDP-1")]))(&s);
        let r = s.rust(&["lid-opened"]);
        inputs_come_first(&s, &r, "true");
        assert_eq!(dispatched(&r), dpms("enable", &["eDP-1"]));
    }

    #[test]
    fn test_open_with_lit_heads_only_enables_the_inputs() {
        let s = Sandbox::new();
        lid_setup("display-off", displays(vec![lit("eDP-1")]))(&s);
        let r = s.rust(&["lid-opened"]);
        inputs_come_first(&s, &r, "true");
        assert!(r.requests.iter().all(|q| !q.starts_with("dispatch") && !q.starts_with("eval hl.monitor")));
        let logged: Vec<String> = log_lines(&s).into_iter().filter(|c| !c.is_empty() && !c.starts_with("EVAL")).collect();
        assert_eq!(logged, Vec::<String>::new());
    }

    #[test]
    fn test_open_exits_0_and_prints_nothing() {
        let s = Sandbox::new();
        lid_setup("display-off", displays(vec![dark("eDP-1")]))(&s);
        let r = s.rust(&["lid-opened"]);
        assert_eq!((r.code, r.stdout.as_str()), (0, ""));
    }

    /// The binary exits 0 and leaves the displays alone; the recorded old run died on the lock with a traceback.
    #[test]
    fn test_a_lock_that_cannot_be_taken_still_turns_the_inputs_off_and_on() {
        let s = Sandbox::new();
        lid_setup("display-off", displays(vec![dark("eDP-1")]))(&s);
        fs::create_dir_all(s.target(LOCK_FILE)).unwrap();
        for (verb, enabled) in [("lid-closed", "false"), ("lid-opened", "true")] {
            let r = s.rust(&[verb]);
            let other: Vec<&String> = r.evals.iter().filter(|e| !e.starts_with("hl.device")).collect();
            assert_eq!((r.code, r.stdout.as_str(), other.len()), (0, "", 0), "{verb}");
            assert!(input_evals(&r, enabled), "{verb}");
            assert!(r.requests.iter().all(|q| !q.starts_with("dispatch")), "{verb}");
        }
    }

    #[test]
    fn test_bound_verbs_survive_a_closed_stdout() {
        for verb in ["lid-closed", "lid-opened", "idle-lock", "saver-past-inhibitor", "recover-ucsi"] {
            let s = Sandbox::new();
            default_lid("nothing")(&s);
            let mut command = Command::new(RUST_BIN);
            command.arg(verb).env_clear().envs(s.env()).current_dir(&s.base);
            let p = spawn_with(&mut command, Stdio::null()).wait_with_output().unwrap();
            assert_eq!((p.status.code(), String::from_utf8_lossy(&p.stderr).as_ref()), (Some(0), ""), "{verb}");
        }
    }
}

mod idle_command_parity {
    use super::*;

    const STAY_AWAKE: &str = "home/.local/state/omarchy/indicators/stay-awake";

    #[test]
    fn test_idle_lock_locks_unless_stay_awake_is_on() {
        const T: &str = "IdleCommandParity.test_idle_lock_locks_unless_stay_awake_is_on";
        checked_each(T, &|_: &Sandbox| {}, &[&["idle-lock"]], BOTH, &mut |s, _| {
            assert_eq!(log_starting(s, "omarchy-system-lock"), ["omarchy-system-lock "]);
        });
        checked_each(T, &|s: &Sandbox| { s.write(STAY_AWAKE, ""); }, &[&["idle-lock"]], BOTH, &mut |s, _| {
            assert_eq!(fs::read_to_string(&s.calls_log).unwrap(), "");
        });
    }

    /// Omarchy's own countdown locks: the plugin killing the saver mid-cycle would read as the user returning.
    #[test]
    fn test_idle_lock_leaves_the_lock_to_omarchys_running_countdown() {
        let s = Sandbox::new();
        s.stub("omarchy-shell", "idle status", 0, r#"{"timers":{"lock":true}}"#);
        s.rust(&["idle-lock"]);
        assert_eq!(log_starting(&s, "omarchy-system-lock"), Vec::<String>::new());
    }

    #[test]
    fn test_saver_past_inhibitor() {
        let inhibiting = r#"[{"inhibitingIdle": false}, {"inhibitingIdle": true}]"#;
        let locked_check = &["omarchy-shell lock isLocked"][..];
        let cases: Vec<(&str, &str, &str, bool, &[&str])> = vec![
            ("inhibiting client", inhibiting, "false\n", false, &["omarchy-shell lock isLocked", "omarchy-launch-screensaver "]),
            ("no client inhibits", r#"[{"inhibitingIdle": false}, {}]"#, "false\n", false, &[]),
            ("no clients", "[]", "false\n", false, &[]),
            ("empty reply", "", "false\n", false, &[]),
            ("bad reply", "nope", "false\n", false, &[]),
            ("stay awake", inhibiting, "false\n", true, &[]),
            ("already locked", inhibiting, "true\n", false, locked_check),
            ("locked, padded", inhibiting, " true \n", false, locked_check),
        ];
        for (label, clients, locked, awake, wanted) in cases {
            let setup = move |s: &Sandbox| {
                s.reply("j/clients", clients);
                s.stub("omarchy-shell", "isLocked", 0, locked);
                if awake {
                    s.write(STAY_AWAKE, "");
                }
            };
            checked_each("IdleCommandParity.test_saver_past_inhibitor", &setup, &[&["saver-past-inhibitor"]], BOTH, &mut |s, _| {
                assert_eq!(log_starting(s, "omarchy"), wanted, "{label}");
            });
        }
    }
}

mod recovery_parity {
    use super::*;

    const MARK: &str = "run/omarchy-power-hub/ucsi-rebound";

    struct Recovery {
        framework: bool,
        helper: bool,
        ucsi: Option<(bool, f64, f64)>,
        marked: bool,
    }

    fn recovery() -> Recovery {
        Recovery { framework: true, helper: true, ucsi: None, marked: false }
    }

    impl Setup for Recovery {
        fn setup(&self, s: &Sandbox) {
            if self.framework {
                s.framework(true);
            }
            if self.helper {
                s.install_helper(false);
            }
            if let Some((online, volts, amps)) = self.ucsi {
                s.ucsi("USBC000:001", online, volts, amps);
            }
            if self.marked {
                s.write(MARK, "");
            }
        }
    }

    #[test]
    fn test_a_missing_driver_is_rebound_once_through_the_helper() {
        let s = Sandbox::new();
        recovery().setup(&s);
        let go = || {
            replay(&s, "RecoveryParity.test_a_missing_driver_is_rebound_once_through_the_helper", &["recover-ucsi"], BOTH);
        };
        go();
        let rebind = format!("sudo -n {}/ops-priv ucsi-rebind", s.helper_dir().display());
        assert_eq!((fs::read_to_string(&s.calls_log).unwrap().trim(), s.target(MARK).exists()), (rebind.as_str(), true));
        go();
        assert_eq!(fs::read_to_string(&s.calls_log).unwrap(), "");
    }

    #[test]
    fn test_the_run_dir_is_made_before_the_mark() {
        let s = Sandbox::new();
        recovery().setup(&s);
        assert!(!s.target("run/omarchy-power-hub").exists());
        let r = s.rust(&["recover-ucsi"]);
        assert_eq!((r.code, r.stdout.as_str(), read(&s, MARK).as_str()), (0, "", ""));
    }

    #[test]
    fn test_nothing_to_recover() {
        let cases = [("online", Recovery { ucsi: Some((true, 20.0, 3.0)), ..recovery() }), ("offline port", Recovery { ucsi: Some((false, 0.0, 0.0)), ..recovery() }),
                     ("marked", Recovery { marked: true, ..recovery() }), ("not a Framework", Recovery { framework: false, ..recovery() }),
                     ("no helper", Recovery { helper: false, ..recovery() })];
        for (label, setup) in cases {
            checked_each("RecoveryParity.test_nothing_to_recover", &setup, &[&["recover-ucsi"]], BOTH, &mut |s, _| {
                assert_eq!(fs::read_to_string(&s.calls_log).unwrap(), "", "{label}");
                assert_eq!(s.target(MARK).exists(), setup.marked, "{label}");
            });
        }
    }

    #[test]
    fn test_a_helper_that_appears_later_is_used_and_marks_then() {
        let s = Sandbox::new();
        Recovery { helper: false, ..recovery() }.setup(&s);
        let go = || {
            replay(&s, "RecoveryParity.test_a_helper_that_appears_later_is_used_and_marks_then", &["recover-ucsi"], BOTH);
        };
        go();
        assert!(!s.target(MARK).exists());
        s.install_helper(false);
        go();
        assert!(s.target(MARK).exists());
    }
}

const SHELL_FILE: &str = "home/.config/omarchy/shell.json";
const UPDATE_UNIT: &str = "[Unit]\nDescription=Omarchy Power Hub device tweaks\n\n[Service]\nType=oneshot\nExecStart=/usr/bin/python3 {bin}/ops.py apply-tweaks\n\n[Install]\nWantedBy=default.target\n";
const BINDINGS_FILE: &str = "home/.config/hypr/bindings.lua";
const UNIT_FILE: &str = "home/.config/systemd/user/omarchy-power-hub-tweaks.service";

mod update_from_python {
    use super::*;

    /// A machine whose bindings and tweaks unit still name ops.py.
    fn old_install(with_unit: bool) -> impl Fn(&Sandbox) {
        move |s| {
            ApplyEnv::default().setup(s);
            let bin = s.bin.display();
            let old = format!("{LID_BEGIN}\nhl.unbind(\"switch:on:Lid Switch\")\nhl.unbind(\"switch:off:Lid Switch\")\n\
                               o.bind(\"switch:on:Lid Switch\", nil, \"python3 {bin}/ops.py lid-closed\", {{ locked = true }})\n\
                               o.bind(\"switch:off:Lid Switch\", nil, \"python3 {bin}/ops.py lid-opened\", {{ locked = true }})\n{LID_END}\n");
            s.write(BINDINGS_FILE, &format!("bind = x\n{old}"));
            if with_unit {
                s.write(UNIT_FILE, &UPDATE_UNIT.replace("{bin}", &bin.to_string()));
            }
        }
    }

    #[test]
    fn test_one_block_one_reload_and_a_unit_that_names_the_binary() {
        let s = Sandbox::new();
        old_install(true)(&s);
        let r = s.rust(&["apply"]);
        let bindings = read(&s, BINDINGS_FILE);
        let unit = read(&s, UNIT_FILE);
        assert_eq!((r.code, r.stdout.as_str()), (0, "ok\n"));
        assert_eq!(bindings.matches(LID_BEGIN).count(), 1);
        assert!(!bindings.contains("python3"));
        assert!(bindings.contains(&format!("{}/power-hub lid-closed", s.bin.display())));
        assert!(bindings.contains(&format!("{}/power-hub lid-opened", s.bin.display())));
        assert_eq!(r.requests.iter().filter(|q| *q == "reload").count(), 1);
        assert!(unit.contains("power-hub apply-tweaks"));
        assert!(!unit.contains("python3"));
        assert!(has_call(&r, "systemctl --user daemon-reload"));
        assert!(!called(&r, "enable"));
        let again = s.rust(&["apply"]);
        assert_eq!((again.requests.iter().filter(|q| *q == "reload").count(), read(&s, UNIT_FILE)), (0, unit));
        assert!(!has_call(&again, "systemctl --user daemon-reload"));
    }

    #[test]
    fn test_a_failed_apply_still_rewrites_a_lid_block_that_names_python() {
        for verb in ["apply", "apply-profile"] {
            let s = Sandbox::new();
            old_install(true)(&s);
            chmod(&s.target(SHELL_FILE), 0o444);
            let r = s.rust(&[verb]);
            let bindings = read(&s, BINDINGS_FILE);
            assert_eq!(r.code, 1, "{verb}");
            assert!(!bindings.contains("python3"), "{verb}");
            assert_eq!(bindings.matches(LID_BEGIN).count(), 1, "{verb}");
            assert!(bindings.contains(&format!("{}/power-hub lid-closed", s.bin.display())), "{verb}");
            assert!(bindings.contains(&format!("{}/power-hub lid-opened", s.bin.display())), "{verb}");
        }
    }

    #[test]
    fn test_no_unit_means_none_is_made() {
        let s = Sandbox::new();
        old_install(false)(&s);
        let r = s.rust(&["apply"]);
        assert!(!s.target(UNIT_FILE).exists());
        assert!(starting(&r, "systemctl").is_empty());
    }

    #[test]
    fn test_a_failed_apply_still_refreshes_the_unit() {
        let s = Sandbox::new();
        old_install(true)(&s);
        let bindings = s.target(BINDINGS_FILE);
        fs::remove_file(&bindings).unwrap();
        fs::create_dir(&bindings).unwrap();
        let r = s.rust(&["apply"]);
        assert_eq!(r.code, 1);
        assert!(read(&s, UNIT_FILE).contains("power-hub apply-tweaks"));
        assert!(has_call(&r, "systemctl --user daemon-reload"));
    }
}
