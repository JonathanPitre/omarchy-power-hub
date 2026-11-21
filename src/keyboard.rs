use crate::{config, kblight, paths};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

/// Read levels in percent, with the mode name each one stands for.
pub const LEVELS: [(&str, i64); 4] = [("off", 0), ("low", 33), ("medium", 67), ("high", 100)];

/// Key-capability bits for KEY_Q..KEY_P, KEY_A..KEY_L and KEY_Z..KEY_M.
const LETTER_KEYS: u64 = 0x0007_F07F_C3FF_0000;

/// Backlight level for Auto: off with an external keyboard, else by ambient lux.
pub fn auto_level(lux: Option<f64>, external: bool) -> i64 {
    if external {
        return 0;
    }
    match lux {
        None => 33,
        Some(lux) if lux < 16.0 => 33,
        Some(lux) if lux < 50.0 => 67,
        Some(lux) if lux < 100.0 => 100,
        Some(_) => 0,
    }
}

/// True when a block of `/proc/bus/input/devices` is a real external keyboard.
fn external_block(block: &str) -> bool {
    let field = |key: &str| block.lines().find_map(|line| line.split_once(key).map(|(_, rest)| rest));
    let first_word = |key: &str| field(key).and_then(|rest| rest.split_whitespace().next());
    let letters = field("KEY=")
        .and_then(|rest| rest.split_whitespace().last())
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|keys| keys & LETTER_KEYS == LETTER_KEYS);
    let kbd = field("Handlers=").is_some_and(|rest| rest.split_whitespace().any(|word| word == "kbd"));
    first_word("Bus=") != Some("0006")
        && !first_word("Vendor=").is_some_and(|vendor| vendor.eq_ignore_ascii_case("32ac"))
        && kbd
        && letters
}

/// True when any device block is an external keyboard. Takes the text of `/proc/bus/input/devices`.
pub fn external_keyboard(devices: &str) -> bool {
    devices.split("\n\n").any(external_block)
}

/// Mode name for a read level: below 33 off, below 67 low, below 100 medium, else high.
pub fn name_of(level: i64) -> &'static str {
    match level {
        ..33 => "off",
        33..67 => "low",
        67..100 => "medium",
        _ => "high",
    }
}

/// Value to write for a read level: 33 writes as 34 and 67 as 68, others pass through.
pub fn write_value(level: i64) -> i64 {
    match level {
        33 => 34,
        67 => 68,
        other => other,
    }
}

/// True when the saved keyboard mode is Auto, or no mode is saved and daylight mode is on.
pub fn is_auto(prefs: &Value) -> bool {
    match prefs.get("keyboard") {
        Some(mode) => mode == "auto",
        None => prefs.get("daylight") == Some(&Value::Bool(true)),
    }
}

/// The backlight level the keyboard reports, or None off a Laptop 16, without `qmk_hid`, or when it does not answer.
pub fn read() -> Option<i64> {
    if !kblight::qmk_available() {
        return None;
    }
    let (code, out) = kblight::run(&backlight());
    kblight::tool_ok(code, &out).then(|| kblight::percent(&out)).flatten()
}

/// The level Auto wants now.
pub fn target() -> i64 {
    let devices = fs::read_to_string(paths::root("/proc/bus/input/devices")).unwrap_or_default();
    auto_level(kblight::lux(), external_keyboard(&devices))
}

fn state_file() -> PathBuf {
    paths::plugin_run_dir().join("kbauto.json")
}

/// What Auto last wrote and whether a key press has since taken the level over.
fn load() -> (Option<i64>, bool) {
    let raw = fs::read_to_string(state_file()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or_default();
    (raw["wrote"].as_i64(), raw["hold"] == true)
}

fn save(wrote: Option<i64>, hold: bool) {
    let _ = fs::create_dir_all(paths::plugin_run_dir());
    let _ = fs::write(state_file(), json!({"wrote": wrote, "hold": hold}).to_string() + "\n");
}

/// The `qmk_hid` call that reads the backlight, and sets it with a value appended.
pub(crate) fn backlight() -> Vec<String> {
    kblight::argv(&["qmk_hid", "via", "--backlight"])
}

/// Sets the backlight to a read level; true when the keyboard took it.
pub fn write_level(level: i64) -> bool {
    let (code, out) = kblight::run(&[backlight(), vec![write_value(level).to_string()]].concat());
    kblight::tool_ok(code, &out)
}

fn write(level: i64) {
    if write_level(level) {
        save(Some(level), false);
    }
}

/// Takes the level back from the keys and sets it for the room.
pub fn apply_auto() {
    write(target());
}

/// The `lights auto` refresh tick: follows the room until a key press sets the level by hand.
pub fn tick() {
    if !is_auto(&config::saved_prefs()) || paths::plugin_run_dir().join("kblight.json").exists() {
        return;
    }
    let (Some(level), (wrote, false)) = (read(), load()) else { return };
    if wrote.is_some_and(|w| w != level) {
        return save(wrote, true);
    }
    let want = target();
    if want != level && !paths::plugin_run_dir().join("kblight.json").exists() {
        write(want);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn auto_follows_the_table() {
        for (lux, ext, want) in [(None, false, 33), (Some(0.0), false, 33), (Some(15.0), false, 33), (Some(16.0), false, 67),
                                 (Some(49.0), false, 67), (Some(50.0), false, 100), (Some(99.0), false, 100),
                                 (Some(100.0), false, 0), (Some(3355.0), false, 0), (Some(0.0), true, 0)] {
            assert_eq!(auto_level(lux, ext), want, "{lux:?} {ext}");
        }
    }

    #[test]
    fn external_keyboard_skips_framework_virtual_and_letterless_devices() {
        let block = |bus: &str, vendor: &str, handlers: &str, key: &str| format!(
            "I: Bus={bus} Vendor={vendor} Product=0001 Version=0111\nN: Name=\"x\"\nH: Handlers={handlers}\nB: KEY={key}\n\n");
        let letters = "1000000000007 ff9f207ac14057ff febeffdfffefffff fffffffffffffffe";
        let fw = block("0003", "32ac", "sysrq kbd leds event10", letters);
        let solaar = block("0006", "046d", "sysrq kbd event27", letters);
        let video = block("0019", "0000", "kbd event8", "3e000b00000000 0 0 0");
        let receiver = block("0003", "046d", "sysrq kbd leds event2", letters);
        assert!(!external_keyboard(&(fw.clone() + &solaar + &video)));
        assert!(external_keyboard(&(fw + &receiver)));
        assert!(!external_keyboard(""));
    }

    #[test]
    fn a_saved_daylight_reads_as_auto_until_a_mode_is_saved() {
        assert!(is_auto(&json!({"daylight": true})));
        assert!(!is_auto(&json!({"daylight": true, "keyboard": "low"})));
        assert!(is_auto(&json!({"keyboard": "auto"})));
        assert!(!is_auto(&json!({})));
    }

    #[test]
    fn levels_round_trip_through_the_module() {
        assert_eq!([0, 30, 33, 66, 67, 99, 100].map(name_of), ["off", "off", "low", "low", "medium", "medium", "high"]);
        assert_eq!([0, 33, 67, 100].map(write_value), [0, 34, 68, 100]);
    }
}
