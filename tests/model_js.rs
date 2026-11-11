//! Runs `Model.js` under `node`; every test skips with a note when `node` is missing.
use serde_json::{Value, json};
use std::process::Command;

/// The value `body` prints with `console.log(JSON.stringify(..))`, or None when `node` is not installed.
fn run(body: &str) -> Option<Value> {
    let model = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Model.js")).unwrap().replace(".pragma library", "");
    let out = match Command::new("node").arg("-e").arg(format!("{model}\n{body}\n")).output() {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: node is not installed");
            return None;
        }
        Err(e) => panic!("node: {e}"),
    };
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    Some(serde_json::from_slice(&out.stdout).unwrap())
}

fn expect(body: &str, want: Value) {
    if let Some(got) = run(body) {
        assert_eq!(got, want);
    }
}

#[test]
fn battery_icon_shows_full_only_at_100() {
    expect(r#"console.log(JSON.stringify({
        d95: batteryIcon(95, "Discharging") === DISCHARGE[8],
        d100: batteryIcon(100, "Discharging") === DISCHARGE[9],
        c95: batteryIcon(95, "Charging") === CHARGE[8],
        d15: batteryIcon(15, "Discharging") === DISCHARGE[0],
        d25: batteryIcon(25, "Discharging") === DISCHARGE[1],
        alert: batteryIcon(8, "Discharging") === ALERT_ICON,
        full: batteryIcon(97, "Full") === FULL_ICON
    }));"#, json!({"d95": true, "d100": true, "c95": true, "d15": true, "d25": true, "alert": true, "full": true}));
}

#[test]
fn bar_percent_toggles_on_battery_too() {
    expect(r#"var on = { showOnBattery: true, showOnAc: false };
    console.log(JSON.stringify({
        battery: barPercentKey("Discharging"),
        ac: barPercentKey("Charging"),
        shownOnBattery: showBarPercent("Discharging", on),
        hiddenOnAc: showBarPercent("Full", on),
        hiddenOnBattery: showBarPercent("Discharging", { showOnBattery: false, showOnAc: false })
    }));"#, json!({"battery": "showPercentageOnBattery", "ac": "alwaysShowPercentage", "shownOnBattery": true, "hiddenOnAc": false, "hiddenOnBattery": false}));
}

#[test]
fn draw_value_discharging_charging_idle() {
    expect(r#"console.log(JSON.stringify({
        dis: drawValue({ status: "Discharging", watts: 12.3 }),
        chg: drawValue({ status: "Charging", watts: 20 }),
        idle: drawValue({ status: "Not charging", watts: 0 }),
        none: drawValue(null)
    }));"#, json!({"dis": "12.3 W", "chg": "20.0 W", "idle": "", "none": ""}));
}

#[test]
fn mode_label_carries_time() {
    expect(r#"console.log(JSON.stringify({
        bat: modeLabel({ present: true, status: "Discharging", minutes: 228, percent: 62 }),
        chg: modeLabel({ present: true, status: "Charging", minutes: 65, acOnline: 1, percent: 47 }),
        full: modeLabel({ present: true, status: "Full", acOnline: 1, percent: 100 }),
        hold: modeLabel({ present: true, status: "Not charging", acOnline: 1, percent: 80 }),
        est: modeLabel({ present: true, status: "Charging", minutes: 0, acOnline: 1, percent: 47 }),
        busPowered: modeLabel({ present: true, status: "Discharging", minutes: 294, acOnline: 1, percent: 100 })
    }));"#, json!({"bat": "DISCHARGING · 3h 48m REMAINING", "chg": "CHARGING · FULL IN 1h 05m", "full": "FULLY CHARGED", "hold": "PLUGGED IN · NOT CHARGING",
                  "est": "PLUGGED IN · CHARGING",
                  // A bus-powered USB-C monitor keeps ACAD online after the adapter is unplugged.
                  "busPowered": "DISCHARGING · 4h 54m REMAINING"}));
}

#[test]
fn mode_parts_split_state_and_time() {
    expect(r#"console.log(JSON.stringify([
        modeParts({ present: true, status: "Charging", minutes: 15, acOnline: 1, percent: 95 }),
        modeParts({ present: true, status: "Discharging", minutes: 272, percent: 92 }),
        modeParts({ present: true, status: "Full", acOnline: 1, percent: 100 })
    ]));"#, json!([{"state": "CHARGING", "time": "FULL IN 15m"}, {"state": "DISCHARGING", "time": "4h 32m REMAINING"}, {"state": "FULLY CHARGED", "time": ""}]));
}

#[test]
fn mode_parts_names_the_drain_to_the_charge_limit() {
    expect(r#"var drain = { present: true, status: "Discharging", minutes: 192, acOnline: 1, percent: 99, estimating: true, drainTo: 80 };
    console.log(JSON.stringify([
        modeParts(drain),
        modeParts(Object.assign({}, drain, { drainTo: 0 })),
        // The poll lags UPower, so a stale drainTo must not outlive the drain.
        modeParts({ present: true, status: "Not charging", acOnline: 1, percent: 80, drainTo: 80 }),
        drainTarget(drain),
        drainTarget({ status: "Not charging", drainTo: 80 })
    ]));"#, json!([{"state": "PLUGGED IN · DRAINING TO 80%", "time": ""}, {"state": "DISCHARGING", "time": "CALCULATING…"},
                  {"state": "PLUGGED IN · NOT CHARGING", "time": ""}, 80, 0]));
}

/// The EC smooths current_now (~8 s): 30 s after unplug a 28 W load first reads 9.9 W, so the first estimate was 431 min instead of ~150.
#[test]
fn mode_parts_calculating_while_reading_settles() {
    expect(r#"console.log(JSON.stringify([
        modeParts({ present: true, status: "Discharging", minutes: 431, percent: 92, estimating: true }),
        modeParts({ present: true, status: "Charging", minutes: 526, acOnline: 1, percent: 92, estimating: true }),
        modeParts({ present: true, status: "Discharging", minutes: 150, percent: 92, estimating: false })
    ]));"#, json!([{"state": "DISCHARGING", "time": "CALCULATING…"}, {"state": "CHARGING", "time": "CALCULATING…"}, {"state": "DISCHARGING", "time": "2h 30m REMAINING"}]));
}

#[test]
fn health_tier() {
    expect("console.log(JSON.stringify([healthTier(93), healthTier(80), healthTier(79), healthTier(60), healthTier(59), healthTier(0)]));",
           json!(["", "", "worn", "worn", "replace", ""]));
}

#[test]
fn charge_limit_label() {
    expect(r#"console.log(JSON.stringify({
        fw: chargeLimitLabel({ kind: "framework", tool: true, limit: 80 }, false),
        fwMissing: chargeLimitLabel({ kind: "framework", tool: false, limit: null }, false),
        boost: chargeLimitLabel({ kind: "framework", tool: true, limit: 80 }, true),
        off: chargeLimitLabel({ kind: "upower", health: false, policy: "80" }, false),
        range: chargeLimitLabel({ kind: "upower", health: true, policy: "75-80" }, false),
        auto: chargeLimitLabel({ kind: "upower", health: true, policy: "firmware" }, false),
        none: chargeLimitLabel(undefined, false)
    }));"#, json!({"fw": "80%", "fwMissing": "—", "boost": "100% (boost)", "off": "Off", "range": "80%", "auto": "Auto", "none": "—"}));
}

#[test]
fn history_bars_empty_are_unknown() {
    expect(r#"var now = 1000000000000;
    var bars = historyBars([], now);
    console.log(JSON.stringify({ n: bars.length, first: bars[0].pct, last: bars[95].pct }));"#, json!({"n": 96, "first": -1, "last": -1}));
}

#[test]
fn history_bars_carry_forward_after_first_sample() {
    expect(r#"var now = 1000000000000;
    var end = Math.floor(now / 1000);
    var start = end - 86400;
    var t = start + 1800;
    var bars = historyBars([{ t: t, pct: 40, charging: false }], now);
    var filled = bars.filter(function(b) { return b.pct >= 0; }).length;
    console.log(JSON.stringify({ n: bars.length, b0: bars[0].pct, b1: bars[1].pct, b2: bars[2].pct, last: bars[95].pct, filled: filled }));"#,
           json!({"n": 96, "b0": -1, "b1": -1, "b2": 40, "last": 40, "filled": 94}));
}

#[test]
fn history_bars_charging_marks_its_buckets() {
    let Some(got) = run(r#"var now = 1000000000000;
    var end = Math.floor(now / 1000);
    var start = end - 86400;
    var bars = historyBars([
        { t: start + 100, pct: 50, charging: false },
        { t: start + 3600, pct: 70, charging: true }
    ], now);
    var chg = [];
    for (var i = 0; i < bars.length; i++) if (bars[i].charging) chg.push(i);
    console.log(JSON.stringify({ chg: chg, pctHour1: bars[4].pct }));"#) else { return };
    assert!(got["chg"].as_array().unwrap().contains(&json!(4)));
    assert_eq!(got["pctHour1"], 70);
}

#[test]
fn idle_labels_read_like_windows_and_macos() {
    expect("console.log(JSON.stringify([0, 60, 300, 150, 45, 3600, 7200, 5400].map(fmtIdleLong)));",
           json!(["Never", "1 minute", "5 minutes", "2 min 30 s", "45 seconds", "1 hour", "2 hours", "1 h 30 min"]));
}

#[test]
fn idle_saved_value_off_the_list_is_inserted_in_order() {
    expect(r#"var opts = idleOptions(IDLE_STOPS, 150);
    console.log(JSON.stringify({ values: opts.map(function(o) { return o.value; }), label: opts[3].label }));"#,
           json!({"values": ["0", "60", "120", "150", "180", "300", "600", "900", "1200", "1800"], "label": "2 min 30 s"}));
}

#[test]
fn idle_saved_stop_is_not_duplicated() {
    expect(r#"console.log(JSON.stringify({ n: idleOptions(HIBERNATE_STOPS, 3600).length, label: idleOptions(HIBERNATE_STOPS, 60)[1].label }));"#,
           json!({"n": 17, "label": "1 minute"}));
}

#[test]
fn saver_is_never_once_displays_go_off_first() {
    expect("console.log(JSON.stringify([effectiveSaver(60, 180), effectiveSaver(180, 180), effectiveSaver(300, 180), effectiveSaver(300, 0)]));", json!([60, 0, 0, 300]));
}

#[test]
fn saver_menu_offers_only_times_before_displays_off() {
    expect(r#"console.log(JSON.stringify({
        before: saverOptions(IDLE_STOPS, 300, 180).map(function(o) { return o.value; }),
        unlimited: saverOptions(IDLE_STOPS, 60, 0).length
    }));"#, json!({"before": ["0", "60", "120"], "unlimited": 9}));
}
