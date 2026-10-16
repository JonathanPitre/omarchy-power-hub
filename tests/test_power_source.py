#!/usr/bin/env python3
import os
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402


def supply(root, name, online, volts, amps):
    d = root / name
    d.mkdir()
    (d / "online").write_text(f"{online}\n")
    (d / "voltage_now").write_text(f"{int(volts * 1e6)}\n")
    (d / "current_now").write_text(f"{int(amps * 1e6)}\n")


class PowerSourceTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)
        self._saved_ps = ops.POWER_SUPPLY
        ops.POWER_SUPPLY = self.root

    def tearDown(self):
        ops.POWER_SUPPLY = self._saved_ps
        self._tmp.cleanup()

    def test_charger_counts_as_ac(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        supply(self.root, "ucsi-source-psy-USBC000:005", 1, 12, 0.42)
        self.assertEqual(ops.ucsi_source_watts(), 100)
        (self.root / "BAT1").mkdir()
        (self.root / "BAT1" / "status").write_text("Charging\n")
        self.assertEqual(ops.active_side(), "ac")

    def test_monitor_feed_alone_is_battery(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 0, 0, 0)
        supply(self.root, "ucsi-source-psy-USBC000:005", 1, 12, 0.42)
        self.assertLess(ops.ucsi_source_watts(), ops.MIN_ADAPTER_WATTS)
        self.assertTrue(ops.on_battery())

    def test_no_ucsi_defers_to_upower(self):
        self.assertIsNone(ops.ucsi_source_watts())

    def test_discharging_on_brick_uses_battery_plan(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        (self.root / "BAT1").mkdir()
        (self.root / "BAT1" / "status").write_text("Discharging\n")
        self.assertFalse(ops.on_battery())
        self.assertTrue(ops.pack_discharging())
        self.assertEqual(ops.active_side(), "battery")


if __name__ == "__main__":
    unittest.main()
