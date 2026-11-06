#!/usr/bin/env python3
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from parity_env import Sandbox, assert_same, build_rust  # noqa: E402


def setUpModule():
    build_rust()


class GpuParity(unittest.TestCase):
    def test_suspended_discrete_gpu(self):
        with Sandbox() as s:
            s.pci("0000:c1:00.0", boot_vga="1")
            s.pci("0000:c2:00.0", boot_vga="0", runtime="suspended")
            assert_same(s.python("gpu"), s.rust("gpu"))
            self.assertEqual(json.loads(s.rust("gpu").stdout), {"present": True, "asleep": True})

    def test_optimus_card_without_boot_vga(self):
        with Sandbox() as s:
            s.pci("0000:c2:00.0", cls="0x030200", driver="nvidia", runtime="active")
            assert_same(s.python("gpu"), s.rust("gpu"))

    def test_no_gpu(self):
        with Sandbox() as s:
            assert_same(s.python("gpu"), s.rust("gpu"))


class ConfigParity(unittest.TestCase):
    CASES = ["", "{", '{"ac": 5}', '{"ac": {"profile": "auto", "lid": "system-default"}}',
             '{"chargeLimit": "abc"}', '{"chargeLimit": 90, "boost": true, "battery": {"lock": 60}}']

    def test_config_matches_for_every_shape(self):
        for raw in self.CASES:
            with self.subTest(raw=raw), Sandbox() as s:
                s.profiles(["power-saver", "balanced", "performance"])
                s.discharging(percent=70)
                s.write("home/.config/omarchy/omarchy-power-hub.json", raw)
                rust = s.rust("config")
                self.assertEqual(rust.code, 0)
                json.loads(rust.stdout)
                if raw != '{"ac": 5}':
                    assert_same(s.python("config"), rust)

    def test_default_ac_profile_needs_performance_to_be_offered(self):
        for names in (["power-saver", "balanced"], ["power-saver", "balanced", "performance"]):
            with self.subTest(names=names), Sandbox() as s:
                s.profiles(names)
                assert_same(s.python("config"), s.rust("config"))


class PowerSourceParity(unittest.TestCase):
    def same(self, s, expected_ac, side, expected_drain=0):
        for verb in ("ac", "drain-to", "config"):
            assert_same(s.python(verb), s.rust(verb))
        self.assertEqual(s.rust("ac").stdout.strip(), str(expected_ac))
        self.assertEqual(s.rust("drain-to").stdout.strip(), str(expected_drain))
        self.assertEqual(json.loads(s.rust("config").stdout)["side"], side)

    def test_ucsi_under_15_watts_counts_as_battery(self):
        with Sandbox() as s:
            s.profiles(["balanced"])
            s.ucsi("USBC000:001", True, 5, 2)
            self.same(s, 0, "battery")

    def test_ucsi_65_watt_source_is_ac(self):
        with Sandbox() as s:
            s.profiles(["balanced"])
            s.ucsi("USBC000:001", True, 20, 3.25)
            s.ucsi("USBC000:002", False, 5, 0)
            self.same(s, 1, "ac")

    def test_no_ucsi_falls_back_to_upower(self):
        for flag, ac, side in (("true", 0, "battery"), ("false", 1, "ac")):
            with self.subTest(on_battery=flag), Sandbox() as s:
                s.profiles(["balanced"])
                s.stub("busctl", "OnBattery", 0, f"b {flag}\n")
                self.same(s, ac, side)

    def test_framework_draining_to_limit_stays_on_ac_side(self):
        with Sandbox() as s:
            s.profiles(["balanced"])
            s.framework()
            s.discharging(percent=90)
            s.ucsi("USBC000:001", True, 20, 3.25)
            s.write("home/.config/omarchy/omarchy-power-hub.json", '{"chargeLimit": 80}')
            self.same(s, 1, "ac", 80)

    def test_discharging_below_limit_is_battery_side(self):
        with Sandbox() as s:
            s.profiles(["balanced"])
            s.framework()
            s.discharging(percent=50)
            s.ucsi("USBC000:001", True, 20, 3.25)
            self.same(s, 1, "battery")


if __name__ == "__main__":
    unittest.main()
