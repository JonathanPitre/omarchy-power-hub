//! The `data` verb: port of `tests/fixtures/battery-plus-data`, one JSON object for the panel.
use crate::{config, exec, hypr, paths, power};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// File contents without trailing newlines, like `$(cat f)`.
fn cat(p: impl AsRef<Path>) -> Option<String> {
    let bytes = fs::read(p).ok()?;
    Some(String::from_utf8_lossy(&bytes).trim_end_matches('\n').to_string())
}

/// Integer file contents; garbage reads as 0 instead of aborting like the shell's arithmetic.
fn int(s: &str) -> i64 {
    s.trim().parse().unwrap_or(0)
}

/// `awk` number conversion: the longest numeric prefix, else 0.
fn num(s: &str) -> f64 {
    let s = s.trim_start();
    let b = s.as_bytes();
    let digits = |mut i: usize| {
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let mut i = digits(usize::from(matches!(b.first(), Some(b'-' | b'+'))));
    if b.get(i) == Some(&b'.') {
        i = digits(i + 1);
    }
    s[..i].parse().unwrap_or(0.0)
}

/// `printf "%.<prec>f"` read back as a number.
fn round(x: f64, prec: usize) -> f64 {
    format!("{x:.prec$}").parse().unwrap_or(0.0)
}

fn executable(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn on_path(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| executable(&d.join(tool))))
}

fn sorted_dir(p: PathBuf) -> Vec<PathBuf> {
    let mut v: Vec<_> = fs::read_dir(p)
        .map(|d| d.flatten().filter(|e| !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

struct Battery {
    present: bool,
    percent: i64,
    status: String,
    cycles: i64,
    now: i64,
    full: i64,
    design: i64,
    power: i64,
}

fn battery() -> Battery {
    let mut b = Battery { present: false, percent: 0, status: "Unknown".into(), cycles: 0, now: 0, full: 0, design: 0, power: 0 };
    let Some(dir) = sorted_dir(paths::root("/sys/class/power_supply"))
        .into_iter()
        .find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("BAT")))
    else {
        return b;
    };
    let g = |name: &str, default: i64| cat(dir.join(name)).map_or(default, |s| int(&s));
    b.present = true;
    b.percent = g("capacity", 0);
    b.status = cat(dir.join("status")).unwrap_or(b.status);
    let volt = g("voltage_now", 0);
    b.cycles = g("cycle_count", 0);
    let mut vdes = g("voltage_min_design", volt);
    if vdes <= 0 {
        vdes = volt;
    }
    let (mut enow, mut efull, mut edes, mut pnow) = (g("energy_now", 0), g("energy_full", 0), g("energy_full_design", 0), g("power_now", 0));
    if enow == 0 && vdes > 0 {
        let scale = |name: &str| g(name, 0).saturating_mul(vdes) / 1_000_000;
        (enow, efull, edes) = (scale("charge_now"), scale("charge_full"), scale("charge_full_design"));
    }
    if pnow == 0 && volt > 0 {
        let mut cur = g("current_now", 0);
        if cur == 0 {
            cur = g("current_avg", 0);
        }
        pnow = cur.saturating_abs().saturating_mul(volt) / 1_000_000;
    }
    (b.now, b.full, b.design, b.power) = (enow, efull, edes, pnow.saturating_abs());
    b
}

/// Package, core and GPU watts from two RAPL samples 0.4 s apart; None when unreadable.
fn rapl() -> Option<[f64; 3]> {
    let base = paths::root("/sys/class/powercap");
    let doms = ["intel-rapl:0", "intel-rapl:0:0", "intel-rapl:0:1"];
    // `head -c1` succeeds on an empty file, so only a failed open or read counts.
    let _first_byte = fs::File::open(base.join(doms[0]).join("energy_uj")).ok()?.read(&mut [0u8]).ok()?;
    let first: Vec<(usize, String)> = doms
        .iter()
        .enumerate()
        .filter(|(_, d)| fs::File::open(base.join(d).join("energy_uj")).is_ok())
        .map(|(i, d)| (i, cat(base.join(d).join("energy_uj")).unwrap_or_default()))
        .collect();
    if first.is_empty() {
        return None;
    }
    thread::sleep(Duration::from_millis(400));
    let mut watts = [0.0; 3];
    for (i, e0) in first.iter().filter(|(_, e)| !e.is_empty()) {
        let dir = base.join(doms[*i]);
        let e1 = cat(dir.join("energy_uj")).unwrap_or_else(|| e0.clone());
        let max = cat(dir.join("max_energy_range_uj")).map_or(0.0, |s| num(&s));
        let mut d = num(&e1) - num(e0);
        if d < 0.0 && max > 0.0 {
            d += max;
        }
        watts[*i] = round(d / 1e6 / 0.4, 2);
    }
    Some(watts)
}

fn tlp(ac: bool) -> Value {
    let mut mode = if let Some(pwr) = cat(paths::root("/run/tlp/last_pwr")) {
        if pwr.split_whitespace().next() == Some("1") { "battery" } else { "ac" }
    } else if ac {
        "ac"
    } else {
        "battery"
    };
    let mut manual = false;
    if let Some(m) = cat(paths::root("/run/tlp/manual_mode")) {
        match m.split_whitespace().next() {
            Some("0") => (manual, mode) = (true, "ac"),
            Some("1" | "2") => (manual, mode) = (true, "battery"),
            _ => {}
        }
    }
    json!({"installed": on_path("tlp"), "active": exec::run(&["systemctl", "is-active", "--quiet", "tlp"], Duration::from_secs(2)).0 == 0,
           "mode": mode, "manual": manual})
}

fn wifi_power_save() -> bool {
    let Some(dev) = sorted_dir(paths::root("/sys/class/net")).into_iter().find_map(|p| {
        let n = p.file_name()?.to_string_lossy().into_owned();
        n.starts_with("wl").then_some(n)
    }) else {
        return false;
    };
    let (_, out) = exec::run(&["iw", "dev", &dev, "get", "power_save"], Duration::from_secs(2));
    let last: Vec<&str> = out.lines().map(|l| l.split_whitespace().last().unwrap_or("")).collect();
    last.join("\n").trim_end_matches('\n') == "on"
}

/// More than half of the devices under `dir` have runtime power management on.
fn mostly_auto(dir: &str) -> bool {
    let controls: Vec<PathBuf> = sorted_dir(paths::root(dir)).into_iter().map(|d| d.join("power/control")).filter(|f| fs::metadata(f).is_ok()).collect();
    let auto = controls.iter().filter(|f| cat(f).is_some_and(|c| c.lines().any(|l| l == "auto"))).count();
    !controls.is_empty() && auto > controls.len() / 2
}

fn panel_power_savings() -> Option<bool> {
    sorted_dir(paths::root("/sys/class/drm"))
        .into_iter()
        .map(|d| d.join("amdgpu/panel_power_savings"))
        .find(|f| fs::File::open(f).is_ok())
        .map(|f| cat(f).is_some_and(|s| int(&s) > 0))
}

/// The keyboard light is only ours to control on a Framework.
fn light_sensor() -> bool {
    power::framework() && sorted_dir(paths::root("/sys/bus/iio/devices")).iter().any(|d| d.join("in_illuminance_raw").exists())
}

fn tweaks() -> Value {
    let abm = panel_power_savings();
    let saved = config::tweak_state();
    let mut t = Map::new();
    let animations = hypr::request("getoption animations:enabled");
    for (k, v) in [
        ("wifi", wifi_power_save()),
        ("usb", mostly_auto("/sys/bus/usb/devices")),
        ("pcie", mostly_auto("/sys/bus/pci/devices")),
        ("audio", cat(paths::root("/sys/module/snd_hda_intel/parameters/power_save")).is_some_and(|s| int(&s) > 0)),
        ("abm", abm == Some(true)),
        ("abmAvailable", abm.is_some()),
        ("effects", animations.is_some_and(|r| r.lines().next() == Some("bool: false"))),
        ("daylight", saved["daylight"] == true),
        ("daylightAvailable", light_sensor()),
    ] {
        t.insert(k.into(), Value::Bool(v));
    }
    if let Value::Object(saved) = saved {
        t.extend(saved.into_iter().filter(|(k, _)| k.ends_with("Pinned")));
    }
    Value::Object(t)
}

/// Every `key<digits>` match in `line`, as `grep -oE 'key=...'` would give them.
fn fields<'a>(line: &'a str, key: &str, signed: bool, dot: bool) -> Vec<&'a str> {
    let mut found = vec![];
    let mut pos = 0;
    while let Some(i) = line[pos..].find(key) {
        let start = pos + i + key.len();
        let rest = &line[start..];
        let sign = usize::from(signed && rest.starts_with('-'));
        let n = rest[sign..].bytes().take_while(|b| b.is_ascii_digit() || (dot && *b == b'.')).count();
        if n > 0 {
            found.push(&rest[..sign + n]);
            pos = start + sign + n;
        } else {
            pos += i + 1;
        }
    }
    found
}

fn sleep() -> Value {
    let installed = executable(&paths::root("/usr/lib/systemd/system-sleep/battery-plus-sleep"));
    let mut out = Map::new();
    if let Some(text) = fs::read(paths::root("/var/log/battery-plus-sleep.log")).ok().map(|b| String::from_utf8_lossy(&b).into_owned()) {
        let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(&text).split('\n').collect();
        let last = lines.last().copied().unwrap_or("");
        if !last.is_empty() {
            let first = |key: &str, signed: bool, dot: bool| fields(last, key, signed, dot).first().copied().unwrap_or("0");
            let average = |key: &str, signed: bool| {
                let all: Vec<f64> = lines[lines.len().saturating_sub(5)..].iter().flat_map(|l| fields(l, key, signed, true)).map(num).collect();
                if all.is_empty() { 0.0 } else { round(all.iter().sum::<f64>() / all.len() as f64, 3) }
            };
            for (k, v) in [
                ("lastAt", json!(last.split_whitespace().next().unwrap_or(""))),
                ("lastDurationSec", json!(int(first("duration=", false, false)))),
                ("lastPreCap", json!(int(first("pre_cap=", false, false)))),
                ("lastPostCap", json!(int(first("post_cap=", false, false)))),
                ("lastWatts", json!(num(first("watts=", false, true)))),
                ("lastPctPerHour", json!(num(first("pct_per_hour=", true, true)))),
                ("samples", json!(text.split('\n').filter(|l| !l.is_empty()).count())),
                ("avgWatts", json!(average("watts=", false))),
                ("avgPctPerHour", json!(average("pct_per_hour=", true))),
            ] {
                out.insert(k.into(), v);
            }
        }
    }
    out.insert("installed".into(), json!(installed));
    Value::Object(out)
}

/// BCM4377b suspend failure (errno -5 at that device), the only one the Wi-Fi fix addresses.
fn wifi_fix() -> Value {
    let pci = paths::root("/sys/bus/pci/devices");
    let chip = sorted_dir(pci.clone()).iter().any(|d| cat(d.join("modalias")).is_some_and(|m| m.contains("v000014E4d00004488")));
    let stats = paths::root("/sys/power/suspend_stats");
    let fails = cat(stats.join("fail")).map_or(0, |s| int(&s));
    let mut problem = false;
    if fails > 0 && fs::File::open(stats.join("last_failed_dev")).is_ok() {
        let dev = cat(stats.join("last_failed_dev")).unwrap_or_default();
        if !dev.is_empty() && cat(stats.join("last_failed_errno")).as_deref() == Some("-5") {
            let attr = |n: &str| cat(pci.join(&dev).join(n));
            problem = attr("vendor").as_deref() == Some("0x14e4") && attr("device").as_deref() == Some("0x4488");
        }
    }
    json!({"chipPresent": chip, "installed": executable(&paths::root("/usr/lib/systemd/system-sleep/battery-plus-wifi-suspend-fix")),
           "problemSeen": problem, "failCount": fails})
}

pub fn collect() -> Value {
    let b = battery();
    let ac = !power::on_battery();
    let drain_to = if b.status == "Discharging" { power::drain_to(&config::raw()) } else { 0 };
    let (now, full, p) = (b.now as f64, b.full as f64, b.power as f64);
    let minutes = match b.status.as_str() {
        _ if b.power <= 0 => 0,
        "Discharging" => (now / p * 60.0) as i64,
        "Charging" => ((full - now) / p * 60.0) as i64,
        _ => 0,
    };
    let health = if b.design > 0 { round(full * 100.0 / b.design as f64, 0) } else { 0.0 };
    let rapl = rapl();
    let [package, core, gpu] = rapl.unwrap_or_default();
    json!({
        "battery": {"present": b.present, "percent": b.percent, "status": b.status, "watts": round(p / 1e6, 2), "health": health,
                    "whFull": round(full / 1e6, 1), "whDesign": round(b.design as f64 / 1e6, 1), "cycles": b.cycles, "minutes": minutes,
                    "acOnline": u8::from(ac), "fraction": round(b.percent as f64 / 100.0, 3), "drainTo": drain_to},
        "rapl": {"available": rapl.is_some(), "package": package, "core": core, "gpu": gpu},
        "tlp": tlp(ac),
        "tweaks": tweaks(),
        "sleep": sleep(),
        "wifiFix": wifi_fix(),
    })
}
