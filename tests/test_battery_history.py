#!/usr/bin/env python3
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402


class ParseUpowerHistoryTests(unittest.TestCase):
    def test_empty_reply(self):
        self.assertEqual(ops.parse_upower_history("a(udu) 0"), [])

    def test_mixed_charging_and_discharging(self):
        raw = (
            "a(udu) 5 1790536632 90 0 1790536507 86.5 1 "
            "1790536259 86 2 1790515183 90 4 1790515183 0 0"
        )
        rows = ops.parse_upower_history(raw)
        self.assertEqual(len(rows), 5)
        self.assertEqual(rows[0], {"t": 1790536632, "pct": 90.0, "charging": False})
        self.assertEqual(rows[1], {"t": 1790536507, "pct": 86.5, "charging": True})
        self.assertEqual(rows[2], {"t": 1790536259, "pct": 86.0, "charging": False})
        self.assertEqual(rows[3], {"t": 1790515183, "pct": 90.0, "charging": False})
        self.assertEqual(rows[4], {"t": 1790515183, "pct": 0.0, "charging": False})

    def test_malformed_reply(self):
        self.assertEqual(ops.parse_upower_history(""), [])
        self.assertEqual(ops.parse_upower_history("s nope"), [])
        self.assertEqual(ops.parse_upower_history("a(udu) 2 1 2"), [])


class BatteryHistoryCommandTests(unittest.TestCase):
    def test_calls_get_history_on_first_battery(self):
        calls = []

        def run(args, timeout=4):
            calls.append(list(args))
            if args[:2] == ["upower", "-e"]:
                return 0, (
                    "/org/freedesktop/UPower/devices/DisplayDevice\n"
                    "/org/freedesktop/UPower/devices/battery_BAT1\n"
                )
            if args[1] == "call" and args[-4:] == ["suu", "charge", "172800", "400"]:
                return 0, "a(udu) 1 100 80 1\n"
            return 1, ""

        rows = ops.battery_history(run)
        self.assertEqual(rows, [{"t": 100, "pct": 80.0, "charging": True}])
        self.assertTrue(any(
            c[0] == "busctl" and c[1] == "call" and any("battery_BAT1" in x for x in c)
            for c in calls
        ))

    def test_no_battery_returns_empty(self):
        def run(args, timeout=4):
            if args[:2] == ["upower", "-e"]:
                return 0, "/org/freedesktop/UPower/devices/DisplayDevice\n"
            return 1, ""

        self.assertEqual(ops.battery_history(run), [])


if __name__ == "__main__":
    unittest.main()
