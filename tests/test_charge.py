#!/usr/bin/env python3
import os
import pathlib
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402


class ParseBusctlTests(unittest.TestCase):
    def test_bool_string_uint(self):
        self.assertEqual(ops.parse_busctl_value('b true'), True)
        self.assertEqual(ops.parse_busctl_value('b false'), False)
        self.assertEqual(ops.parse_busctl_value('u 80'), 80)
        self.assertEqual(ops.parse_busctl_value('t 3'), 3)
        self.assertEqual(ops.parse_busctl_value('s "75-80"'), "75-80")


class UPowerHealthTests(unittest.TestCase):
    def test_unsupported_when_no_threshold_devices(self):
        def run(args, timeout=4):
            if args[:2] == ["upower", "-e"]:
                return 0, "/org/freedesktop/UPower/devices/battery_BAT0\n"
            if args[-1] == "ChargeThresholdSupported":
                return 0, "b false\n"
            return 1, ""

        st = ops.upower_health_status(run)
        self.assertEqual(st["kind"], "none")
        self.assertFalse(st["available"])

    def test_enabled_start_end_policy(self):
        def run(args, timeout=4):
            if args[:2] == ["upower", "-e"]:
                return 0, "/org/freedesktop/UPower/devices/battery_BAT0\n/org/freedesktop/UPower/devices/DisplayDevice\n"
            prop = args[-1]
            return 0, {
                "ChargeThresholdSupported": "b true",
                "ChargeThresholdEnabled": "b true",
                "ChargeThresholdSettingsSupported": "t 3",
                "ChargeStartThreshold": "u 75",
                "ChargeEndThreshold": "u 80",
            }.get(prop, "b false")

        st = ops.upower_health_status(run)
        self.assertEqual(st["kind"], "upower")
        self.assertTrue(st["available"])
        self.assertTrue(st["health"])
        self.assertEqual(st["policy"], "75-80")

    def test_enable_calls_each_supported_battery(self):
        calls = []

        def run(args, timeout=4):
            calls.append(list(args))
            if args[:2] == ["upower", "-e"]:
                return 0, "/org/freedesktop/UPower/devices/battery_BAT0\n"
            if args[-1] == "ChargeThresholdSupported":
                return 0, "b true\n"
            if args[1] == "call":
                return 0, ""
            return 0, "b false\n"

        err = ops.set_health(True, run)
        self.assertEqual(err, "")
        self.assertTrue(any("EnableChargeThreshold" in c and "true" in c for c in calls))


class FrameworkChargeTests(unittest.TestCase):
    def setUp(self):
        self._fw = ops.framework
        self._which = ops.shutil.which
        self._run = ops.run
        self._helper = ops.helper_bin
        ops.helper_bin = lambda: ops.pathlib.Path("/helper/ops-priv")

    def tearDown(self):
        ops.framework = self._fw
        ops.shutil.which = self._which
        ops.run = self._run
        ops.helper_bin = self._helper

    def test_framework_prefers_ec_not_upower(self):
        ops.framework = lambda: True
        ops.shutil.which = lambda name: "/usr/bin/framework_tool" if name == "framework_tool" else None
        ops.run = lambda args, timeout=4: (0, "Minimum 0%, Maximum 80%\n")
        st = ops.charge_status()
        self.assertEqual(st["kind"], "framework")
        self.assertEqual(st["limit"], 80)
        self.assertTrue(st["tool"])

    def test_reads_limit_through_helper(self):
        calls = []
        ops.framework = lambda: True
        ops.shutil.which = lambda name: "/usr/bin/framework_tool"
        ops.run = lambda args, timeout=4: calls.append(list(args)) or (0, "Maximum charge limit: 80\n")
        ops.charge_status()
        self.assertEqual(calls, [["sudo", "-n", "/helper/ops-priv", "charge-limit"]])

    def test_missing_helper_needs_permission(self):
        ops.framework = lambda: True
        ops.shutil.which = lambda name: "/usr/bin/framework_tool"
        ops.helper_bin = lambda: None
        st = ops.charge_status()
        self.assertEqual((st["reason"], st["limit"]), ("permission", None))

    def test_sudo_password_error_is_not_shown(self):
        ops.framework = lambda: True
        ops.shutil.which = lambda name: "/usr/bin/framework_tool"
        ops.run = lambda args, timeout=4: (1, "sudo: a password is required\n")
        st = ops.charge_status()
        self.assertEqual((st["reason"], st["limit"]), ("permission", None))

    def test_non_framework_uses_upower(self):
        ops.framework = lambda: False
        ops.shutil.which = lambda name: None
        ops.run = lambda args, timeout=4: (
            (0, "/org/freedesktop/UPower/devices/battery_BAT0\n")
            if args[:2] == ["upower", "-e"]
            else (0, "b true\n") if args[-1] == "ChargeThresholdSupported"
            else (0, "b false\n") if args[-1] == "ChargeThresholdEnabled"
            else (0, "t 2\n") if args[-1] == "ChargeThresholdSettingsSupported"
            else (0, "u 80\n") if args[-1] == "ChargeEndThreshold"
            else (1, "")
        )
        st = ops.charge_status()
        self.assertEqual(st["kind"], "upower")
        self.assertFalse(st["health"])
        self.assertEqual(st["policy"], "80")


class ReapplyTests(unittest.TestCase):
    def test_writes_saved_limit_when_ec_forgot(self):
        calls = []

        def run(args, timeout=4):
            calls.append(list(args))
            return 0, ""

        ch = {"kind": "framework", "available": True, "tool": True, "limit": 100}
        out = ops.maybe_reapply_framework_limit(
            {"chargeLimit": 80, "boost": False},
            ch,
            run,
            lambda: "/helper/ops-priv",
        )
        self.assertEqual(out["limit"], 80)
        self.assertEqual(calls, [["sudo", "-n", "/helper/ops-priv", "charge-limit", "80"]])

    def test_skips_while_boost_on(self):
        calls = []
        ops.maybe_reapply_framework_limit(
            {"chargeLimit": 80, "boost": True},
            {"kind": "framework", "available": True, "tool": True, "limit": 100},
            lambda args, timeout=4: calls.append(args) or (0, ""),
            lambda: "/helper/ops-priv",
        )
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
