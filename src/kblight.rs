//! Keyboard and power-button lights. Laptop 16 keys are USB; 12 and 13 use the EC.
use crate::{charge, exec, paths};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DAYLIGHT_LUX: f64 = 100.0;
pub const FP_LEVELS: [&str; 5] = ["auto", "high", "medium", "low", "ultra-low"];
type Argv = Vec<String>;

fn argv(parts: &[&str]) -> Argv {
    parts.iter().map(|s| s.to_string()).collect()
}

fn run(args: &[String]) -> (i32, String) {
    exec::run(&args.iter().map(String::as_str).collect::<Vec<_>>(), Duration::from_secs(4))
}

fn sudo(args: &[String]) -> Argv {
    [argv(&["sudo", "-n"]), args.to_vec()].concat()
}

/// The exit code and message a tool must not show for its call to count.
fn tool_ok(code: i32, out: &str) -> bool {
    code == 0 && !["[ERROR]", "Failed to communicate", "Not a Framework Laptop"].iter().any(|bad| out.contains(bad))
}

/// The tool as the user, then through `sudo -n`, then through the privileged helper.
fn call(tool: &[String], helper: Option<Argv>) -> (i32, String) {
    let (code, out) = run(tool);
    if tool_ok(code, &out) {
        return (0, out);
    }
    let (code, out) = run(&sudo(tool));
    match helper {
        Some(h) if !tool_ok(code, &out) => run(&sudo(&h)),
        _ => (code, out),
    }
}

/// `framework_tool --kblight 0` becomes `ops-priv kblight 0`.
fn priv_argv(tool: &[String], helper: Option<&Path>) -> Option<Argv> {
    let verb = tool.get(1)?.strip_prefix("--")?;
    Some([vec![helper?.display().to_string(), verb.into()], tool[2..].to_vec()].concat())
}

/// The first `<prefix>:\s*(\d{1,3})\s*%` the regex search would find.
fn level_after(text: &str, prefixes: &[&str]) -> Option<i64> {
    text.char_indices().find_map(|(i, _)| {
        let after = prefixes.iter().find_map(|p| text[i..].strip_prefix(p))?.trim_start();
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        (1..=3).contains(&digits).then_some(())?;
        after[digits..].trim_start().starts_with('%').then(|| after[..digits].parse().ok())?
    })
}

fn in_range(n: i64) -> Option<i64> {
    (0..=100).contains(&n).then_some(n)
}

fn percent(text: &str) -> Option<i64> {
    level_after(text, &["Brightness:", "Keyboard backlight:"]).and_then(in_range)
}

fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
}

/// What the fingerprint LED (or the power button) was doing: `sys` marks the sysfs LED.
#[derive(Default, Debug, PartialEq)]
struct Fp {
    sys: bool,
    requested: Option<String>,
    trigger: Option<String>,
    brightness: Option<i64>,
}

impl Fp {
    fn to_json(&self) -> Value {
        let mut m = Map::new();
        if self.sys {
            m.insert("sys".into(), true.into());
        }
        if let Some(r) = &self.requested {
            m.insert("requested".into(), r.as_str().into());
        }
        if let Some(t) = &self.trigger {
            m.insert("trigger".into(), t.as_str().into());
        }
        if let Some(b) = self.brightness {
            m.insert("brightness".into(), b.into());
        }
        Value::Object(m)
    }

    fn is_empty(&self) -> bool {
        *self == Fp::default()
    }
}

fn requested_ok(s: &str) -> bool {
    FP_LEVELS.contains(&s) || s == "custom"
}

/// `framework_tool --fp-brightness` output as a state, or None when it names neither a level nor a brightness.
fn fp_state(text: &str) -> Option<Fp> {
    let requested = text.find("Requested:").map(|i| {
        let line = text[i + "Requested:".len()..].trim_start().split('\n').next().unwrap_or_default();
        line.split_whitespace().collect::<Vec<_>>().join("-").to_lowercase()
    });
    let fp = Fp {
        requested: requested.filter(|r| requested_ok(r)),
        brightness: level_after(text, &["Brightness:"]).and_then(in_range),
        ..Fp::default()
    };
    (!fp.is_empty()).then_some(fp)
}

/// The bracketed name in a `trigger` file, e.g. `none [chromeos-auto] timer`.
fn trigger(text: &str) -> Option<String> {
    let name = text.match_indices('[').find_map(|(i, _)| {
        let rest = &text[i + 1..];
        let end = rest.find(']')?;
        (end > 0).then(|| &rest[..end])
    })?;
    name_ok(name).then(|| name.to_string())
}

#[derive(Default, Debug, PartialEq)]
struct Saved {
    held: bool,
    level: Option<i64>,
    fp: Option<Fp>,
}

impl Saved {
    fn is_empty(&self) -> bool {
        *self == Saved::default()
    }
}

/// `kblight.json`, hardened: anything that is not an object of the expected fields loads as empty.
fn load_state(path: &Path) -> Saved {
    let Some(Value::Object(raw)) = fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()) else {
        return Saved::default();
    };
    let level = |v: Option<&Value>| v.and_then(Value::as_i64).and_then(in_range);
    let fp = raw.get("fp").and_then(Value::as_object).map(|fp| Fp {
        sys: fp.get("sys") == Some(&Value::Bool(true)),
        requested: fp.get("requested").and_then(Value::as_str).filter(|r| requested_ok(r)).map(String::from),
        trigger: fp.get("trigger").and_then(Value::as_str).filter(|t| name_ok(t)).map(String::from),
        brightness: level(fp.get("brightness")),
    });
    Saved { held: raw.get("held") == Some(&Value::Bool(true)), level: level(raw.get("level")), fp: fp.filter(|f| !f.is_empty()) }
}

fn dmi(name: &str) -> String {
    fs::read_to_string(paths::root("/sys/class/dmi/id").join(name)).unwrap_or_default()
}

/// `16` for the Laptop 16 (USB keys), `ec` for the 12 and 13, empty for anything else.
fn model_of(vendor: &str, product: &str) -> &'static str {
    if !vendor.to_lowercase().contains("framework") {
        return "";
    }
    let product = product.to_lowercase();
    let mut words = product.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty());
    if words.clone().any(|w| w == "16") {
        "16"
    } else if words.any(|w| matches!(w, "12" | "13")) {
        "ec"
    } else {
        ""
    }
}

/// Directory entries of `dir` whose name `keep` accepts, sorted by name.
fn sorted_entries(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| keep(n))
        .collect();
    names.sort();
    names.into_iter().map(|n| dir.join(n)).collect()
}

fn power_led() -> Option<PathBuf> {
    let leds = sorted_entries(&paths::root("/sys/class/leds"), |n| n.len() >= "chromeos:power".len() && n.starts_with("chromeos") && n.ends_with(":power"));
    leds.into_iter().find(|p| p.join("brightness").exists())
}

/// Ambient light, or None when no sensor can be read.
fn lux() -> Option<f64> {
    let read = |p: PathBuf| fs::read_to_string(p).ok()?.trim().parse::<f64>().ok();
    sorted_entries(&paths::root("/sys/bus/iio/devices"), |n| n.starts_with("iio:device")).into_iter().find_map(|dev| {
        let value = read(dev.join("in_illuminance_raw"))?;
        Some(read(dev.join("in_illuminance_scale")).map_or(value, |scale| value * scale))
    })
}

fn put(path: &Path, value: &str) -> bool {
    fs::write(path, value).is_ok() || exec::run_input(&["sudo", "-n", "tee", &path.display().to_string()], value, Duration::from_secs(4)).0 == 0
}

fn read_int(path: &Path) -> Option<i64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn blank_power(led: &Path) -> Option<Fp> {
    let brightness = match read_int(&led.join("brightness")) {
        Some(n) => in_range(n),
        None => match read_int(&led.join("max_brightness")) {
            Some(n) => in_range(n),
            None => Some(100),
        },
    };
    let fp = Fp { sys: true, trigger: fs::read_to_string(led.join("trigger")).ok().and_then(|t| trigger(&t)), brightness, ..Fp::default() };
    put(&led.join("brightness"), "0").then_some(fp)
}

fn wake_power(led: &Path, fp: &Fp) {
    if let Some(name) = &fp.trigger {
        put(&led.join("trigger"), name);
    }
    // Writing a brightness takes the LED off its trigger, so only a manual LED gets one.
    if let (Some(n), None | Some("none")) = (fp.brightness, fp.trigger.as_deref()) {
        put(&led.join("brightness"), &n.to_string());
    }
}

fn fp_restore(fp: &Fp) -> Option<Argv> {
    match (&fp.requested, fp.brightness) {
        (Some(r), _) if FP_LEVELS.contains(&r.as_str()) => Some(argv(&["framework_tool", "--fp-led-level", r])),
        (_, Some(b)) => Some(argv(&["framework_tool", "--fp-brightness", &b.to_string()])),
        _ => None,
    }
}

fn save(path: &Path, state: &Map<String, Value>) {
    let _ = path.parent().map(fs::create_dir_all);
    let _ = fs::write(path, Value::Object(state.clone()).to_string() + "\n");
}

/// Whether this machine is a Framework whose power button light `framework_tool` can set.
pub fn power_button_available() -> bool {
    !model_of(&dmi("sys_vendor"), &dmi("product_name")).is_empty() && exec::on_path("framework_tool")
}

/// Set the power button light to a named level, or the error to show.
pub fn set_power_button(level: &str) -> String {
    if !FP_LEVELS.contains(&level) || !power_button_available() {
        return "The power button level cannot be set on this machine".into();
    }
    let helper = charge::helper_bin().or_else(|| Some(paths::ops_priv()).filter(|p| p.is_file()));
    let args = argv(&["framework_tool", "--fp-led-level", level]);
    let (code, out) = call(&args, priv_argv(&args, helper.as_deref()));
    if !tool_ok(code, &out) {
        return out.trim().to_string();
    }
    // The EC only shows a new level when it takes the LED back, so hand it over once.
    if let Some(led) = power_led().filter(|l| fs::read_to_string(l.join("trigger")).ok().and_then(|t| trigger(&t)).as_deref() == Some("chromeos-auto")) {
        put(&led.join("trigger"), "none");
        put(&led.join("trigger"), "chromeos-auto");
    }
    String::new()
}

/// Dark lights for `off`, or the saved ones back for a wake; `daylight` keeps the keys off in a bright room.
pub fn sync(off: bool, daylight: bool) {
    let path = paths::plugin_run_dir().join("kblight.json");
    let helper = charge::helper_bin().or_else(|| Some(paths::ops_priv()).filter(|p| p.is_file()));
    let kind = model_of(&dmi("sys_vendor"), &dmi("product_name"));
    if kind.is_empty() {
        return;
    }
    let fw = exec::on_path("framework_tool");
    let qmk = kind == "16" && exec::on_path("qmk_hid");
    let ec_keys = kind == "ec" && fw;
    let led = power_led();
    if !qmk && !ec_keys && !fw && led.is_none() {
        return;
    }
    let keys = if qmk { argv(&["qmk_hid", "via", "--backlight"]) } else { argv(&["framework_tool", "--kblight"]) };
    let key_call = |args: &[String]| call(args, if ec_keys { priv_argv(args, helper.as_deref()) } else { None });
    let fw_call = |args: &[String]| call(args, priv_argv(args, helper.as_deref()));
    let saved = load_state(&path);
    if !off {
        if saved.is_empty() {
            return;
        }
        let held = (qmk || ec_keys) && saved.level.is_some() && daylight && lux().is_some_and(|l| l >= DAYLIGHT_LUX);
        if let (true, Some(level), false) = (qmk || ec_keys, saved.level, held) {
            // The Laptop 16 keyboard rounds down, so writing its reported 33 back gives 0.
            let level = if qmk { (level + 1).min(100) } else { level };
            key_call(&[keys.clone(), vec![level.to_string()]].concat());
        }
        match (&led, &saved.fp) {
            (Some(led), Some(fp)) if fp.sys => wake_power(led, fp),
            (_, Some(fp)) if fw => {
                if let Some(args) = fp_restore(fp) {
                    fw_call(&args);
                }
            }
            _ => {}
        }
        match saved.level {
            // Kept so the next wake in a darker room can still restore the level.
            Some(level) if held => save(&path, &Map::from_iter([("level".into(), level.into()), ("held".into(), true.into())])),
            _ => {
                let _ = fs::remove_file(&path);
            }
        }
        return;
    }
    // A second off would read the darkened lights and lose the levels to restore.
    if !saved.is_empty() && !saved.held {
        return;
    }
    let mut state = Map::new();
    if qmk || ec_keys {
        let (code, out) = key_call(&keys);
        let read = tool_ok(code, &out).then(|| percent(&out)).flatten();
        if let Some(level) = read.filter(|&l| l != 0).or(saved.level).or(read) {
            state.insert("level".into(), level.into());
        }
        key_call(&[keys, argv(&["0"])].concat());
    }
    let fp = if let Some(led) = &led {
        blank_power(led)
    } else if fw {
        let get = argv(&["framework_tool", "--fp-brightness"]);
        let (code, out) = fw_call(&get);
        let fp = tool_ok(code, &out).then(|| fp_state(&out)).flatten();
        fw_call(&argv(&["framework_tool", "--fp-brightness", "0"]));
        fp
    } else {
        None
    };
    if let Some(fp) = fp {
        state.insert("fp".into(), fp.to_json());
    }
    if state.is_empty() {
        let _ = fs::remove_file(&path);
    } else {
        save(&path, &state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kblight-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("kblight.json");
        fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn percent_is_the_first_match_in_range() {
        for (text, want) in [
            ("Brightness: 40%\n", Some(40)),
            ("Keyboard backlight: 30%", Some(30)),
            ("Brightness:7 %", Some(7)),
            ("Brightness: 100%", Some(100)),
            ("Brightness: 0%", Some(0)),
            ("Brightness: 101%", None),
            ("Brightness: 101%\nBrightness: 50%", None),
            ("Brightness: 1000%", None),
            ("Brightness: 1000%\nBrightness: 20%", Some(20)),
            ("Brightness: 0040%", None),
            ("Brightness: 040%", Some(40)),
            ("Brightness: -5%", None),
            ("Brightness: 40", None),
            ("Brightness 40%", None),
            ("brightness: 40%", None),
            ("", None),
        ] {
            assert_eq!(percent(text), want, "{text:?}");
        }
    }

    #[test]
    fn fp_state_reads_the_level_name_and_brightness() {
        let fp = |requested: Option<&str>, brightness| Some(Fp { requested: requested.map(String::from), brightness, ..Fp::default() });
        assert_eq!(fp_state("Requested: Auto\nBrightness: 10%\n"), fp(Some("auto"), Some(10)));
        assert_eq!(fp_state("Requested:   Ultra  Low \nBrightness: 10%"), fp(Some("ultra-low"), Some(10)));
        assert_eq!(fp_state("Requested: ULTRA-LOW"), fp(Some("ultra-low"), None));
        assert_eq!(fp_state("Requested:\nCustom"), fp(Some("custom"), None));
        assert_eq!(fp_state("Requested: Auto\nRequested: Low"), fp(Some("auto"), None));
        assert_eq!(fp_state("Keyboard backlight: 30%"), None);
        assert_eq!(fp_state("Requested: nope\nBrightness: 101%"), None);
        assert_eq!(fp_state("Brightness: 1000%\nBrightness: 5%"), fp(None, Some(5)));
        assert_eq!(fp_state("Requested:"), None);
    }

    #[test]
    fn wake_leaves_brightness_to_a_restored_trigger() {
        let led = tmp("wake-auto", "").parent().unwrap().to_path_buf();
        fs::write(led.join("brightness"), "0").unwrap();
        wake_power(&led, &Fp { sys: true, trigger: Some("chromeos-auto".into()), brightness: Some(100), ..Fp::default() });
        assert_eq!(fs::read_to_string(led.join("trigger")).unwrap(), "chromeos-auto");
        assert_eq!(fs::read_to_string(led.join("brightness")).unwrap(), "0", "a brightness write takes the LED off its trigger");
        wake_power(&led, &Fp { sys: true, trigger: Some("none".into()), brightness: Some(40), ..Fp::default() });
        assert_eq!(fs::read_to_string(led.join("brightness")).unwrap(), "40");
    }

    #[test]
    fn trigger_is_the_first_bracketed_name() {
        for (text, want) in [
            ("none [chromeos-auto] timer\n", Some("chromeos-auto")),
            ("[a:b_c-1] x [d]", Some("a:b_c-1")),
            ("[] [late]", Some("late")),
            ("[[a]", None),
            ("[bad name]", None),
            ("[first] [second]", Some("first")),
            ("none timer", None),
            ("[unclosed", None),
            ("", None),
        ] {
            assert_eq!(trigger(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn model_needs_framework_and_a_16_12_or_13_word() {
        for (vendor, product, want) in [
            ("Framework", "Laptop 16 (AMD Ryzen AI 300 Series)", "16"),
            ("FRAMEWORK COMPUTER", "laptop 16", "16"),
            ("Framework", "Laptop 13 (Intel Core Ultra Series 1)", "ec"),
            ("Framework", "Laptop 12", "ec"),
            ("Framework", "Laptop 13 Pro 16GB", "ec"),
            ("Framework", "Laptop 16 / 13", "16"),
            ("Framework", "Laptop 160", ""),
            ("Framework", "Laptop16", ""),
            ("Framework", "Laptop 1.6", ""),
            ("Framework", "Desktop", ""),
            ("LENOVO", "Laptop 16", ""),
            ("", "Laptop 16", ""),
        ] {
            assert_eq!(model_of(vendor, product), want, "{vendor} {product}");
        }
    }

    #[test]
    fn saved_files_of_any_shape_load_as_empty_or_cleaned() {
        for (i, text) in ["", "nope", "[]", "[1, 2]", "5", "\"x\"", "null", "true", "{}", "{\"level\": 150}", "{\"level\": 40.0}", "{\"level\": true}", "{\"held\": 1}", "{\"fp\": 3}", "{\"fp\": {}}", "{\"fp\": {\"requested\": [\"auto\"], \"trigger\": 5}}"].iter().enumerate() {
            assert_eq!(load_state(&tmp(&format!("empty{i}"), text)), Saved::default(), "{text:?}");
        }
        assert_eq!(load_state(Path::new("/nonexistent/kblight.json")), Saved::default());
        let saved = load_state(&tmp("full", r#"{"held": true, "level": 40, "fp": {"sys": true, "requested": "custom", "trigger": "a b", "brightness": 200}}"#));
        assert_eq!(saved, Saved { held: true, level: Some(40), fp: Some(Fp { sys: true, requested: Some("custom".into()), ..Fp::default() }) });
    }

    #[test]
    fn fp_restore_prefers_a_named_level() {
        let fp = |r: &str, b| Fp { requested: Some(r.into()), brightness: b, ..Fp::default() };
        assert_eq!(fp_restore(&fp("auto", Some(5))), Some(argv(&["framework_tool", "--fp-led-level", "auto"])));
        assert_eq!(fp_restore(&fp("custom", Some(5))), Some(argv(&["framework_tool", "--fp-brightness", "5"])));
        assert_eq!(fp_restore(&fp("custom", None)), None);
    }

    #[test]
    fn helper_argv_drops_the_dashes() {
        let helper = Path::new("/h/ops-priv");
        assert_eq!(priv_argv(&argv(&["framework_tool", "--kblight", "0"]), Some(helper)), Some(argv(&["/h/ops-priv", "kblight", "0"])));
        assert_eq!(priv_argv(&argv(&["qmk_hid"]), Some(helper)), None);
        assert_eq!(priv_argv(&argv(&["tee", "x"]), Some(helper)), None);
        assert_eq!(priv_argv(&argv(&["framework_tool", "--kblight"]), None), None);
    }
}
