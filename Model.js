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
// Mirrors effective_saver in src/settings.rs.
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

// ---------- On resume ----------

var RESUME_GROUPS = ["WHEN", "UPDATE", "AFTER", "SCRIPT"];
var RESUME_LOG = "~/.local/state/omarchy-power-hub/resume.log";
var RESUME_ROWS = [
  { key: "wake", title: "Wake from sleep", kind: "switch", group: "WHEN" },
  { key: "displays", title: "Displays back on", kind: "switch", group: "WHEN" },
  { key: "unlock", title: "Unlock", kind: "switch", group: "WHEN" },
  { key: "awayMinutes", title: "Away at least", kind: "spin", group: "WHEN", step: 5, max: 1440, unit: "min" },
  { key: "everyHours", title: "At most every", kind: "spin", group: "WHEN", step: 1, max: 168, unit: "h" },
  { key: "omarchy", title: "Omarchy", kind: "switch", group: "UPDATE" },
  { key: "plugins", title: "Omarchy plugins", kind: "switch", group: "UPDATE" },
  { key: "leanCtx", title: "lean-ctx", kind: "switch", group: "UPDATE", tool: "leanCtx" },
  { key: "rulesync", title: "Rulesync", kind: "switch", group: "UPDATE", tool: "rulesync" },
  { key: "skillsManager", title: "Skills Manager", kind: "switch", group: "UPDATE", tool: "skillsManager" },
  { key: "t3code", title: "T3 Code", kind: "seg", group: "UPDATE", tool: "t3code" },
  { key: "restartShell", title: "Restart shell", kind: "switch", group: "AFTER" },
  { key: "sessionRestore", title: "Session Restore", kind: "switch", group: "AFTER", tool: "hyprPersist" },
  { key: "script", title: "Run this script instead", kind: "text", group: "SCRIPT" }
];

// Updates a run would do: tools that are off, not installed, or covered by Omarchy update don't count.
function resumeSummary(r) {
  if (!r || !r.settings) return "";
  var s = r.settings, i = r.installed || {};
  if (!s.triggers.length) return "Off";
  var when = "on " + s.triggers.join(", ");
  if (s.script) return "script · " + when;
  var n = [s.omarchy, s.plugins, s.leanCtx && i.leanCtx, !s.omarchy && s.rulesync && i.rulesync,
           !s.omarchy && s.skillsManager && i.skillsManager, s.t3code !== "off" && i.t3code].filter(Boolean).length;
  return n + (n === 1 ? " update" : " updates") + " · " + when;
}

function resumeRows(r) {
  var s = r.settings, i = r.installed || {};
  var off = !s.triggers.length;
  var needs = {
    awayMinutes: s.triggers.indexOf("displays") >= 0 || s.triggers.indexOf("unlock") >= 0,
    restartShell: s.omarchy || s.plugins,
    sessionRestore: s.t3code === "restart"
  };
  return RESUME_ROWS.map(function(d, n) {
    var trigger = d.key === "wake" || d.key === "displays" || d.key === "unlock", when = d.group === "WHEN";
    var enabled = trigger || (!off && (!s.script || when || d.group === "SCRIPT") && needs[d.key] !== false);
    return {
      key: d.key, title: d.title, kind: d.kind, group: d.group, first: n === 0 || RESUME_ROWS[n - 1].group !== d.group, step: d.step, max: d.max, unit: d.unit,
      value: trigger && d.kind === "switch" ? s.triggers.indexOf(d.key) >= 0 : s[d.key],
      visible: !d.tool || !!i[d.tool],
      enabled: enabled,
      locked: s.omarchy && (d.key === "rulesync" || d.key === "skillsManager")
    };
  });
}

// The comma list `resume triggers` takes after switching one trigger.
function resumeTriggers(r, key, on) {
  return ["wake", "displays", "unlock"].filter(function(t) {
    return t === key ? on : r.settings.triggers.indexOf(t) >= 0;
  }).join(",");
}

// A spin row's next value, one step either way.
function resumeStep(row, dir) {
  return clamp(row.value + dir * row.step, row.key === "awayMinutes" ? row.step : 1, row.max);
}

function resumeLast(r) {
  if (!r) return "";
  if (r.lastError) return "Last run failed · " + RESUME_LOG;
  return r.lastRun ? "Last run " + fmtAgo(new Date(r.lastRun * 1000).toISOString()) : "";
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

// Device-power-saving rows: what each toggle does, plus the tradeoff to weigh
// before turning it on. `help` opens on the (i); `framework` rows sit in the Framework group.
var TWEAK_INFO = [
  {
    key: "wifi",
    title: "Wi-Fi power saving",
    help: "Saves roughly 0.5–1 W. Adds a few milliseconds of latency, and on some "
        + "Broadcom/Intel adapters can cause brief stalls or lower throughput on a "
        + "weak signal. Turn it off if you notice connection hiccups or video-call drops."
  },
  {
    key: "pcie",
    title: "PCIe runtime power",
    help: "One of the larger idle-power wins (~1–2 W) — SSD controller, card reader, "
        + "Thunderbolt/USB4. Rarely a device fails to resume and needs a reboot. The "
        + "installed TLP config already keeps the NVMe controller out of this."
  },
  {
    key: "usb",
    title: "USB autosuspend",
    help: "Saves about 0.1–0.5 W per device. Some webcams, audio interfaces, DACs and "
        + "wireless dongles drop off or take a moment to wake. If a device misbehaves, "
        + "replug it or turn this off."
  },
  {
    key: "audio",
    title: "Audio codec power save",
    help: "Saves ~0.4 W. You may hear a faint pop, or lose the first fraction of a "
        + "second of playback, when it wakes. Harmless — disable it if the click bothers you."
  },
  {
    key: "abm",
    optional: true,
    title: "Panel power saving",
    help: "AMD's adaptive backlight. Saves display power, but colors can look less accurate "
        + "and dark scenes can shift slightly. Travel mode turns this on for its duration."
  },
  {
    key: "effects",
    title: "Reduce effects",
    help: "Less work for the GPU on every window change. Windows open and move instantly "
        + "instead of animating. Travel mode turns this on for its duration."
  },
  {
    key: "daylight",
    framework: true,
    setting: true,
    title: "Keyboard light off in daylight",
    help: "When the displays wake at 100 lux or more, the keyboard light stays off. In a "
        + "darker room it returns to its level. The light sensor is only read at wake."
  }
];
