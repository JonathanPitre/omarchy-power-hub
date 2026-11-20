use serde_json::Value;

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
