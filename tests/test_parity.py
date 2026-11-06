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


    def test_missing_power_profiles_daemon_defaults_ac_to_balanced(self):
        with Sandbox() as s:
            s.stub("omarchy-powerprofiles-list", "", 0, "")
            python, rust = s.python("config"), s.rust("config")
            assert_same(python, rust)
            self.assertEqual(json.loads(rust.stdout)["config"]["ac"]["profile"], "balanced")

    def test_saved_config_is_loaded(self):
        with Sandbox() as s:
            s.write("home/.config/omarchy/omarchy-power-hub.json", '{"chargeLimit": 90}')
            self.assertEqual(json.loads(s.rust("config").stdout)["config"]["chargeLimit"], 90)


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
            s.write("home/.config/omarchy/omarchy-power-hub.json", '{"chargeLimit": 85}')
            self.same(s, 1, "ac", 85)

    def test_discharging_below_limit_is_battery_side(self):
        with Sandbox() as s:
            s.profiles(["balanced"])
            s.framework()
            s.discharging(percent=50)
            s.ucsi("USBC000:001", True, 20, 3.25)
            self.same(s, 1, "battery")


UPOWER_LIST = ("/org/freedesktop/UPower/devices/line_power_AC\n/org/freedesktop/UPower/devices/battery_BAT1\n"
               "/org/freedesktop/UPower/devices/DisplayDevice\n")


class ChargeParity(unittest.TestCase):
    def helper_rule(self, s):
        return f"    (root) NOPASSWD: {s.helper_dir}/ops-priv\n"

    def test_helper_needed_when_installed_copy_differs(self):
        for kind in ("tampered", "absent"):
            with self.subTest(kind=kind), Sandbox() as s:
                if kind == "tampered":
                    s.install_helper(tamper=True)
                s.stub("sudo", "-n -l", 0, self.helper_rule(s))
                py, rs = s.python("helper-needed"), s.rust("helper-needed")
                assert_same(py, rs)
                self.assertEqual(rs.stdout.strip(), "needed")
                self.assertNotIn("sudo -n -l", rs.calls_text)

    def test_helper_ok_when_bytes_match_and_sudo_rule_present(self):
        with Sandbox() as s:
            s.install_helper()
            s.stub("sudo", "-n -l", 0, self.helper_rule(s))
            py, rs = s.python("helper-needed"), s.rust("helper-needed")
            assert_same(py, rs)
            self.assertEqual(rs.stdout.strip(), "ok")
            s.stub("sudo", "-n -l", 0, "    (root) NOPASSWD: /somewhere/else\n")
            py, rs = s.python("helper-needed"), s.rust("helper-needed")
            assert_same(py, rs)
            self.assertEqual(rs.stdout.strip(), "needed")

    def test_boost_off_prints_ok_and_forks_nothing(self):
        with Sandbox() as s:
            s.config(boost=False)
            r = s.rust("boost-check")
            self.assertEqual((r.stdout.strip(), r.calls), ("ok", []))

    def test_boost_restores_saved_limit_when_full(self):
        with Sandbox() as s:
            s.framework(); s.install_helper()
            s.stub("sudo", "charge-limit 75", 0)
            s.stub("upower", "-i", 0, "state: fully-charged\npercentage: 100%\n")
            s.config(boost=True, boostSavedLimit=75)
            py = s.python("boost-check")
            s.config(boost=True, boostSavedLimit=75)
            rs = s.rust("boost-check")
            assert_same(py, rs)
            self.assertEqual(rs.stdout.strip(), "restored")
            self.assertIn(f"sudo -n {s.helper_dir}/ops-priv charge-limit 75", rs.calls_text)
            self.assertFalse(s.read_config()["boost"])

    def test_boost_holds_below_full_and_pkexec_failure_is_reported(self):
        with Sandbox() as s:
            s.framework()
            s.stub("upower", "-i", 0, "state: charging\npercentage: 60%\n")
            s.config(boost=True)
            assert_same(s.python("boost-check"), s.rust("boost-check"))
            self.assertEqual(s.rust("boost-check").stdout.strip(), "holding")
            s.stub("upower", "-i", 0, "state: fully-charged\n")
            s.config(boost=True, boostSavedLimit=70)
            py = s.python("boost-check")
            s.config(boost=True, boostSavedLimit=70)
            rs = s.rust("boost-check")
            assert_same(py, rs)
            self.assertEqual((rs.code, rs.stdout.strip()), (1, "privilege required"))
            self.assertIn("charge-limit 70", rs.calls_text)

    def test_led_sudo_skip_when_not_framework(self):
        with Sandbox() as s:
            s.write("/sys/class/leds/chromeos:white:power/brightness", "0\n")
            s.stub("sudo", "-n -l", 0, "(root) NOPASSWD: /usr/bin/tee /sys/class/leds/chromeos:white:power/brightness\n")
            py, rs = s.python("led-sudo-needed"), s.rust("led-sudo-needed")
            assert_same(py, rs)
            self.assertEqual((rs.stdout.strip(), rs.calls), ("skip", []))

    def test_led_sudo_ok_or_needed_on_framework_with_led(self):
        with Sandbox() as s:
            s.framework()
            assert_same(s.python("led-sudo-needed"), s.rust("led-sudo-needed"))
            self.assertEqual(s.rust("led-sudo-needed").stdout.strip(), "skip")
            s.write("/sys/class/leds/chromeos:white:power/brightness", "0\n")
            assert_same(s.python("led-sudo-needed"), s.rust("led-sudo-needed"))
            self.assertEqual(s.rust("led-sudo-needed").stdout.strip(), "needed")
            s.stub("sudo", "-n -l", 0, "(root) NOPASSWD: /usr/bin/tee /sys/class/leds/chromeos:white:power/brightness\n")
            assert_same(s.python("led-sudo-needed"), s.rust("led-sudo-needed"))
            self.assertEqual(s.rust("led-sudo-needed").stdout.strip(), "ok")

    def test_battery_history_parses_upower_rows(self):
        with Sandbox() as s:
            s.stub("upower", "-e", 0, UPOWER_LIST)
            s.stub("busctl", "GetHistory", 0, "a(udu) 2 1730000000 85.5 1 1730000600 84.000000 2\n")
            assert_same(s.python("battery-history"), s.rust("battery-history"))
            self.assertEqual(json.loads(s.rust("battery-history").stdout), [
                {"t": 1730000000, "pct": 85.5, "charging": True}, {"t": 1730000600, "pct": 84.0, "charging": False}])
            self.assertIn("busctl call org.freedesktop.UPower /org/freedesktop/UPower/devices/battery_BAT1 "
                          "org.freedesktop.UPower.Device GetHistory suu charge 172800 400", s.rust("battery-history").calls_text)

    def test_battery_history_empty_without_battery(self):
        with Sandbox() as s:
            s.stub("upower", "-e", 0, "/org/freedesktop/UPower/devices/line_power_AC\n/org/freedesktop/UPower/devices/DisplayDevice\n")
            s.stub("busctl", "GetHistory", 0, "a(udu) 1 1 2 1\n")
            py, rs = s.python("battery-history"), s.rust("battery-history")
            assert_same(py, rs)
            self.assertEqual(json.loads(rs.stdout), [])
            self.assertNotIn("GetHistory", rs.calls_text)


if __name__ == "__main__":
    unittest.main()
