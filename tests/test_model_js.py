#!/usr/bin/env python3
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
MODEL = ROOT / "Model.js"


class ModelJsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not shutil.which("node"):
            raise unittest.SkipTest("node not installed")
        src = MODEL.read_text()
        src = src.replace(".pragma library", "")
        cls._src = src

    def _run(self, body):
        script = self._src + "\n" + body + "\n"
        with tempfile.NamedTemporaryFile("w", suffix=".js", delete=False) as f:
            f.write(script)
            path = f.name
        try:
            out = subprocess.check_output(["node", path], text=True, timeout=5)
        finally:
            pathlib.Path(path).unlink(missing_ok=True)
        return json.loads(out)

    def test_battery_icon_shows_full_only_at_100(self):
        body = r'''
          console.log(JSON.stringify({
            d95: batteryIcon(95, "Discharging") === DISCHARGE[8],
            d100: batteryIcon(100, "Discharging") === DISCHARGE[9],
            c95: batteryIcon(95, "Charging") === CHARGE[8],
            d15: batteryIcon(15, "Discharging") === DISCHARGE[0],
            d25: batteryIcon(25, "Discharging") === DISCHARGE[1],
            alert: batteryIcon(8, "Discharging") === ALERT_ICON,
            full: batteryIcon(97, "Full") === FULL_ICON
          }));
        '''
        self.assertEqual(self._run(body), {"d95": True, "d100": True, "c95": True, "d15": True,
                                           "d25": True, "alert": True, "full": True})

    def test_bar_percent_toggles_on_battery_too(self):
        body = r'''
          var on = { showOnBattery: true, showOnAc: false };
          console.log(JSON.stringify({
            battery: barPercentKey("Discharging"),
            ac: barPercentKey("Charging"),
            shownOnBattery: showBarPercent("Discharging", on),
            hiddenOnAc: showBarPercent("Full", on),
            hiddenOnBattery: showBarPercent("Discharging", { showOnBattery: false, showOnAc: false })
          }));
        '''
        self.assertEqual(self._run(body), {"battery": "showPercentageOnBattery", "ac": "alwaysShowPercentage",
            "shownOnBattery": True, "hiddenOnAc": False, "hiddenOnBattery": False})

    def test_draw_value_discharging_charging_idle(self):
        body = r'''
          console.log(JSON.stringify({
            dis: drawValue({ status: "Discharging", watts: 12.3 }),
            chg: drawValue({ status: "Charging", watts: 20 }),
            idle: drawValue({ status: "Not charging", watts: 0 }),
            none: drawValue(null)
          }));
        '''
        r = self._run(body)
        self.assertEqual(r["dis"], "12.3 W")
        self.assertEqual(r["chg"], "20.0 W")
        self.assertEqual(r["idle"], "")
        self.assertEqual(r["none"], "")

    def test_mode_label_carries_time(self):
        body = r'''
          console.log(JSON.stringify({
            bat: modeLabel({ present: true, status: "Discharging", minutes: 228, percent: 62 }),
            chg: modeLabel({ present: true, status: "Charging", minutes: 65, acOnline: 1, percent: 47 }),
            full: modeLabel({ present: true, status: "Full", acOnline: 1, percent: 100 }),
            hold: modeLabel({ present: true, status: "Not charging", acOnline: 1, percent: 80 }),
            est: modeLabel({ present: true, status: "Charging", minutes: 0, acOnline: 1, percent: 47 }),
            busPowered: modeLabel({ present: true, status: "Discharging", minutes: 294, acOnline: 1, percent: 100 })
          }));
        '''
        r = self._run(body)
        self.assertEqual(r["bat"], "DISCHARGING · 3h 48m REMAINING")
        self.assertEqual(r["chg"], "CHARGING · FULL IN 1h 05m")
        self.assertEqual(r["full"], "FULLY CHARGED")
        self.assertEqual(r["hold"], "PLUGGED IN · NOT CHARGING")
        self.assertEqual(r["est"], "PLUGGED IN · CHARGING")
        # A bus-powered USB-C monitor keeps ACAD online after the adapter is unplugged.
        self.assertEqual(r["busPowered"], "DISCHARGING · 4h 54m REMAINING")

    def test_mode_parts_split_state_and_time(self):
        body = r'''
          console.log(JSON.stringify([
            modeParts({ present: true, status: "Charging", minutes: 15, acOnline: 1, percent: 95 }),
            modeParts({ present: true, status: "Discharging", minutes: 272, percent: 92 }),
            modeParts({ present: true, status: "Full", acOnline: 1, percent: 100 })
          ]));
        '''
        self.assertEqual(self._run(body), [
            {"state": "CHARGING", "time": "FULL IN 15m"},
            {"state": "DISCHARGING", "time": "4h 32m REMAINING"},
            {"state": "FULLY CHARGED", "time": ""}])

    def test_mode_parts_names_the_drain_to_the_charge_limit(self):
        body = r'''
          var drain = { present: true, status: "Discharging", minutes: 192, acOnline: 1, percent: 99, estimating: true, drainTo: 80 };
          console.log(JSON.stringify([
            modeParts(drain),
            modeParts(Object.assign({}, drain, { drainTo: 0 })),
            // The poll lags UPower, so a stale drainTo must not outlive the drain.
            modeParts({ present: true, status: "Not charging", acOnline: 1, percent: 80, drainTo: 80 }),
            drainTarget(drain),
            drainTarget({ status: "Not charging", drainTo: 80 })
          ]));
        '''
        self.assertEqual(self._run(body), [
            {"state": "PLUGGED IN · DRAINING TO 80%", "time": ""},
            {"state": "DISCHARGING", "time": "CALCULATING…"},
            {"state": "PLUGGED IN · NOT CHARGING", "time": ""},
            80, 0])

    def test_mode_parts_calculating_while_reading_settles(self):
        # The EC smooths current_now (~8 s): 30 s after unplug a 28 W load first reads
        # 9.9 W, so the first estimate was 431 min instead of ~150.
        body = r'''
          console.log(JSON.stringify([
            modeParts({ present: true, status: "Discharging", minutes: 431, percent: 92, estimating: true }),
            modeParts({ present: true, status: "Charging", minutes: 526, acOnline: 1, percent: 92, estimating: true }),
            modeParts({ present: true, status: "Discharging", minutes: 150, percent: 92, estimating: false })
          ]));
        '''
        self.assertEqual(self._run(body), [
            {"state": "DISCHARGING", "time": "CALCULATING…"},
            {"state": "CHARGING", "time": "CALCULATING…"},
            {"state": "DISCHARGING", "time": "2h 30m REMAINING"}])

    def test_health_tier(self):
        body = r'''
          console.log(JSON.stringify([healthTier(93), healthTier(80), healthTier(79), healthTier(60), healthTier(59), healthTier(0)]));
        '''
        self.assertEqual(self._run(body), ["", "", "worn", "worn", "replace", ""])

    def test_charge_limit_label(self):
        body = r'''
          console.log(JSON.stringify({
            fw: chargeLimitLabel({ kind: "framework", tool: true, limit: 80 }, false),
            fwMissing: chargeLimitLabel({ kind: "framework", tool: false, limit: null }, false),
            boost: chargeLimitLabel({ kind: "framework", tool: true, limit: 80 }, true),
            off: chargeLimitLabel({ kind: "upower", health: false, policy: "80" }, false),
            range: chargeLimitLabel({ kind: "upower", health: true, policy: "75-80" }, false),
            auto: chargeLimitLabel({ kind: "upower", health: true, policy: "firmware" }, false),
            none: chargeLimitLabel(undefined, false)
          }));
        '''
        self.assertEqual(self._run(body), {"fw": "80%", "fwMissing": "—", "boost": "100% (boost)",
            "off": "Off", "range": "80%", "auto": "Auto", "none": "—"})

    def test_history_bars_empty_are_unknown(self):
        body = r'''
          var now = 1000000000000;
          var bars = historyBars([], now);
          console.log(JSON.stringify({ n: bars.length, first: bars[0], last: bars[95] }));
        '''
        r = self._run(body)
        self.assertEqual(r["n"], 96)
        self.assertEqual(r["first"]["pct"], -1)
        self.assertEqual(r["last"]["pct"], -1)

    def test_history_bars_carry_forward_after_first_sample(self):
        body = r'''
          var now = 1000000000000;
          var end = Math.floor(now / 1000);
          var start = end - 86400;
          var t = start + 1800;
          var bars = historyBars([{ t: t, pct: 40, charging: false }], now);
          var filled = bars.filter(function(b) { return b.pct >= 0; }).length;
          console.log(JSON.stringify({
            n: bars.length,
            b0: bars[0].pct,
            b1: bars[1].pct,
            b2: bars[2].pct,
            last: bars[95].pct,
            filled: filled
          }));
        '''
        r = self._run(body)
        self.assertEqual(r["n"], 96)
        self.assertEqual(r["b0"], -1)
        self.assertEqual(r["b1"], -1)
        self.assertEqual(r["b2"], 40)
        self.assertEqual(r["last"], 40)
        self.assertEqual(r["filled"], 94)

    def test_history_bars_charging_marks_its_buckets(self):
        body = r'''
          var now = 1000000000000;
          var end = Math.floor(now / 1000);
          var start = end - 86400;
          var bars = historyBars([
            { t: start + 100, pct: 50, charging: false },
            { t: start + 3600, pct: 70, charging: true }
          ], now);
          var chg = [];
          for (var i = 0; i < bars.length; i++) if (bars[i].charging) chg.push(i);
          console.log(JSON.stringify({ chg: chg, pctHour1: bars[4].pct }));
        '''
        r = self._run(body)
        self.assertIn(4, r["chg"])
        self.assertEqual(r["pctHour1"], 70)


if __name__ == "__main__":
    unittest.main()
