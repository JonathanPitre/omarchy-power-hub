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

    def _pack(self, status, capacity):
        (self.root / "BAT1").mkdir()
        (self.root / "BAT1" / "status").write_text(status + "\n")
        (self.root / "BAT1" / "capacity").write_text(f"{capacity}\n")

    def _framework(self, limit=80, boost=False):
        saved = ops.framework
        ops.framework = lambda: True
        self.addCleanup(setattr, ops, "framework", saved)
        return {"chargeLimit": limit, "boost": boost}

    def test_ec_drain_to_limit_on_brick_stays_ac(self):
        # Lowering the limit below the charge makes the EC discharge on a healthy adapter.
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        self._pack("Discharging", 98)
        self.assertEqual(ops.active_side(self._framework()), "ac")

    def test_monitor_feed_above_limit_is_battery(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 0, 0, 0)
        supply(self.root, "ucsi-source-psy-USBC000:005", 1, 12, 0.42)
        self._pack("Discharging", 98)
        self.assertEqual(ops.active_side(self._framework()), "battery")

    def test_discharging_at_limit_is_battery(self):
        # The EC holds at the limit, so any discharge there is the 800 mA fault.
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        self._pack("Discharging", 80)
        self.assertEqual(ops.active_side(self._framework()), "battery")

    def test_boost_has_no_limit_to_drain_to(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        self._pack("Discharging", 98)
        self.assertEqual(ops.active_side(self._framework(boost=True)), "battery")

    def test_other_vendors_keep_battery_plan_when_discharging(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        self._pack("Discharging", 98)
        saved = ops.framework
        ops.framework = lambda: False
        self.addCleanup(setattr, ops, "framework", saved)
        self.assertEqual(ops.active_side({"chargeLimit": 80}), "battery")

    def test_drain_to_reports_limit_only_while_ec_drains_on_ac(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 1, 20, 5)
        self._pack("Discharging", 98)
        cfg = self._framework()
        self.assertEqual(ops.drain_to(cfg), 80)
        (self.root / "BAT1" / "status").write_text("Not charging\n")
        self.assertEqual(ops.drain_to(cfg), 0)

    def test_drain_to_is_zero_on_monitor_feed(self):
        supply(self.root, "ucsi-source-psy-USBC000:005", 1, 12, 0.42)
        self._pack("Discharging", 98)
        self.assertEqual(ops.drain_to(self._framework()), 0)


class UcsiRecoveryTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.root = pathlib.Path(self._tmp.name)
        self.calls = []
        for name, value in (("POWER_SUPPLY", self.root), ("RUN", self.root / "run"),
                            ("framework", lambda: True),
                            ("helper_bin", lambda: "/helper/ops-priv")):
            self.addCleanup(setattr, ops, name, getattr(ops, name))
            setattr(ops, name, value)

    def _run(self, args, timeout=4):
        self.calls.append(args)
        return 0, ""

    def test_rebinds_once_per_boot_when_ucsi_is_missing(self):
        ops.recover_ucsi(self._run)
        ops.recover_ucsi(self._run)
        self.assertEqual(self.calls, [["sudo", "-n", "/helper/ops-priv", "ucsi-rebind"]])

    def test_leaves_a_working_driver_alone(self):
        supply(self.root, "ucsi-source-psy-USBC000:003", 0, 0, 0)
        ops.recover_ucsi(self._run)
        self.assertEqual(self.calls, [])

    def test_stale_helper_never_prompts_and_can_retry(self):
        ops.helper_bin = lambda: None
        ops.recover_ucsi(self._run)
        self.assertEqual(self.calls, [])
        ops.helper_bin = lambda: "/helper/ops-priv"
        ops.recover_ucsi(self._run)
        self.assertEqual(len(self.calls), 1)

    def test_other_vendors_are_left_alone(self):
        ops.framework = lambda: False
        ops.recover_ucsi(self._run)
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
