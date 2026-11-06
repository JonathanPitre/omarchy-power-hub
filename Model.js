.pragma library

// Nerd Font battery glyphs (Material Design set, matching the built-in power widget).
var DISCHARGE = ["󰁺", "󰁻", "󰁼", "󰁽", "󰁾",
                 "󰁿", "󰂀", "󰂁", "󰂂", "󰁹"];
var CHARGE    = ["󰢜", "󰂆", "󰂇", "󰂈", "󰢝",
                 "󰂉", "󰢞", "󰂊", "󰂋", "󰂅"];
var FULL_ICON = "󰂅";       // 󰂅
var ALERT_ICON = "󰂃";      // 󰂃

function clamp(v, lo, hi) { return Math.max(lo, Math.min(hi, v)); }

// Glyph i shows (i+1)*10%, rounded down, so "full" means 100% and 95% reads as 90%.
function batteryIcon(percent, status) {
  var p = Number(percent) || 0;
  var i = p >= 100 ? 9 : clamp(Math.floor(p / 10) - 1, 0, 8);
  if (status === "Full" || status === "FullyCharged") return FULL_ICON;
  if (status === "Charging") return CHARGE[i];
  if (status === "Discharging" && percent <= 10) return ALERT_ICON;
  return DISCHARGE[i];
}

// The bar percentage has one switch per power state; right-click flips the current one.
function barPercentKey(status) {
  return status === "Discharging" ? "showPercentageOnBattery" : "alwaysShowPercentage";
}

function showBarPercent(status, s) {
  return status === "Discharging" ? s.showOnBattery !== false : !!s.showOnAc;
}

function fmtWatts(w) {
  var n = Number(w) || 0;
  return (n >= 10 ? n.toFixed(1) : n.toFixed(2)) + " W";
}

function fmtDuration(mins) {
  var m = Math.max(0, Math.round(Number(mins) || 0));
  if (m === 0) return "—";
  var h = Math.floor(m / 60);
  var r = m % 60;
  return h > 0 ? (h + "h " + (r < 10 ? "0" : "") + r + "m") : (r + "m");
}

// The charge limit the EC is discharging to on AC, or 0. UPower updates sooner than the poll.
function drainTarget(b) {
  return b && b.status === "Discharging" ? Number(b.drainTo) || 0 : 0;
}

// Hero meta line as { state, time }: the state is dimmed, the time is highlighted.
// e.g. DISCHARGING · 3h 48m REMAINING / CHARGING · FULL IN 1h 05m / FULLY CHARGED.
function modeParts(b) {
  if (!b || !b.present) return { state: "NO BATTERY", time: "" };
  var drainTo = drainTarget(b);
  if (drainTo) return { state: "PLUGGED IN · DRAINING TO " + drainTo + "%", time: "" };
  // Right after a plug or unplug the EC's smoothed current still reads low, so any
  // estimate would be hours too long.
  if (b.estimating && (b.status === "Discharging" || b.status === "Charging"))
    return { state: b.status === "Charging" ? "CHARGING" : "DISCHARGING", time: "CALCULATING…" };
  var t = b.minutes > 0 ? fmtDuration(b.minutes) : "";
  // Discharging wins over ACAD: a bus-powered USB-C monitor keeps ACAD online after unplug.
  if (b.status === "Discharging") return { state: "DISCHARGING", time: t ? t + " REMAINING" : "" };
  if (b.status === "Full" || (b.acOnline && b.percent >= 99)) return { state: "FULLY CHARGED", time: "" };
  if (b.status === "Charging")
    return t ? { state: "CHARGING", time: "FULL IN " + t } : { state: "PLUGGED IN · CHARGING", time: "" };
  if (b.acOnline && b.status === "Not charging") return { state: "PLUGGED IN · NOT CHARGING", time: "" };
  if (b.acOnline) return { state: "PLUGGED IN", time: "" };
  return { state: "BATTERY", time: "" };
}

function modeLabel(b) {
  var m = modeParts(b);
  return m.time ? m.state + " · " + m.time : m.state;
}

// Wear level: "" healthy, "worn" below 80% (the usual cycle-life rating), "replace" below 60%.
function healthTier(health) {
  var h = Number(health) || 0;
  if (!h || h >= 80) return "";
  return h < 60 ? "replace" : "worn";
}

// Active charge cap from the plan section's `charge` state, e.g. "80%", "Off", "Auto".
function chargeLimitLabel(c, boosting) {
  if (!c || !c.kind) return "—";
  if (boosting) return "100% (boost)";
  if (c.kind === "framework") return c.tool && c.limit != null ? c.limit + "%" : "—";
  if (!c.health) return "Off";
  var p = String(c.policy || "");
  if (p === "firmware") return "Auto";
  return p ? p.split("-").pop() + "%" : "On";
}

// Which segmented power mode is active.
function activeMode(tlp) {
  if (!tlp || !tlp.installed || !tlp.active) return "";
  if (!tlp.manual) return "auto";
  return tlp.mode === "battery" ? "saver" : "full";
}

function raplLabel(rapl) {
  if (!rapl || !rapl.available) return "";
  var parts = ["CPU " + Number(rapl.package).toFixed(1) + " W"];
  if (Number(rapl.gpu) > 0.05) parts.push("GPU " + Number(rapl.gpu).toFixed(1) + " W");
  return parts.join("  ·  ");
}

function drawValue(b) {
  if (b && (b.status === "Discharging" || b.status === "Charging") && b.watts > 0) return fmtWatts(b.watts);
  return "";
}

function historyBars(points, now) {
  var end = Math.floor((Number(now) || 0) / 1000);
  if (!end) end = Math.floor(Date.now() / 1000);
  var start = end - 86400;
  var n = 96, step = 900;
  var sums = [], counts = [], chg = [];
  for (var i = 0; i < n; i++) {
    sums.push(0);
    counts.push(0);
    chg.push(false);
  }
  var rows = Array.isArray(points) ? points.slice() : [];
  rows.sort(function(a, b) { return (Number(a.t) || 0) - (Number(b.t) || 0); });
  var carry = null;
  for (var j = 0; j < rows.length; j++) {
    var t = Number(rows[j].t) || 0;
    var sample = { pct: Number(rows[j].pct) || 0, charging: !!rows[j].charging };
    if (t < start) {
      carry = sample;
      continue;
    }
    if (t > end) break;
    var idx = Math.min(n - 1, Math.floor((t - start) / step));
    sums[idx] += sample.pct;
    counts[idx] += 1;
    if (sample.charging) chg[idx] = true;
  }
  var buckets = [];
  for (i = 0; i < n; i++) {
    if (counts[i] > 0) {
      carry = { pct: sums[i] / counts[i], charging: chg[i] };
      buckets.push({ pct: carry.pct, charging: carry.charging });
    } else if (carry) {
      buckets.push({ pct: carry.pct, charging: carry.charging });
    } else {
      buckets.push({ pct: -1, charging: false });
    }
  }
  return buckets;
}

// ---------- Sleep-drain tracking (extras/sleep-tracking) -----------------

function fmtSleepDuration(sec) {
  var s = Math.max(0, Math.round(Number(sec) || 0));
  var h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  if (h > 0) return h + "h " + (m < 10 ? "0" : "") + m + "m";
  if (m > 0) return m + "m";
  return s + "s";
}

var IDLE_STOPS = [0, 60, 120, 180, 300, 600, 900, 1200, 1800];
var HIBERNATE_STOPS = [
  0, 60, 120, 180, 240, 300, 600, 900, 1200, 1500, 1800, 2700,
  3600, 7200, 10800, 14400, 18000
];
var LID_OPTIONS = [
  { value: "nothing", label: "Do nothing" },
  { value: "display-off", label: "Displays off" },
  { value: "suspend", label: "Sleep" },
  { value: "hibernate", label: "Hibernate" }
];
var PROFILE_OPTIONS = [
  { value: "power-saver", label: "󰌪 Power saver" },
  { value: "balanced", label: "󰊚 Balanced" },
  { value: "performance", label: "󰓅 Performance" }
];

// "Never", "5 minutes", "2 hours"; values off the whole-unit grid read "2 min 30 s".
function fmtIdleLong(sec) {
  var s = Math.max(0, Math.round(Number(sec) || 0));
  if (s === 0) return "Never";
  var h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  var units = [[h, "hour", "h"], [m, "minute", "min"], [r, "second", "s"]]
    .filter(function(u) { return u[0] > 0; });
  if (units.length === 1)
    return units[0][0] + " " + units[0][1] + (units[0][0] === 1 ? "" : "s");
  return units.map(function(u) { return u[0] + " " + u[2]; }).join(" ");
}

// Dropdown options for `stops`, plus the saved value in order when it is not a stop.
function idleOptions(stops, current) {
  var cur = Math.max(0, Math.round(Number(current) || 0));
  var values = stops.indexOf(cur) >= 0 ? stops.slice()
    : stops.concat([cur]).sort(function(a, b) { return a - b; });
  return values.map(function(v) { return { value: String(v), label: fmtIdleLong(v) }; });
}

// Displays going off first would close the saver, so it counts as Never.
// Mirrors effective_saver in ops.py.
function effectiveSaver(saver, displaysOff) {
  var s = Number(saver) || 0, d = Number(displaysOff) || 0;
  return d > 0 && d <= s ? 0 : s;
}

function saverOptions(stops, saver, displaysOff) {
  var d = Number(displaysOff) || 0;
  return idleOptions(stops.filter(function(s) { return d === 0 || s < d; }),
                     effectiveSaver(saver, d));
}

function fmtPctPerHour(v) {
  return (Number(v) || 0).toFixed(2) + "%/hr";
}

// Coarse "how long ago" for the last-sleep timestamp; ISO string in, words out.
function fmtAgo(iso) {
  var t = Date.parse(iso || "");
  if (isNaN(t)) return "";
  var mins = Math.max(0, Math.round((Date.now() - t) / 60000));
  if (mins < 1) return "just now";
  if (mins < 60) return mins + "m ago";
  var hrs = Math.round(mins / 60);
  if (hrs < 48) return hrs + "h ago";
  return Math.round(hrs / 24) + "d ago";
}

// Null until at least one real sleep has been recorded.
function sleepSummary(s) {
  if (!s || !s.samples) return null;
  return {
    ago: fmtAgo(s.lastAt),
    duration: fmtSleepDuration(s.lastDurationSec),
    lastRate: fmtPctPerHour(s.lastPctPerHour),
    lastWatts: fmtWatts(s.lastWatts),
    avgRate: fmtPctPerHour(s.avgPctPerHour),
    avgWatts: fmtWatts(s.avgWatts),
    samples: Number(s.samples) || 0
  };
}

// ---------- Wi-Fi suspend-fix detector (extras/wifi-suspend-fix) ---------
// Shown only when the exact BCM4377b PM-timeout bug has actually been
// observed on this machine (see bin/battery-plus-data), not just when the
// chip is present.
function showWifiFixBanner(w) {
  return false;
}

var WIFI_FIX_INFO = {
  title: "Wi-Fi suspend bug detected",
  help: "Known issue on the Intel MacBook Air 9,1 (T2): leaving the brcmfmac "
      + "driver bound during suspend times out its PCI power-management call "
      + "(errno -5), which aborts suspend and can crash-loop it continuously — "
      + "draining the battery in hours instead of days. The fix unloads the "
      + "driver before suspend and reloads it after resume, and stops the card "
      + "being armed as a wakeup source."
};

function wifiFixBlurb(w) {
  var n = (w && w.failCount) || 0;
  return "Suspend has failed " + n + " time" + (n === 1 ? "" : "s")
       + " at this Wi-Fi adapter.";
}

// Device-power-saving rows: what each toggle does, plus the tradeoff to weigh
// before turning it on. `blurb` shows under the label; `help` opens on the (i).
var TWEAK_INFO = [
  {
    key: "wifi",
    title: "Wi-Fi power saving",
    blurb: "Radio sleeps between packets",
    help: "Saves roughly 0.5–1 W. Adds a few milliseconds of latency, and on some "
        + "Broadcom/Intel adapters can cause brief stalls or lower throughput on a "
        + "weak signal. Turn it off if you notice connection hiccups or video-call drops."
  },
  {
    key: "pcie",
    title: "PCIe runtime power",
    blurb: "Idle PCIe devices suspend",
    help: "One of the larger idle-power wins (~1–2 W) — SSD controller, card reader, "
        + "Thunderbolt/USB4. Rarely a device fails to resume and needs a reboot. The "
        + "installed TLP config already keeps the NVMe controller out of this."
  },
  {
    key: "usb",
    title: "USB autosuspend",
    blurb: "Idle USB devices power down, except keyboards and mice",
    help: "Saves about 0.1–0.5 W per device. Some webcams, audio interfaces, DACs and "
        + "wireless dongles drop off or take a moment to wake. If a device misbehaves, "
        + "replug it or turn this off."
  },
  {
    key: "audio",
    title: "Audio codec power save",
    blurb: "Codec sleeps a second after sound stops",
    help: "Saves ~0.4 W. You may hear a faint pop, or lose the first fraction of a "
        + "second of playback, when it wakes. Harmless — disable it if the click bothers you."
  },
  {
    key: "abm",
    title: "Panel power saving",
    blurb: "Lowers the backlight and boosts contrast",
    help: "AMD's adaptive backlight. Saves display power, but colors can look less accurate "
        + "and dark scenes can shift slightly. Travel mode turns this on for its duration."
  },
  {
    key: "effects",
    title: "Reduce effects",
    blurb: "Turns off animations and shadows",
    help: "Less work for the GPU on every window change. Windows open and move instantly "
        + "instead of animating. Travel mode turns this on for its duration."
  }
];
