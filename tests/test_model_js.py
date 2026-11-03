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
        self.assertEqual(r["chg"], "—")
        self.assertEqual(r["idle"], "—")
        self.assertEqual(r["none"], "—")

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
