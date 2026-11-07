#!/usr/bin/env python3
import contextlib
import json
import os
import pathlib
import re
import signal
import subprocess
import sys
import threading
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from parity_env import DROPPED, REPO, RUST_BIN, Sandbox, assert_same, assert_same_files, both, build_rust  # noqa: E402


def setUpModule():
    build_rust()


class SandboxFixtures(unittest.TestCase):
    def test_light_fixtures_land_under_root(self):
        with Sandbox() as sb:
            sb.dmi("Framework", "Laptop 16")
            sb.power_led("none", 0, 100)
            sb.light_sensor(40, scale=0.5)
            read = lambda p: (sb.root / p).read_text()  # noqa: E731
            self.assertEqual(read("sys/class/dmi/id/product_name"), "Laptop 16\n")
            self.assertEqual(read("sys/class/leds/chromeos:white:power/max_brightness"), "100\n")
            self.assertEqual(read("sys/bus/iio/devices/iio:device0/in_illuminance_scale"), "0.5\n")


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

    def test_boost_off_forks_nothing_without_valid_saved_profiles(self):
        for raw in (None, "", "{", '{"boost": false}', '{"boost": 0, "ac": 5}'):
            with self.subTest(raw=raw), Sandbox() as s:
                if raw is not None:
                    s.write(s.CONFIG_PATH, raw)
                r = s.rust("boost-check")
                self.assertEqual((r.code, r.stdout.strip(), r.calls), (0, "ok", []))
                if raw != '{"boost": 0, "ac": 5}':  # Python crashes on a non-object side
                    assert_same(s.python("boost-check"), r)

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

    def test_boost_full_battery_spellings_follow_power_source(self):
        cases = [("percentage: 100%\n", True, "restored"), ("percentage: 100%\n", False, "holding"), ("state: fully charged\n", False, "restored")]
        for out, ac, expected in cases:
            with self.subTest(out=out, ac=ac), Sandbox() as s:
                s.framework(); s.install_helper()
                s.stub("sudo", "charge-limit 75", 0)
                s.stub("upower", "-i", 0, out)
                if ac:
                    s.ucsi("USBC000:001", True, 20, 3.25)
                else:
                    s.ucsi("USBC000:001", True, 5, 2)
                s.config(boost=True, boostSavedLimit=75)
                py = s.python("boost-check")
                s.config(boost=True, boostSavedLimit=75)
                rs = s.rust("boost-check")
                assert_same(py, rs)
                self.assertEqual(rs.stdout.strip(), expected)
                self.assertEqual(f"sudo -n {s.helper_dir}/ops-priv charge-limit 75" in rs.calls_text, expected == "restored")
                self.assertEqual(s.read_config()["boost"], expected == "holding")

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


CPU = "/sys/devices/system/cpu"
TRAVEL_FILE = "run/omarchy-power-hub/travel.json"
DIM_FILE = "run/omarchy-power-hub/quick-dim.json"
LAPTOP = {"name": "eDP-1", "width": 1920, "height": 1200, "refreshRate": 165.004, "transform": 0,
          "availableModes": ["1920x1200@165.00Hz", "1920x1200@59.94Hz", "1920x1200@48.00Hz", "1920x1080@60.00Hz"]}
SNAPSHOT_MONITOR = {"output": "eDP-1", "name": "eDP-1", "width": 1920, "height": 1200, "x": 0, "y": 0,
                    "refreshRate": 165.004, "scale": 1.25, "transform": 0}
FROZEN = {name: {"available": True, "detail": f"Frozen {name}"} for name in ("profile", "turbo", "epp", "brightness")}
FROZEN["refresh"] = {"available": False, "detail": "Laptop refresh capped at 59.94 Hz"}
TRAVEL_MALFORMED = "Travel restore point is malformed; restore ownership manually"
UNREADABLE = "Saved settings are unreadable; restore ownership manually"


def travel_snapshot(**changes):
    state = {"on": True, "complete": True, "reduceCpu": True, "source": "battery", "error": "",
             "pending": ["profile", "turbo", "brightness", "refresh"], "profile": "balanced",
             "cpu": {"turbo": True, "epp": []}, "brightness": {"monitor": "eDP-1", "before": 70},
             "monitor": SNAPSHOT_MONITOR, "controls": FROZEN}
    return {**state, **changes}


def status_of(s):
    py, rs = s.python("status"), s.rust("status")
    assert_same(py, rs)
    return json.loads(rs.stdout)


def laptop(s, brightness=55):
    s.monitors(LAPTOP)
    s.stub("omarchy", "brightness display --monitor eDP-1", 0, f"{brightness}\n")


class StatusParity(unittest.TestCase):
    def test_idle_laptop(self):
        with Sandbox() as s:
            s.profiles(["power-saver", "balanced", "performance"])
            s.config(chargeLimit=75)
            s.discharging(percent=70)
            laptop(s)
            s.write(f"{CPU}/intel_pstate/no_turbo", "0\n")
            s.write(f"{CPU}/cpu0/cpufreq/energy_performance_preference", "balance_performance\n")
            s.write(f"{CPU}/cpu0/cpufreq/energy_performance_available_preferences", "default performance balance_power power\n")
            s.stub("busctl", "CanHibernate", 0, 's "yes"\n')
            st = status_of(s)
            self.assertEqual((st["side"], st["boost"], st["framework"], st["config"]["chargeLimit"]), ("battery", False, False, 75))
            self.assertEqual(st["travel"]["phase"], "off")
            self.assertEqual(st["travel"]["controls"], {
                "profile": {"available": True, "detail": "Power saver"},
                "turbo": {"available": True, "detail": "CPU turbo off"},
                "epp": {"available": True, "detail": "Energy-saving CPU preference"},
                "brightness": {"available": True, "detail": "Laptop brightness capped at 40%"},
                "refresh": {"available": True, "detail": "Laptop refresh capped at 59.94 Hz"}})
            self.assertEqual(st["quickDim"], {"on": False, "available": True, "brightness": 55, "error": ""})
            self.assertEqual(st["hibernate"], {"available": True, "reason": ""})
            self.assertEqual(st["tweaks"], {})

    def test_idle_laptop_without_cpu_controls_or_fast_panel(self):
        with Sandbox() as s:
            s.monitors({**LAPTOP, "refreshRate": 60.0})
            st = status_of(s)
            self.assertEqual(st["travel"]["controls"]["refresh"], {"available": True, "detail": "Laptop refresh is 60 Hz (already at or below 60 Hz)"})
            self.assertEqual(st["travel"]["controls"]["brightness"], {"available": False, "detail": "Brightness cannot be read"})
            self.assertFalse(any(st["travel"]["controls"][k]["available"] for k in ("profile", "turbo", "epp")))
            s.monitors({**LAPTOP, "availableModes": ["1920x1200@165.00Hz"]})
            self.assertEqual(status_of(s)["travel"]["controls"]["refresh"], {"available": False, "detail": "No supported mode at or below 60 Hz"})

    def test_travel_on_valid_snapshot_reports_frozen_controls(self):
        with Sandbox() as s:
            laptop(s)
            s.write(TRAVEL_FILE, json.dumps(travel_snapshot()))
            st = status_of(s)
            self.assertEqual((st["travel"]["phase"], st["travel"]["reduceCpu"], st["travel"]["error"]), ("on", True, ""))
            self.assertEqual(st["travel"]["controls"], FROZEN)
            s.write(TRAVEL_FILE, json.dumps(travel_snapshot(complete=False, error="Power saver was replaced by balanced")))
            st = status_of(s)
            self.assertEqual((st["travel"]["phase"], st["travel"]["error"]), ("incomplete", "Power saver was replaced by balanced"))
            self.assertEqual(st["travel"]["controls"], FROZEN)

    def test_legacy_travel_file_and_partial_controls(self):
        with Sandbox() as s:
            s.write(TRAVEL_FILE, json.dumps({"on": True, "monitors": [], "brightness": None, "controls": {"turbo": {"available": "yes"}, "epp": {"available": False}}}))
            st = status_of(s)
            self.assertEqual((st["travel"]["phase"], st["travel"]["reduceCpu"]), ("incomplete", True))
            self.assertEqual(st["travel"]["controls"]["epp"], {"available": False, "detail": "Energy-saving CPU preference"})
            self.assertEqual(st["travel"]["controls"]["turbo"], {"available": False, "detail": "CPU turbo off"})
            s.write(TRAVEL_FILE, json.dumps({"on": True, "complete": True, "reduceCpu": 0, "monitors": []}))
            self.assertFalse(status_of(s)["travel"]["reduceCpu"])

    def test_malformed_travel_file_reports_restore_error(self):
        snapshots = {
            "not json": ("{", UNREADABLE),
            "not an object": ("[]", "Saved settings are malformed"),
            "on is not a bool": ('{"on": "yes"}', TRAVEL_MALFORMED),
            "on without a snapshot": ('{"on": true}', TRAVEL_MALFORMED),
            "pending lists an unknown token": (json.dumps(travel_snapshot(pending=["profile", "bogus"])), TRAVEL_MALFORMED),
            "complete but pending is short": (json.dumps(travel_snapshot(pending=["profile"])), TRAVEL_MALFORMED),
            "unsafe monitor name": (json.dumps(travel_snapshot(monitor={**SNAPSHOT_MONITOR, "name": "eDP 1"})), TRAVEL_MALFORMED),
            "zero scale": (json.dumps(travel_snapshot(monitor={**SNAPSHOT_MONITOR, "scale": 0})), TRAVEL_MALFORMED),
            "bool width": (json.dumps(travel_snapshot(monitor={**SNAPSHOT_MONITOR, "width": True})), TRAVEL_MALFORMED),
            "control without detail": (json.dumps(travel_snapshot(controls={**FROZEN, "turbo": {"available": True}})), TRAVEL_MALFORMED),
            "duplicate epp cpu": (json.dumps(travel_snapshot(cpu={"turbo": True, "epp": [{"cpu": 0, "before": "power", "target": None}] * 2})), TRAVEL_MALFORMED),
            "unknown tweak": (json.dumps(travel_snapshot(tweaks=["bogus"])), TRAVEL_MALFORMED),
            "cpu fields without reduceCpu": (json.dumps(travel_snapshot(reduceCpu=False)), TRAVEL_MALFORMED),
        }
        for name, (raw, error) in snapshots.items():
            with self.subTest(name), Sandbox() as s:
                s.write(TRAVEL_FILE, raw)
                st = status_of(s)
                self.assertEqual((st["travel"]["phase"], st["travel"]["error"]), ("incomplete", error))

    def test_valid_snapshots_with_epp_and_tweaks(self):
        with Sandbox() as s:
            epp = [{"cpu": 0, "before": "balance_performance", "target": "power"}, {"cpu": 1, "before": "performance", "target": None}]
            state = travel_snapshot(cpu={"turbo": None, "epp": epp}, tweaks=["abm"], monitor=None, brightness=None, profile=None,
                                    pending=["epp:0", "epp:1", "tweak:abm"], controls={**FROZEN, "refresh": {"available": False, "detail": "none"}})
            s.write(TRAVEL_FILE, json.dumps(state))
            self.assertEqual(status_of(s)["travel"]["error"], "")
            s.write(TRAVEL_FILE, json.dumps({**state, "pending": ["epp:0", "tweak:abm"]}))
            self.assertEqual(status_of(s)["travel"]["error"], TRAVEL_MALFORMED)

    def test_unreadable_quick_dim_file(self):
        with Sandbox() as s:
            laptop(s)
            (s.run / "omarchy-power-hub" / "quick-dim.json").mkdir(parents=True)
            self.assertEqual(status_of(s)["quickDim"], {"on": True, "available": True, "brightness": None, "error": UNREADABLE})
            self.assertEqual(s.rust("status").calls_text.count("omarchy brightness"), 1)  # Travel's read only

    def test_quick_dim_state_files(self):
        message = "Quick dim restore point is malformed; restore ownership manually"
        cases = {
            "on and valid": ({"on": True, "monitor": "eDP-1", "brightness": 82, "error": "stuck"}, 40, 40, "stuck"),
            "on, monitor gone": ({"on": True, "monitor": "DP-9", "brightness": 82, "error": ""}, 40, None, ""),
            "off": ({"on": False}, 55, 55, ""),
            "brightness out of range": ({"on": True, "monitor": "eDP-1", "brightness": 101}, 40, None, message),
            "brightness is a float": ({"on": True, "monitor": "eDP-1", "brightness": 82.0}, 40, None, message),
            "unsafe monitor": ({"on": True, "monitor": "a b", "brightness": 82}, 40, None, message),
            "on is a string": ({"on": "yes"}, 40, None, message),
        }
        for name, (state, shown, brightness, error) in cases.items():
            with self.subTest(name), Sandbox() as s:
                laptop(s, shown)
                s.write(DIM_FILE, json.dumps(state))
                dim = status_of(s)["quickDim"]
                self.assertEqual((dim["brightness"], dim["error"]), (brightness, error))
                self.assertEqual(dim["on"], state["on"] is True or state["on"] == "yes")

    def test_brightness_must_be_a_plain_percentage(self):
        for out in ("101", "-1", "40%", "", "abc", "1234"):
            with self.subTest(out=out), Sandbox() as s:
                laptop(s, out)
                self.assertEqual(status_of(s)["quickDim"], {"on": False, "available": False, "brightness": None, "error": ""})
        with Sandbox() as s:
            laptop(s)
            s.stub("omarchy", "brightness display --monitor eDP-1", 1, "55\n")
            self.assertIsNone(status_of(s)["quickDim"]["brightness"])

    def test_stale_framework_limit_is_reapplied(self):
        with Sandbox() as s:
            s.framework(); s.install_helper(); s.config(chargeLimit=80); s.charge_limit(90)
            assert_same(s.python("status"), s.rust("status"))
            self.assertIn("charge-limit 80", s.rust("status").calls_text)
            self.assertEqual(json.loads(s.rust("status").stdout)["charge"], {
                "kind": "framework", "available": True, "reason": "", "limit": 80, "tool": True, "health": None, "policy": None})

    def test_failed_reapply_keeps_the_stale_limit(self):
        with Sandbox() as s:
            s.framework(); s.install_helper(); s.config(chargeLimit=80)
            s.stub("sudo", "charge-limit 80", 1)
            s.charge_limit(90)
            st = status_of(s)
            self.assertEqual((st["charge"]["limit"], st["charge"]["available"]), (90, True))
            self.assertIn(f"sudo -n {s.helper_dir}/ops-priv charge-limit 80", s.rust("status").calls_text)

    def test_reapplied_limit_is_clamped_to_40_100(self):
        for saved, sent in ((30, 40), (120, 100)):
            with self.subTest(saved=saved), Sandbox() as s:
                s.framework(); s.install_helper(); s.config(chargeLimit=saved); s.charge_limit(90)
                self.assertEqual(status_of(s)["charge"]["limit"], sent)
                self.assertIn(f"charge-limit {sent}", s.rust("status").calls_text)

    def test_framework_limit_that_matches_is_left_alone(self):
        with Sandbox() as s:
            s.framework(); s.install_helper(); s.config(chargeLimit=85); s.charge_limit(85)
            st = status_of(s)
            self.assertEqual(st["charge"]["limit"], 85)
            self.assertNotIn("charge-limit 85", s.rust("status").calls_text)
            s.config(chargeLimit=70, boost=True)
            st = status_of(s)
            self.assertEqual((st["charge"]["limit"], st["boost"]), (85, True))

    def test_framework_without_helper_needs_permission(self):
        with Sandbox() as s:
            s.framework(); s.config(chargeLimit=80)
            self.assertEqual(status_of(s)["charge"], {
                "kind": "framework", "available": False, "reason": "permission", "limit": None, "tool": True, "health": None, "policy": None})
            self.assertNotIn("sudo", s.rust("status").calls_text)

    def test_framework_with_unreadable_limit(self):
        with Sandbox() as s:
            s.framework(); s.install_helper()
            s.stub("sudo", "charge-limit", 1, "sudo: a password is required\n")
            self.assertEqual(status_of(s)["charge"]["reason"], "permission")
            s.stub("sudo", "charge-limit", 0, "EC said no\n")
            self.assertEqual(status_of(s)["charge"]["reason"], "EC said no")

    def test_non_framework_reports_upower_thresholds(self):
        with Sandbox() as s:
            s.stub("upower", "-e", 0, UPOWER_LIST)
            s.stub("busctl", "ChargeThresholdSupported", 0, "b true\n")
            s.stub("busctl", "ChargeThresholdEnabled", 0, "b true\n")
            s.stub("busctl", "ChargeThresholdSettingsSupported", 0, "u 3\n")
            s.stub("busctl", "ChargeStartThreshold", 0, "u 40\n")
            s.stub("busctl", "ChargeEndThreshold", 0, "u 80\n")
            self.assertEqual(status_of(s)["charge"], {
                "kind": "upower", "available": True, "reason": "", "limit": None, "tool": False, "health": True, "policy": "40-80"})
            s.stub("busctl", "ChargeThresholdEnabled", 0, "b false\n")
            s.stub("busctl", "ChargeThresholdSettingsSupported", 0, "u 4\n")
            charge = status_of(s)["charge"]
            self.assertEqual((charge["health"], charge["policy"]), (False, "firmware"))

    def test_non_framework_without_thresholds(self):
        with Sandbox() as s:
            self.assertEqual(status_of(s)["charge"], {
                "kind": "none", "available": False, "reason": "no charge threshold", "limit": None, "tool": False, "health": None, "policy": None})

    def test_hibernate_blocked_by_lockdown_nohibernate_bitwarden_and_swap(self):
        secret = "an application holds secret memory, which blocks hibernation"
        bitwarden = "Bitwarden holds secret memory that blocks hibernation. Set SECURE_KEY_CONTAINER_BACKEND=mlock and restart Bitwarden."
        cases = {
            "lockdown": ({"/sys/kernel/security/lockdown": "none [integrity] confidentiality\n"}, "kernel lockdown is blocking hibernation"),
            "lockdown none": ({"/sys/kernel/security/lockdown": "[none] integrity confidentiality\n"}, secret),
            "nohibernate": ({"/proc/cmdline": "quiet nohibernate resume=x\n"}, "the kernel was booted with nohibernate"),
            "nohibernate as a substring": ({"/proc/cmdline": "quiet nohibernated\n"}, secret),
            "bitwarden": ({"/proc/4242/cmdline": "/opt/bitwarden/bitwarden\0--flag\0"}, bitwarden),
            "other process": ({"/proc/4242/cmdline": "/usr/bin/bash\0"}, secret),
            "non-pid entry": ({"/proc/self/cmdline": "/opt/bitwarden/bitwarden\0"}, secret),
        }
        for name, (files, reason) in cases.items():
            with self.subTest(name), Sandbox() as s:
                s.write("/sys/power/disk", "[disabled]\n")
                for path, text in files.items():
                    s.write(path, text)
                self.assertEqual(status_of(s)["hibernate"], {"available": False, "reason": reason})
                self.assertNotIn("CanHibernate", s.rust("status").calls_text)
        swap = "hibernate is not available: check swap and a resume= kernel parameter"
        for answer, ok in (('s "yes"\n', True), ('s "no"\n', False), ('s "challenge"\n', False), ('s "yes"\ns "no"\n', False), ("", False)):
            with self.subTest(answer=answer), Sandbox() as s:
                s.write("/sys/power/disk", "[platform] shutdown reboot suspend\n")
                s.stub("busctl", "CanHibernate", 0, answer)
                self.assertEqual(status_of(s)["hibernate"], {"available": ok, "reason": "" if ok else swap})

    def test_no_hyprland_socket(self):
        with Sandbox() as s:
            s.profiles(["power-saver"])
            laptop(s)
            os.remove(s.sock_path)
            st = status_of(s)
            self.assertEqual(st["quickDim"], {"on": False, "available": False, "brightness": None, "error": ""})
            self.assertEqual(st["travel"]["controls"]["refresh"], {"available": False, "detail": "No supported mode at or below 60 Hz"})
            self.assertEqual(st["travel"]["controls"]["profile"]["available"], True)
            self.assertNotIn("omarchy brightness", s.rust("status").calls_text)

    def test_tweaks_file_is_returned_as_parsed(self):
        for raw, expected in (("[1, 2]", [1, 2]), ('{"abm": true}', {"abm": True}), ("5", 5), ("null", None), ('"x"', "x"), ("{", {}), ("", {})):
            with self.subTest(raw=raw), Sandbox() as s:
                s.write("home/.config/omarchy/power-hub-tweaks.json", raw)
                self.assertEqual(status_of(s)["tweaks"], expected)

    def test_status_runs_each_probe_once(self):
        with Sandbox() as s:
            s.profiles(["power-saver", "balanced"])
            s.config()
            laptop(s)
            r = s.rust("status")
            self.assertEqual(r.calls_text.count("omarchy brightness"), 1)
            self.assertEqual(r.requests, ["j/monitors"])
            self.assertEqual(sum("net.hadess" in c for c in r.calls), 1)
            self.assertNotIn("powerprofilesctl", r.calls_text)
            self.assertNotIn("omarchy-powerprofiles-list", r.calls_text)
            self.assertNotIn("hyprctl", r.calls_text)


def data_of(s):
    py, rs = s.bash_data(), s.rust("data")
    assert_same(py, rs)
    return json.loads(rs.stdout)


class DataParity(unittest.TestCase):
    def test_discharging_energy_battery(self):
        with Sandbox() as s:
            s.battery()
            s.framework()
            s.discharging(percent=90)
            s.ucsi("USBC000:001", True, 20, 3.25)
            s.config(chargeLimit=85)
            d = data_of(s)
            self.assertEqual(d["battery"], {"present": True, "percent": 90, "status": "Discharging", "watts": 10.0,
                                            "health": 91, "whFull": 50.0, "whDesign": 55.0, "cycles": 123, "minutes": 240,
                                            "acOnline": 1, "fraction": 0.9, "drainTo": 85})
            self.assertEqual(d["rapl"], {"available": False, "package": 0, "core": 0, "gpu": 0})

    def test_charging_charge_units_battery(self):
        with Sandbox() as s:
            s.battery(charge_units=True, status="Charging", capacity=80, current_now="-1000000")
            s.stub("busctl", "OnBattery", 0, "b false\n")
            d = data_of(s)
            self.assertEqual((d["battery"]["whFull"], d["battery"]["whDesign"], d["battery"]["watts"]), (47.9, 51.3, 12.0))
            self.assertEqual((d["battery"]["minutes"], d["battery"]["acOnline"], d["battery"]["drainTo"]), (39, 1, 0))
            s.battery(charge_units=True, status="Discharging", current_now=None, current_avg="500000", power_now=None)
            s.stub("busctl", "OnBattery", 0, "b true\n")
            d = data_of(s)
            self.assertEqual((d["battery"]["watts"], d["battery"]["minutes"], d["battery"]["acOnline"]), (6.0, 399, 0))

    def test_no_battery_desktop(self):
        with Sandbox() as s:
            r = s.rust("data")
            self.assertFalse(json.loads(r.stdout)["battery"]["present"])
            assert_same(s.bash_data(), r)

    def test_garbage_in_capacity_gives_valid_json(self):
        with Sandbox() as s:
            s.battery(capacity="n/a")
            r = s.rust("data")
            self.assertEqual(r.code, 0)
            battery = json.loads(r.stdout)["battery"]
            self.assertEqual((battery["present"], battery["percent"], battery["whFull"], battery["status"]),
                             (True, 0, 50.0, "Discharging"))
            s.battery(energy_now="x", power_now="", voltage_now="1e9", cycle_count="?")
            r = s.rust("data")
            self.assertEqual(r.code, 0)
            self.assertTrue(json.loads(r.stdout)["battery"]["present"])

    def test_tlp_manual_and_last_pwr(self):
        with Sandbox() as s:
            s._write_stub("tlp")
            s.stub("systemctl", "is-active", 0)
            s.write("/run/tlp/last_pwr", "1 1700000000\n")
            tlp = data_of(s)["tlp"]
            self.assertEqual(tlp, {"installed": True, "active": True, "mode": "battery", "manual": False})
            s.write("/run/tlp/manual_mode", "0\n")
            self.assertEqual(data_of(s)["tlp"], {"installed": True, "active": True, "mode": "ac", "manual": True})
            s.write("/run/tlp/manual_mode", "2\n")
            s.write("/run/tlp/last_pwr", "0 1\n")
            self.assertEqual(data_of(s)["tlp"]["mode"], "battery")
            s.write("/run/tlp/manual_mode", "")
            self.assertEqual(data_of(s)["tlp"], {"installed": True, "active": True, "mode": "ac", "manual": False})

    def test_tlp_mode_falls_back_to_the_adapter(self):
        with Sandbox() as s:
            s.stub("busctl", "OnBattery", 0, "b true\n")
            self.assertEqual(data_of(s)["tlp"], {"installed": False, "active": False, "mode": "battery", "manual": False})
            s.stub("busctl", "OnBattery", 0, "b false\n")
            self.assertEqual(data_of(s)["tlp"]["mode"], "ac")

    def test_tweaks_counts_and_pins(self):
        with Sandbox() as s:
            s.write("/sys/class/net/wlan0/address", "00\n")
            s.stub("iw", "dev wlan0 get power_save", 0, "Power save: on\n")
            for n, mode in enumerate(("auto", "auto", "on")):
                s.write(f"/sys/bus/pci/devices/0000:00:0{n}.0/power/control", mode + "\n")
            for n, mode in enumerate(("auto", "on")):
                s.write(f"/sys/bus/usb/devices/1-{n}/power/control", mode + "\n")
            s.write("/sys/module/snd_hda_intel/parameters/power_save", "1\n")
            s.write("/sys/class/drm/card1/amdgpu/panel_power_savings", "3\n")
            s.hypr["getoption animations:enabled"] = "bool: false\nset: true\n"
            s.write("home/.config/omarchy/power-hub-tweaks.json",
                    json.dumps({"wifiPinned": True, "usbPinned": False, "audioPinned": 3, "other": 1, "wifi": "no"}))
            r = s.rust("data")
            d = data_of(s)
            self.assertEqual(d["tweaks"], {"wifi": True, "usb": False, "pcie": True, "audio": True, "abm": True,
                                           "abmAvailable": True, "effects": True, "daylight": False, "daylightAvailable": False,
                                           "wifiPinned": True, "usbPinned": False, "audioPinned": 3})
            self.assertEqual(r.requests, ["getoption animations:enabled"])
            self.assertIn("iw dev wlan0 get power_save", r.calls_text)

    def test_tweaks_default_off(self):
        with Sandbox() as s:
            s.write("/sys/class/net/wlan0/address", "00\n")
            s.write("/sys/class/drm/card1/amdgpu/panel_power_savings", "0\n")
            s.write("/sys/module/snd_hda_intel/parameters/power_save", "0\n")
            self.assertEqual(data_of(s)["tweaks"], {"wifi": False, "usb": False, "pcie": False, "audio": False,
                                                    "abm": False, "abmAvailable": True, "effects": False,
                                                    "daylight": False, "daylightAvailable": False})

    def test_daylight_option_needs_a_framework_with_a_sensor(self):
        with Sandbox() as s:
            tweaks = lambda: json.loads(s.rust("data").stdout)["tweaks"]
            s.write("/sys/bus/iio/devices/iio:device0/in_illuminance_raw", "37\n")
            self.assertFalse(tweaks()["daylightAvailable"])
            s.write("/sys/class/dmi/id/sys_vendor", "Framework\n")
            self.assertTrue(tweaks()["daylightAvailable"])
            s.write("home/.config/omarchy/power-hub-tweaks.json", json.dumps({"daylight": True}))
            self.assertTrue(tweaks()["daylight"])

    def test_tweaks_file_holding_a_list(self):
        with Sandbox() as s:
            s.write("home/.config/omarchy/power-hub-tweaks.json", "[1, 2]")
            assert_same(s.bash_data(), s.rust("data"))
            self.assertNotIn("Pinned", s.rust("data").stdout)

    def test_sleep_log_rolling_average(self):
        with Sandbox() as s:
            watts = (0.9, 1.1, 1.7, 2.2, 2.9, 3.4, 4.1)
            pph = (-3.1, -2.2, -1.9, -1.4, -1.1, -0.8, -0.6)
            rows = [f"2026-11-0{i + 1}T10:00:00+00:00 duration={600 + i} pre_cap={80 - i} post_cap={79 - i} drop_wh=0.5 "
                    f"watts={w} pct_per_hour={p}" for i, (w, p) in enumerate(zip(watts, pph))]
            rows.insert(1, "")
            s.write("/var/log/battery-plus-sleep.log", "\n".join(rows) + "\n")
            p = s.write("/usr/lib/systemd/system-sleep/battery-plus-sleep", "#!/bin/sh\n")
            p.chmod(0o755)
            sleep = data_of(s)["sleep"]
            self.assertEqual((sleep["installed"], sleep["samples"], sleep["lastAt"]), (True, 7, "2026-11-07T10:00:00+00:00"))
            self.assertEqual((sleep["lastDurationSec"], sleep["lastPreCap"], sleep["lastPostCap"]), (606, 74, 73))
            self.assertAlmostEqual(sleep["avgWatts"], 2.86)
            self.assertAlmostEqual(sleep["avgPctPerHour"], -1.16)
            self.assertEqual((sleep["lastWatts"], sleep["lastPctPerHour"]), (4.1, -0.6))

    def test_sleep_log_absent_or_blank(self):
        with Sandbox() as s:
            self.assertEqual(data_of(s)["sleep"], {"installed": False})
            s.write("/var/log/battery-plus-sleep.log", "\n\n")
            self.assertEqual(data_of(s)["sleep"], {"installed": False})

    def test_wifi_suspend_problem_detected(self):
        with Sandbox() as s:
            s.write("/sys/bus/pci/devices/0000:03:00.0/modalias", "pci:v000014E4d00004488sv00001028sd00000001bc02sc80i00\n")
            s.write("/sys/bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n")
            s.write("/sys/bus/pci/devices/0000:03:00.0/device", "0x4488\n")
            s.write("/sys/power/suspend_stats/fail", "2\n")
            s.write("/sys/power/suspend_stats/last_failed_dev", "0000:03:00.0\n")
            s.write("/sys/power/suspend_stats/last_failed_errno", "-5\n")
            self.assertEqual(data_of(s)["wifiFix"], {"chipPresent": True, "installed": False, "problemSeen": True, "failCount": 2})
            s.write("/sys/power/suspend_stats/last_failed_errno", "-22\n")
            self.assertEqual(data_of(s)["wifiFix"]["problemSeen"], False)
            s.write("/sys/power/suspend_stats/last_failed_errno", "-5\n")
            s.write("/sys/bus/pci/devices/0000:03:00.0/device", "0x4377\n")
            self.assertEqual(data_of(s)["wifiFix"]["problemSeen"], False)
            p = s.write("/usr/lib/systemd/system-sleep/battery-plus-wifi-suspend-fix", "#!/bin/sh\n")
            p.chmod(0o755)
            self.assertTrue(data_of(s)["wifiFix"]["installed"])

    def test_rapl_readable(self):
        with Sandbox() as s:
            rapl = "/sys/class/powercap/intel-rapl:0"
            s.write(f"{rapl}/energy_uj", "5000000\n")
            s.write(f"{rapl}:0/energy_uj", "100\n")
            s.write(f"{rapl}/max_energy_range_uj", "10000000\n")
            self.assertEqual(data_of(s)["rapl"], {"available": True, "package": 0, "core": 0, "gpu": 0})

            def counter_moves(run):
                s.write(f"{rapl}/energy_uj", "9000000\n")
                s.write(f"{rapl}:0/energy_uj", "1000000\n")
                s.write(f"{rapl}:0/max_energy_range_uj", "0\n")
                timer = threading.Timer(0.25, lambda: (s.write(f"{rapl}/energy_uj", "1000000\n"),
                                                       s.write(f"{rapl}:0/energy_uj", "3000000\n")))
                timer.start()
                try:
                    return run()
                finally:
                    timer.join()

            py, rs = counter_moves(s.bash_data), counter_moves(lambda: s.rust("data"))
            assert_same(py, rs)
            # package wrapped past max_energy_range (+2 J over 0.4 s); core rose 2 J with no wrap range
            self.assertEqual(json.loads(rs.stdout)["rapl"], {"available": True, "package": 5.0, "core": 5.0, "gpu": 0})

    def test_data_never_calls_lspci_or_python(self):
        with Sandbox() as s:
            forbidden = ("lspci", "python3", "hyprctl", "jq", "awk")
            for tool in forbidden:
                s._write_stub(tool)
            s.battery()
            r = s.rust("data")
            self.assertEqual(r.code, 0)
            for tool in forbidden:
                self.assertNotIn(tool, r.calls_text)
            self.assertTrue(r.calls)
            self.assertTrue(all(c.split()[0] in ("busctl", "iw", "systemctl") for c in r.calls), r.calls)

IDLE_FILE = "run/omarchy-power-hub/idle-wake.json"
STATE_FILE = "run/omarchy-power-hub/display-off.json"


def seed_cursor(s, cursor, age=0):
    path = s.write(IDLE_FILE, json.dumps({"cursor": cursor}) + "\n")
    os.utime(path, (time.time() - age,) * 2)


def poll_both(s, *args, prev=None, age=0):
    """Python then Rust, each from the same cursor file; returns the Rust result and the file it left."""
    out = []
    for run in (s.python, s.rust):
        s._target(IDLE_FILE).unlink(missing_ok=True)
        if prev is not None:
            seed_cursor(s, prev, age)
        r = run("idle-wake-poll", *args)
        out.append((r, s._target(IDLE_FILE).read_text()))
    (py, py_file), (rs, rs_file) = out
    assert_same(py, rs)
    unittest.TestCase().assertEqual(py_file, rs_file)
    return rs, rs_file


EDP = {"name": "eDP-1", "dpmsStatus": True}


def device_eval(name, enabled):
    return f'hl.device({{ name = "{name}", enabled = {enabled} }})'


class IdleParity(unittest.TestCase):
    def test_cursor_moved_wakes(self):
        with Sandbox() as s:
            s.monitors({"name": "eDP-1", "dpmsStatus": False})
            s.devices(mice=["m"], keyboards=["kb"])
            s.hypr["cursorpos"] = "500, 300\n"
            r, f = poll_both(s, "blanked", prev="10, 10")
            self.assertEqual(r.stdout.strip(), "wake")
            self.assertEqual(r.evals, [device_eval("m", "true"), device_eval("kb", "true")])
            self.assertEqual(f, '{"cursor": "500, 300"}\n')
            s.hypr["cursorpos"] = "10, 10\n"
            self.assertEqual(poll_both(s, "blanked", prev="10, 10")[0].stdout.strip(), "hold")

    def test_partly_lit_displays_ask_to_blank_again(self):
        with Sandbox() as s:
            s.monitors({"name": "eDP-1", "dpmsStatus": False}, EDP | {"name": "DP-1"})
            s.hypr["cursorpos"] = "10, 10\n"
            self.assertEqual(poll_both(s, "blanked", prev="10, 10")[0].stdout.strip(), "blank")
            self.assertEqual(poll_both(s, prev="10, 10")[0].stdout.strip(), "hold")

    def test_stale_cursor_file_does_not_wake(self):
        with Sandbox() as s:
            s.monitors({"name": "eDP-1", "dpmsStatus": False})
            s.hypr["cursorpos"] = "500, 300\n"
            self.assertEqual(poll_both(s, "blanked", prev="10, 10", age=1)[0].stdout.strip(), "wake")
            r, f = poll_both(s, "blanked", prev="10, 10", age=10)
            self.assertEqual(r.stdout.strip(), "hold")
            self.assertEqual(f, '{"cursor": "500, 300"}\n')
            self.assertEqual(poll_both(s, "blanked")[0].stdout.strip(), "hold")

    def test_lid_closed_disables_pointers_and_keyboards(self):
        with Sandbox() as s:
            s.lid("closed")
            s.devices(mice=["m1", "m2"], keyboards=["kb"])
            r, _ = poll_both(s, "blanked")
            self.assertEqual(r.stdout.strip(), "closed")
            self.assertEqual(r.evals, [device_eval(n, "false") for n in ("m1", "m2", "kb")])
            # Not blanked: the pointers stay enabled even with the lid shut.
            r, _ = poll_both(s)
            self.assertEqual((r.stdout.strip(), r.evals), ("closed", [device_eval(n, "true") for n in ("m1", "m2", "kb")]))
            s.lid("open")
            r, _ = poll_both(s, "off", prev="1, 1")
            self.assertEqual(r.evals, [device_eval(n, "true") for n in ("m1", "m2", "kb")])

    def test_blanked_but_monitors_awake_wakes(self):
        with Sandbox() as s:
            s.hypr["cursorpos"] = "5, 5\n"
            s.monitors(EDP, {"name": "DP-1", "disabled": True, "dpmsStatus": False})
            self.assertEqual(poll_both(s, "blanked", prev="5, 5")[0].stdout.strip(), "wake")
            self.assertEqual(poll_both(s, prev="5, 5")[0].stdout.strip(), "hold")
            s.monitors(EDP, {"name": "DP-2", "dpmsStatus": False})
            self.assertEqual(poll_both(s, "blanked", prev="5, 5")[0].stdout.strip(), "blank")
            s.monitors()
            self.assertEqual(poll_both(s, "blanked", prev="5, 5")[0].stdout.strip(), "hold")

    def test_socket_gone_still_answers_from_lid(self):
        with Sandbox(hypr_up=False) as s:
            s.lid("closed")
            r = s.rust("idle-wake-poll", "blanked")
            self.assertEqual((r.code, r.stdout.strip()), (0, "closed"))

    def test_unsafe_device_name_never_reaches_eval(self):
        with Sandbox() as s:
            s.devices(mice=['ok-mouse', 'bad"name'], keyboards=['ok-kb'])
            assert_same(s.python("idle-wake-poll"), s.rust("idle-wake-poll"))
            self.assertFalse(any("bad" in e for e in s.rust("idle-wake-poll").evals))
            self.assertEqual(s.rust("idle-wake-poll").evals, [device_eval("ok-mouse", "true"), device_eval("ok-kb", "true")])

    def test_power_button_keyboard_skipped(self):
        with Sandbox() as s:
            s.devices(mice=["power-button", "video-bus"],
                      keyboards=["power-button", "video-bus", "x-gpio-keys", "my-power-button", "AT:kb.0_1-x"])
            r, _ = poll_both(s, "blanked")
            self.assertEqual(r.evals, [device_eval(n, "true") for n in ("power-button", "video-bus", "my-power-button", "AT:kb.0_1-x")])

    def test_non_string_device_names_follow_python_str(self):
        with Sandbox() as s:
            s.devices(mice=[5, True, False, None, "", 1.5, ["a"], {"name": "x"}, "nl\n", "sp ace"], keyboards=[0, 7])
            r, _ = poll_both(s)
            self.assertEqual(r.evals, [device_eval(n, "true") for n in ("5", "True", "1.5", "x", "nl\n", "7")])

    def test_no_forks(self):
        with Sandbox() as s:
            self.assertEqual(s.rust("idle-wake-poll", "blanked").calls, [])


def park(s, *specs):
    s.write(STATE_FILE, json.dumps({"disabled": list(specs)}) + "\n")


def sync_both(s, *specs):
    out = []
    for run in (s.python, s.rust):
        park(s, *specs)
        r = run("displays", "sync")
        out.append((r, s._target(STATE_FILE).read_text()))
    (py, py_file), (rs, rs_file) = out
    assert_same(py, rs)
    unittest.TestCase().assertEqual(json.loads(py_file), json.loads(rs_file))
    return rs, rs_file


class SyncParity(unittest.TestCase):
    def test_idle_when_nothing_parked(self):
        with Sandbox() as s:
            s.monitors(EDP)
            r, _ = sync_both(s)
            self.assertEqual((r.stdout.strip(), r.evals), ("idle", []))
            s._target(STATE_FILE).unlink()
            assert_same(s.python("displays", "sync"), s.rust("displays", "sync"))
            self.assertEqual(s.rust("displays", "sync").stdout.strip(), "idle")

    def test_waiting_while_a_monitor_sleeps(self):
        with Sandbox() as s:
            spec = {"name": "DP-1", "width": 1920, "height": 1080}
            for mons in ([EDP, {"name": "DP-2", "dpmsStatus": False}], []):
                s.monitors(*mons)
                r, f = sync_both(s, spec)
                self.assertEqual((r.stdout.strip(), r.evals, f), ("waiting", [], json.dumps({"disabled": [spec]}) + "\n"))

    def test_restored_sends_same_lua_and_clears_state(self):
        with Sandbox() as s:
            s.monitors(EDP, {"name": "DP-9", "disabled": True})
            specs = [
                {"name": "DP-1", "output": "desc:BOE NE160 (1)+x", "width": 2560, "height": 1600,
                 "refreshRate": 59.951, "x": 0, "y": 0, "scale": 1.25},
                {"name": "DP-2", "width": 1920, "height": 1080, "refreshRate": 60.0, "x": -1920, "y": 0, "scale": 1},
                {"name": "DP-3", "width": 3840, "height": 2160, "refreshRate": 144, "x": 2560, "y": 100, "scale": 2.0},
                {"name": "DP-4", "width": 800, "height": 600},
                {"name": "DP-5", "width": 0, "height": 600, "refreshRate": 75},
                {"name": "DP-6", "width": 100, "height": 100, "refreshRate": 1e16, "scale": 0.00001},
                {"name": "DP-7", "width": 100, "height": 100, "refreshRate": 0.1, "scale": 1.5e300},
                {"name": "bad name"}, {"name": "DP-8", "output": "desc:a;b"}, {"name": ""}, {"name": 0}, {"output": "x"},
                "HDMI-A-1", 12, None,
            ]
            r, f = sync_both(s, *specs)
            self.assertEqual((r.stdout.strip(), json.loads(f)), ("restored", {"disabled": []}))
            self.assertIn('hl.monitor({ output = "desc:BOE NE160 (1)+x", mode = "2560x1600@59.951", '
                          'position = "0x0", scale = 1.25, disabled = false })', r.evals)
            self.assertIn('hl.monitor({ output = "DP-2", mode = "1920x1080@60.0", position = "-1920x0", scale = 1, '
                          'disabled = false })', r.evals)
            self.assertIn('hl.monitor({ output = "DP-3", mode = "3840x2160@144", position = "2560x100", scale = 2.0, '
                          'disabled = false })', r.evals)
            self.assertIn('hl.monitor({ output = "DP-4", mode = "800x600@60", position = "0x0", scale = 1, '
                          'disabled = false })', r.evals)
            self.assertIn('hl.monitor({ output = "DP-5", disabled = false })', r.evals)
            joined = "".join(r.evals)
            self.assertIn('@1e+16"', joined)
            self.assertIn("scale = 1e-05", joined)
            self.assertIn("scale = 1.5e+300", joined)

    def test_unopenable_lock_fails_closed(self):
        with Sandbox() as s:
            s.monitors(EDP)
            for run in (s.python, s.rust):
                park(s, {"name": "DP-1"})
                lock = s._target("run/omarchy-power-hub/display-off.lock")
                lock.unlink(missing_ok=True)
                lock.mkdir()
                r = run("displays", "sync")
                lock.rmdir()
                self.assertEqual((r.code, r.stdout, r.evals, s.read_state()), (1, "", [], {"disabled": [{"name": "DP-1"}]}))

    def test_waits_for_python_holding_the_lock(self):
        with Sandbox() as s:
            s.monitors(EDP)
            park(s, {"name": "DP-1"})
            with s.hold_flock("run/omarchy-power-hub/display-off.lock"):
                p = s.rust_start("displays", "sync")
                time.sleep(0.2)
                self.assertIsNone(p.poll())
                self.assertEqual(s.read_state(), {"disabled": [{"name": "DP-1"}]})
            self.assertEqual(p.wait(timeout=2), 0)
            self.assertEqual(p.stdout.read().strip(), "restored")
            self.assertEqual(s.read_state(), {"disabled": []})
            self.assertEqual(s._evals, ['hl.monitor({ output = "DP-1", disabled = false })'])



SHELL_FILE = "home/.config/omarchy/shell.json"
BINDINGS_FILE = "home/.config/hypr/bindings.lua"
HIBERNATE_DROPIN = "/etc/systemd/sleep.conf.d/90-omarchy-power-hub.conf"
SHELL_IDLE = '{"idle": {"screensaver": 150, "lock": 300}, "other": 1}'
PROFILES = ["power-saver", "balanced", "performance"]


def settings_setup(shell=SHELL_IDLE, bindings=None, battery=False, **keys):
    def setup(s):
        s.config(**keys)
        s.profiles(PROFILES)
        s.write("home/.config/hypr/.keep", "")
        if shell is not None:
            s.write(SHELL_FILE, shell)
        if bindings is not None:
            s.write(BINDINGS_FILE, bindings)
        if battery:
            s.stub("busctl", "OnBattery", 0, "b true\n")
    return setup


@contextlib.contextmanager
def checked(setup, *steps):
    """Python and Rust must agree on every step; yields a fresh Rust sandbox after the steps and the last result."""
    both(setup, *steps)
    with Sandbox() as s:
        setup(s)
        last = [s.rust(*step) for step in steps][-1]
        yield s, last


def shell_of(s):
    return json.loads(s._target(SHELL_FILE).read_text())


class SettingsParity(unittest.TestCase):
    def test_active_side_rewrites_only_idle(self):
        with checked(settings_setup(battery=True), ["set", "battery", "lock", "600"]) as (s, _):
            self.assertEqual(shell_of(s), {"idle": {"screensaver": 150, "lock": 600}, "other": 1})

    def test_inactive_side_changes_the_config_only(self):
        with checked(settings_setup(), ["set", "battery", "lock", "600"]) as (s, _):
            self.assertEqual(s._target(SHELL_FILE).read_text(), SHELL_IDLE)
            self.assertEqual(s.read_config()["battery"]["lock"], 600)

    def test_matching_shell_json_is_not_written(self):
        text = '{"idle":{"screensaver":150.0,"lock":300}}'
        with checked(settings_setup(shell=text), ["set", "ac", "lock", "300"]) as (s, _):
            self.assertEqual(s._target(SHELL_FILE).read_text(), text)

    def test_saver_hidden_by_displays_off_maps_to_a_week(self):
        with checked(settings_setup(ac={"profile": "balanced", "displaysOff": 200}), ["set", "ac", "screensaver", "300"]) as (s, _):
            self.assertEqual(shell_of(s)["idle"]["screensaver"], 7 * 24 * 3600)

    def test_negative_values_clamp_to_zero(self):
        with checked(settings_setup(), ["set", "ac", "sleep", "-5"]) as (s, _):
            self.assertEqual(s.read_config()["ac"]["sleep"], 0)

    def test_lid_writes_one_block_and_reloads_once(self):
        step = ["set", "ac", "lid", "display-off"]
        with checked(settings_setup(bindings="bind = x"), step, step) as (s, last):
            text = s._target(BINDINGS_FILE).read_text()
            self.assertEqual((text.count("-- BEGIN"), text.count("-- END")), (1, 1))
            self.assertIn(f"python3 {s.bin}/ops.py lid-closed", text)
            self.assertEqual(last.requests, [])
        with Sandbox() as s:
            settings_setup(bindings="bind = x")(s)
            self.assertEqual(s.rust(*step).requests, ["reload"])

    def test_lid_block_cases(self):
        stale = "a = 1\n-- BEGIN Omarchy Power Hub lid action override\nold\n-- END Omarchy Power Hub lid action override\nb = 2\n"
        cases = {"missing": None, "empty": "", "no final newline": "a = 1", "stale block": stale, "two blocks": stale + stale,
                 "begin without end": "a = 1\n-- BEGIN Omarchy Power Hub lid action override\nold\n"}
        for name, bindings in cases.items():
            for action in ("display-off", "nothing"):
                with self.subTest(name, action=action):
                    both(settings_setup(bindings=bindings), ["set", "ac", "lid", action])

    def test_matching_hibernate_delay_needs_no_sudo(self):
        def setup(s):
            settings_setup()(s)
            s.write(HIBERNATE_DROPIN, "[Sleep]\nHibernateDelaySec=1800\n")
        with checked(setup, ["set", "ac", "hibernateAfter", "1800"]) as (_, last):
            self.assertFalse([c for c in last.calls if c.startswith("sudo")])

    def test_hibernate_delay_is_pushed_through_the_helper(self):
        def setup(s):
            settings_setup()(s)
            s.install_helper()
            s.stub("busctl", "CanHibernate", 0, 's "yes"\n')
        with checked(setup, ["set", "ac", "hibernateAfter", "900"]) as (_, last):
            self.assertTrue([c for c in last.calls if "hibernate-delay 900" in c])

    def test_unavailable_hibernation_gives_the_reason(self):
        def setup(s):
            settings_setup()(s)
            s.write("/sys/power/disk", "[disabled]\n")
        with checked(setup, ["set", "ac", "hibernateAfter", "1800"]) as (_, last):
            self.assertEqual((last.code, "secret memory" in last.stdout), (1, True))

    def test_bad_arguments_give_the_error_strings(self):
        steps = [["set", "ac", "nope", "1"], ["set", "x", "lock", "1"], ["set", "ac", "lid", "bad"], ["set", "ac", "profile", "bad"]]
        with checked(settings_setup(), *steps) as (s, _):
            self.assertEqual([s.rust(*step).stdout.strip() for step in steps], ["bad key", "bad source", "bad lid action", "bad profile"])

    def test_non_numbers_are_a_bad_value(self):
        with Sandbox() as s:
            settings_setup()(s)
            r = s.rust("set", "ac", "lock", "abc")
            self.assertEqual((r.code, r.stdout.strip()), (1, "bad value"))

    def test_travel_holding_the_cpu_only_saves_the_profile(self):
        def setup(s):
            settings_setup()(s)
            s.write(TRAVEL_FILE, json.dumps(travel_snapshot()))
        with checked(setup, ["set", "ac", "profile", "saver"]) as (s, last):
            self.assertEqual(s.read_config()["ac"]["profile"], "power-saver")
            self.assertFalse([c for c in last.calls if "powerprofiles-set" in c])

    def test_profile_on_the_active_side_is_applied(self):
        with checked(settings_setup(), ["profile", "saver"]) as (_, last):
            self.assertIn("omarchy-powerprofiles-set ac power-saver", last.calls)

    def test_a_named_profile_never_asks_the_daemon(self):
        for step in (["profile", "saver"], ["set", "ac", "profile", "saver"]):
            with checked(settings_setup(), step) as (_, last):
                self.assertFalse([c for c in last.calls if "net.hadess" in c])

    def test_profile_on_the_battery_side_is_applied(self):
        with checked(settings_setup(battery=True), ["profile", "full"]) as (_, last):
            self.assertIn("omarchy-powerprofiles-set battery performance", last.calls)

    def test_profile_for_the_inactive_side_is_only_remembered(self):
        with checked(settings_setup(), ["set", "battery", "profile", "saver"]) as (s, last):
            self.assertEqual(s._target("home/.local/state/omarchy/powerprofiles/battery").read_text(), "power-saver\n")
            self.assertFalse([c for c in last.calls if "powerprofiles-set" in c])

    def test_failed_profile_change_reports_the_tool_output(self):
        def setup(s):
            settings_setup()(s)
            s.stub("omarchy-powerprofiles-set", "", 1, "no daemon\n")
        with checked(setup, ["profile", "saver"]) as (_, last):
            self.assertEqual((last.code, last.stdout.strip()), (1, "no daemon"))

    def test_missing_arguments_are_usage(self):
        with Sandbox() as s:
            self.assertEqual([s.rust(*a).code for a in (["set", "ac", "lock"], ["profile"])], [2, 2])

    def test_shell_json_keeps_everything_but_idle(self):
        raw = '{"zeta": "café", "idle": {"screensaver": 1, "other": 0.30000000000000004}, "alpha": [1]}'
        with Sandbox() as s:
            settings_setup(shell=raw)(s)
            self.assertEqual(s.rust("set", "ac", "lock", "600").code, 0)
            text = s._target(SHELL_FILE).read_text()
            self.assertEqual(list(json.loads(text)), ["zeta", "idle", "alpha"])
            self.assertEqual(json.loads(text)["idle"], {"screensaver": 150, "other": 0.30000000000000004, "lock": 600})
            self.assertIn("café", text)
            self.assertIn("0.30000000000000004", text)

    def test_malformed_shell_json_is_left_alone(self):
        with checked(settings_setup(shell="{broken"), ["set", "ac", "lock", "600"]) as (s, last):
            self.assertEqual((last.code, last.stdout.strip(), s._target(SHELL_FILE).read_text()), (1, "shell.json unreadable", "{broken"))

    def test_load_asks_for_profiles_only_when_ac_lacks_one(self):
        def setup(s):
            s.write(Sandbox.CONFIG_PATH, '{"ac": {"profile": "saver"}, "battery": {}}')
            s.profiles(PROFILES)
        with checked(setup, ["config"]) as (_, last):
            self.assertFalse([c for c in last.calls if "net.hadess" in c])

    def test_status_asks_for_profiles_once(self):
        with Sandbox() as s:
            s.profiles(PROFILES)
            self.assertEqual(len([c for c in s.rust("status").calls if "net.hadess" in c]), 1)


BATTERY = "/org/freedesktop/UPower/devices/battery_BAT0"


def framework_setup(limit=90, **keys):
    def setup(s):
        s.framework(); s.install_helper(); s.config(**keys); s.charge_limit(limit)
    return setup


def battery_setup(*devices, fail=False):
    def setup(s):
        s.config()
        s.stub("upower", "-e", 0, "".join(f"{d}\n" for d in devices))
        s.stub("busctl", "ChargeThresholdSupported", 0, "b true\n")
        s.stub("busctl", "EnableChargeThreshold", int(fail))
    return setup


def no_helper_setup(**keys):
    def setup(s):
        s.framework(); s.config(**keys)
    return setup


def no_tool_setup(**keys):
    def setup(s):
        s.framework(tool=False); s.install_helper(); s.config(**keys)
    return setup


class ChargeVerbParity(unittest.TestCase):
    def test_missing_framework_tool(self):
        message = "framework_tool is not installed"
        for step in (["charge", "70"], ["boost", "on"], ["boost", "off"]):
            with self.subTest(step=step), checked(no_tool_setup(chargeLimit=80), step) as (_, r):
                self.assertEqual((r.code, r.stdout.strip()), (1, message))
        with Sandbox() as s:
            no_tool_setup()(s)
            py, rs = s.python("status"), s.rust("status")
            assert_same(py, rs)
            self.assertEqual(json.loads(rs.stdout)["charge"]["reason"], message)

    def test_charge_without_the_helper_goes_through_pkexec(self):
        for code, out in ((0, "ok"), (1, "privilege required")):
            with self.subTest(code=code):
                def setup(s):
                    no_helper_setup(chargeLimit=80)(s)
                    s.stub("pkexec", "charge-limit", code)
                with checked(setup, ["charge", "70"], ["boost", "on"]) as (s, r):
                    cfg = s.read_config()
                    self.assertEqual((r.code, r.stdout.strip(), cfg["boostSavedLimit"], cfg["boost"]), (code, out, 70 if code == 0 else 80, True))
                    self.assertIn("ops-priv charge-limit 100", r.calls_text)
                    self.assertNotIn("sudo", r.calls_text)

    def test_boost_off_without_the_helper(self):
        for code in (0, 1):
            def setup(s):
                no_helper_setup(boost=True, boostSavedLimit=75)(s)
                s.stub("pkexec", "charge-limit", code)
            with self.subTest(code=code), checked(setup, ["boost", "off"]) as (s, r):
                self.assertEqual((r.code, s.read_config()["boost"]), (code, False))
                self.assertIn("ops-priv charge-limit 75", r.calls_text)

    def test_charge_saves_the_limit_and_the_boost_restore_point(self):
        for keys, saved in (({}, 70), ({"boost": True, "boostSavedLimit": 60}, 60)):
            with self.subTest(keys=keys), checked(framework_setup(**keys), ["charge", "70"]) as (s, r):
                cfg = s.read_config()
                self.assertEqual((r.stdout.strip(), cfg["chargeLimit"], cfg["boostSavedLimit"]), ("ok", 70, saved))

    def test_charge_clamps_the_tool_argument_and_saves_the_raw_value(self):
        for raw, sent in ((30, 40), (110, 100)):
            with self.subTest(raw=raw), checked(framework_setup(), ["charge", str(raw)]) as (s, r):
                self.assertEqual(s.read_config()["chargeLimit"], raw)
                self.assertIn(f"ops-priv charge-limit {sent}", r.calls_text)

    def test_charge_failure_leaves_the_config_alone(self):
        def setup(s):
            s.stub("sudo", "charge-limit 70", 1)
            framework_setup(chargeLimit=80)(s)
        with checked(setup, ["charge", "70"]) as (s, r):
            self.assertEqual((r.code, r.stdout.strip(), s.read_config()["chargeLimit"]), (1, "privilege required", 80))

    def test_charge_without_a_framework(self):
        with checked(lambda s: s.config(), ["charge", "70"]) as (s, r):
            self.assertEqual((r.code, r.stdout.strip(), "chargeLimit" in s.read_config()), (1, "not a Framework machine", False))

    def test_charge_with_a_bad_value_changes_nothing(self):
        for bad in ("abc", "", "7.5"):
            with self.subTest(bad=bad), Sandbox() as s:
                framework_setup(chargeLimit=80)(s)
                before = s.snapshot()
                r = s.rust("charge", bad)
                self.assertEqual((r.code, r.stdout.strip(), r.calls), (1, "bad value", []))
                self.assertEqual(s.snapshot(), before)

    def test_boost_on_stores_the_live_limit_then_goes_to_100(self):
        cases = [(framework_setup(90, chargeLimit=80), 90), (framework_setup(0, chargeLimit=65), 65),
                 (framework_setup(0), 80), (lambda s: s.config(chargeLimit=65), 65), (lambda s: s.config(), 80)]
        for setup, stored in cases:
            with self.subTest(stored=stored), checked(setup, ["boost", "on"]) as (s, _):
                cfg = s.read_config()
                self.assertEqual((cfg["boost"], cfg["boostSavedLimit"]), (True, stored))

    def test_boost_off_restores_the_saved_limit(self):
        for keys, sent in (({"boostSavedLimit": 75}, 75), ({"boostSavedLimit": 0, "chargeLimit": 65}, 65), ({}, 80)):
            with self.subTest(keys=keys), checked(framework_setup(100, boost=True, **keys), ["boost", "off"]) as (s, r):
                self.assertFalse(s.read_config()["boost"])
                self.assertIn(f"ops-priv charge-limit {sent}", r.calls_text)

    def test_boost_spellings_round_trip(self):
        for on, off in (("1", "0"), ("true", "false"), ("on", "off"), ("on", "x")):
            with self.subTest(on=on, off=off), checked(framework_setup(90, chargeLimit=80), ["boost", on], ["boost", off]) as (s, _):
                self.assertFalse(s.read_config()["boost"])

    def test_failing_privilege_keeps_boost_saved_and_exits_1(self):
        def setup(s):
            s.stub("sudo", "charge-limit 100", 1)
            framework_setup(90, chargeLimit=80)(s)
        with checked(setup, ["boost", "on"]) as (s, r):
            cfg = s.read_config()
            self.assertEqual((r.code, r.stdout.strip(), cfg["boost"], cfg["boostSavedLimit"]), (1, "privilege required", True, 90))

    def test_health_follows_its_spellings(self):
        for arg in ("on", "off", "1", "true", "0", "x"):
            with self.subTest(arg=arg), checked(battery_setup(BATTERY), ["health", arg]) as (_, r):
                desired = "true" if arg in ("on", "1", "true") else "false"
                self.assertIn(f"EnableChargeThreshold b {desired}", r.calls_text)

    def test_health_updates_every_battery_and_reports_one_failure(self):
        second = BATTERY.replace("BAT0", "BAT1")
        for fail in (False, True):
            with self.subTest(fail=fail), checked(battery_setup(BATTERY, second, fail=fail), ["health", "on"]) as (_, r):
                self.assertEqual((r.calls_text.count("EnableChargeThreshold"), r.stdout.strip(), r.code),
                                 (2, "Failed to update charge limits", 1) if fail else (2, "ok", 0))

    def test_health_without_batteries(self):
        with checked(lambda s: s.config(), ["health", "on"]) as (_, r):
            self.assertEqual((r.code, r.stdout.strip()), (1, "Battery charge limits are not supported"))

    def test_helper_install(self):
        cases = [(lambda s: s.stub("pkexec", "helper-install", 0), 0, "ok"),
                 (lambda s: s.stub("pkexec", "helper-install", 1, "denied\n"), 1, "denied"),
                 (lambda s: None, 1, "helper install failed")]
        for setup, code, out in cases:
            with self.subTest(out=out), checked(setup, ["helper-install"]) as (s, r):
                self.assertEqual((r.code, r.stdout.strip()), (code, out))
                self.assertIn(f"pkexec {s.bin}/ops-priv helper-install", r.calls_text)


TWEAKS_FILE = "home/.config/omarchy/power-hub-tweaks.json"
UNIT_FILE = "home/.config/systemd/user/omarchy-power-hub-tweaks.service"


def mask_exec(path, text):
    return re.sub(r"ExecStart=.*", "ExecStart=<masked>", text)


def tweak_setup(prefs=None, travel=None, effects=(True, True), **rules):
    def setup(s):
        s.config()
        s.stub("pkexec", "", 0)
        s.hypr["getoption animations:enabled"] = f"bool: {str(effects[0]).lower()}\nset: true\n"
        s.hypr["getoption decoration:shadow:enabled"] = f"int: {int(effects[1])}\nset: true\n"
        if prefs is not None:
            s.write(TWEAKS_FILE, prefs if isinstance(prefs, str) else json.dumps(prefs))
        if travel is not None:
            s.write(TRAVEL_FILE, json.dumps(travel))
        for tool, (match, code, out) in rules.items():
            s.stub(tool, match, code, out)
    return setup


def tweak_both(setup, *steps):
    both(setup, *steps, normalise=mask_exec)


@contextlib.contextmanager
def tweaked(setup, *steps):
    """Python and Rust agree on every step; yields a fresh Rust sandbox after the steps and the last result."""
    tweak_both(setup, *steps)
    with Sandbox() as s:
        setup(s)
        yield s, [s.rust(*step) for step in steps][-1]


class ActionParity(unittest.TestCase):
    READS = ("busctl get-property org.freedesktop.UPower", "systemctl is-active")  # what the trailing data adds
    ROWS = (
        (["profile", "saver"], ["action", "mode", "saver"]),
        (["cpu", "turbo", "off"], ["action", "turbo", "off"]),
        (["cpu", "epp", "power"], ["action", "epp", "power"]),
        (["tweak", "wifi", "on"], ["action", "wifi", "on"]),
        (["tweak", "wifi", "on", "persist"], ["action", "persist", "wifi", "on"]),
        (["unpin", "wifi"], ["action", "unpersist", "wifi"]),
        (["daylight", "on"], ["action", "daylight", "on"]),
    )

    def test_action_changes_then_prints_data(self):
        for py, rs in self.ROWS:
            with self.subTest(verb=rs[1]):
                both(tweak_setup(), (py, rs), normalise=mask_exec, stdout=False, drop=(*DROPPED, *self.READS))
                with Sandbox() as s:
                    tweak_setup()(s)
                    s.stub("omarchy-powerprofiles-list", "", 0, "")
                    out = s.rust(*rs)
                    self.assertEqual((out.code, "battery" in json.loads(out.stdout)), (0, True))

    def test_action_with_a_failing_write_still_prints_data(self):
        with Sandbox() as s:
            s.config()
            for argv in (["action", "wifi", "on"], ["action", "nonsense"], ["action", "epp", ""]):
                out = s.rust(*argv)
                self.assertEqual((out.code, "battery" in json.loads(out.stdout)), (0, True), argv)

    def test_install_buttons_print_nothing(self):
        with Sandbox() as s:
            s.config()
            for verb in ("install", "install-wifi-fix"):
                out = s.rust("action", verb)
                self.assertEqual((out.code, out.stdout), (0, ""), verb)

    def test_terminal_action_quotes_the_path(self):
        with Sandbox() as s:
            s.config()
            bin_dir = s.base / "a b" / "bin"
            script = s.base / "a b" / "extras" / "sleep-tracking" / "apply.sh"
            script.parent.mkdir(parents=True)
            script.write_text("")
            bin_dir.mkdir()
            s.calls_log.write_text("")
            p = subprocess.run([str(RUST_BIN), "action", "install-sleep-tracking"], env={**s._env(), "POWER_HUB_BIN": str(bin_dir)},
                               cwd=s.base, text=True, capture_output=True, timeout=30)
            for _ in range(100):
                calls = s.calls_log.read_text().strip()
                if calls:
                    break
                time.sleep(0.05)
            self.assertEqual((p.returncode, p.stdout), (0, ""))
            self.assertEqual(calls, f"omarchy-launch-floating-terminal-with-presentation bash '{script}'")

    def test_python_free_panel_actions(self):
        self.assertNotIn("battery-plus-action", (REPO / "PowerService.qml").read_text())
        for gone in ("battery-plus-action", "battery-plus-priv"):
            self.assertFalse((REPO / "bin" / gone).exists())


class TweakParity(unittest.TestCase):
    def test_privileged_tweaks_follow_their_spellings(self):
        tweak_both(tweak_setup(), ["tweak", "wifi", "on"], ["tweak", "wifi", "off"], ["tweak", "usb", "true"], ["tweak", "pcie", "x"])

    def test_failing_privilege_reports_and_still_saves(self):
        tweak_both(lambda s: s.config(), ["tweak", "wifi", "on"])

    def test_persist_writes_the_unit_and_enables_it(self):
        for spelling in ("persist", "1", "pin", "no"):
            with self.subTest(spelling=spelling), tweaked(tweak_setup(), ["tweak", "wifi", "on", spelling]) as (s, r):
                self.assertEqual((s._target(UNIT_FILE).exists(), "systemctl --user enable --now omarchy-power-hub-tweaks.service" in r.calls_text),
                                 (spelling != "no",) * 2)

    def test_eval_error_reply_is_reported_by_both(self):
        def setup(s):
            tweak_setup()(s)
            s.eval_reply = "error: no such monitor"
        tweak_both(setup, ["tweak", "effects", "on"])
        with Sandbox() as s:
            setup(s)
            self.assertIn("error: no such monitor", s.rust("tweak", "effects", "on").stdout)

    def test_effects_on_saves_the_previous_values_and_sends_both_evals(self):
        for effects in ((True, True), (False, True)):
            with self.subTest(effects=effects), tweaked(tweak_setup(effects=effects), ["tweak", "effects", "on"]) as (s, r):
                self.assertEqual(json.loads(s._target(TWEAKS_FILE).read_text())["effectsBefore"],
                                 {"animations:enabled": effects[0], "decoration:shadow:enabled": effects[1]})
                self.assertEqual(r.evals, ["hl.config({ animations = { enabled = false } })", "hl.config({ decoration = { shadow = { enabled = false } } })"])

    def test_effects_off_restores_from_the_saved_values(self):
        before = {"effectsBefore": {"animations:enabled": False, "decoration:shadow:enabled": True}}
        tweak_both(tweak_setup(before), ["tweak", "effects", "off"])
        tweak_both(tweak_setup({"effectsBefore": {"animations:enabled": None}}), ["tweak", "effects", "off"])

    def test_effects_off_without_a_record_defaults_to_enabled(self):
        with tweaked(tweak_setup(), ["tweak", "effects", "off"]) as (_, r):
            self.assertEqual(r.evals, ["hl.config({ animations = { enabled = true } })", "hl.config({ decoration = { shadow = { enabled = true } } })"])

    def test_second_effects_on_keeps_the_first_record(self):
        tweak_both(tweak_setup(), ["tweak", "effects", "on"], ["tweak", "effects", "on"])
        with Sandbox() as s:
            tweak_setup()(s)
            s.rust("tweak", "effects", "on")
            s.hypr["getoption animations:enabled"] = "bool: false\n"
            s.rust("tweak", "effects", "on")
            self.assertTrue(json.loads(s._target(TWEAKS_FILE).read_text())["effectsBefore"]["animations:enabled"])

    def test_effects_unreachable_hyprland_is_an_error(self):
        with Sandbox(hypr_up=False) as s:
            s.config()
            r = s.rust("tweak", "effects", "on")
            self.assertEqual(r.code, 1)
            self.assertIn("Hyprland is not reachable", r.stdout)

    def test_turbo_is_refused_while_travel_owns_the_cpu(self):
        travel = travel_snapshot()
        tweak_both(tweak_setup(travel=travel), ["tweak", "turbo", "on"])
        tweak_both(tweak_setup(travel=travel_snapshot(reduceCpu=False)), ["tweak", "turbo", "on"])

    def test_unpin_daylight_and_cpu_keep_every_other_key(self):
        prefs = {"z": 1, "wifi": True, "wifiPinned": True, "usbPinned": True, "daylight": False, "a": [1]}
        tweak_both(tweak_setup(prefs), ["unpin", "wifi"], ["unpin", "missing"], ["daylight", "on"], ["daylight", "off"], ["daylight", "x"])

    def test_cpu_pins_epp_and_leaves_turbo_alone(self):
        tweak_both(tweak_setup({"z": 1}), ["cpu", "epp", "power"], ["cpu", "turbo", "off"])
        tweak_both(tweak_setup(), ["cpu", "epp", "power"])

    def test_failed_cpu_change_is_not_saved(self):
        tweak_both(lambda s: s.config(), ["cpu", "epp", "power"])

    def test_cpu_is_refused_while_travel_owns_it(self):
        tweak_both(tweak_setup({"z": 1}, travel=travel_snapshot()), ["cpu", "epp", "power"], ["cpu", "turbo", "off"])

    def test_apply_tweaks_replays_the_pinned_keys(self):
        prefs = {k + "Pinned": True for k in ("wifi", "pcie", "usb", "audio", "abm", "effects", "turbo")}
        prefs.update(wifi=True, usb=False, effects=True, turbo=True, pcie=0, epp="power", eppPinned=True)
        tweak_both(tweak_setup(prefs), ["apply-tweaks"])
        tweak_both(tweak_setup({"wifi": True, "usbPinned": True}), ["apply-tweaks"])
        tweak_both(tweak_setup({}), ["apply-tweaks"])
        tweak_both(lambda s: s.config(), ["apply-tweaks"])

    def test_apply_tweaks_skips_the_cpu_while_travel_owns_it(self):
        prefs = {"turboPinned": True, "turbo": True, "wifiPinned": True, "wifi": True, "eppPinned": True, "epp": "power"}
        tweak_both(tweak_setup(prefs, travel=travel_snapshot()), ["apply-tweaks"])

    def test_apply_tweaks_reports_the_last_failure(self):
        def setup(pins):
            def go(s):
                s.config()
                s.write(TWEAKS_FILE, json.dumps({k: True for k in pins} | {"epp": "power"}))
                for key in ("wifi", "usb", "epp"):
                    s.stub("pkexec", key, 1, f"{key} failed\n")
            return go
        for pins, last in ((["wifiPinned", "usbPinned", "eppPinned"], "epp failed"), (["wifiPinned", "usbPinned"], "usb failed")):
            with self.subTest(last=last), tweaked(setup(pins), ["apply-tweaks"]) as (_, r):
                self.assertEqual(r.stdout.strip(), last)

    def test_tweaks_unit_runs_the_binary(self):
        with Sandbox() as s:
            tweak_setup()(s)
            s.rust("tweak", "wifi", "on", "persist")
            unit = s._target(UNIT_FILE).read_text()
            self.assertIn(f"ExecStart={s.bin}/power-hub apply-tweaks\n", unit)
            self.assertEqual(unit.replace(f"{s.bin}/power-hub", "X"), "[Unit]\nDescription=Omarchy Power Hub device tweaks\n\n"
                             "[Service]\nType=oneshot\nExecStart=X apply-tweaks\n\n[Install]\nWantedBy=default.target\n")

    def test_list_in_tweaks_file_is_replaced_not_a_crash(self):
        with Sandbox() as s:
            tweak_setup("[1]")(s)
            r = s.rust("tweak", "wifi", "on")
            self.assertEqual((r.code, json.loads(s._target(TWEAKS_FILE).read_text())), (0, {"wifi": True}))


def dim_setup(start=80, state=None, travel=None, monitors=(LAPTOP,), set_body="echo ${v%\\%}"):
    """Brightness `start`; a set runs `set_body` with `v` the requested value and stores what it prints."""
    def setup(s):
        s.monitors(*monitors)
        state_file = s.base / "brightness.state"
        state_file.write_text(f"{start}\n")
        s._stateful("omarchy", f'''case "$*" in
  *--no-osd*) v="${{@: -1}}"; out=$({set_body}) || {{ echo "$out"; exit 1; }}; echo "$out" > {state_file};;
  *"brightness display --monitor"*) cat {state_file};;
  *) exit 1;;
esac''')
        for path, value in ((DIM_FILE, state), (TRAVEL_FILE, travel)):
            if value is not None:
                s.write(path, value if isinstance(value, str) else json.dumps(value))
    return setup


DIM_ON = {"on": True, "monitor": "eDP-1", "brightness": 70, "error": ""}
FAIL = "echo boom; false"


def read_stub(code, out):
    return lambda s: s.stub("omarchy", "brightness display --monitor", code, out)


def after(*setups):
    return lambda s: [f(s) for f in setups]


class QuickDimParity(unittest.TestCase):
    def test_on_caps_at_40_and_off_restores(self):
        for start in (80, 30, 40, 0):
            with self.subTest(start=start):
                both(dim_setup(start), ["quick-dim", "on"], ["quick-dim", "off"])

    def test_already_on_or_off_changes_nothing(self):
        both(dim_setup(40, state=DIM_ON), ["quick-dim", "on"])
        both(dim_setup(80), ["quick-dim", "off"])

    def test_refused_while_travel_is_on(self):
        both(dim_setup(80, travel=travel_snapshot()), ["quick-dim", "on"], ["quick-dim", "off"])

    def test_no_laptop_display(self):
        for monitors in (({"name": "DP-1"},), ()):
            both(dim_setup(80, monitors=monitors), ["quick-dim", "on"])

    def test_unreadable_brightness(self):
        for code, out in ((1, ""), (0, "abc\n"), (0, "101\n"), (0, "")):
            with self.subTest(code=code, out=out):
                both(after(dim_setup(80), read_stub(code, out)), ["quick-dim", "on"])

    def test_failing_set_keeps_the_restore_point_with_the_error(self):
        both(dim_setup(80, set_body=FAIL), ["quick-dim", "on"], ["quick-dim", "off"])
        both(dim_setup(70, state=DIM_ON, set_body=FAIL), ["quick-dim", "off"])

    def test_set_that_misses_by_two_is_an_error_and_by_one_is_fine(self):
        both(dim_setup(80, set_body="echo $((${v%\\%}+2))"), ["quick-dim", "on"])
        both(dim_setup(80, set_body="echo $((${v%\\%}+1))"), ["quick-dim", "on"])
        both(dim_setup(40, state=DIM_ON, set_body="echo 10"), ["quick-dim", "off"])

    def test_invalid_saved_state_is_shown_and_kept(self):
        for state in ('{"on": "yes"}', '{"on": true}', '{"on": true, "monitor": "e DP", "brightness": 5}',
                      '{"on": true, "monitor": "eDP-1", "brightness": 101}', "[1]", "{"):
            with self.subTest(state=state):
                both(dim_setup(80, state=state), ["quick-dim", "on"], ["quick-dim", "off"])

    def test_usage(self):
        for argv in (["quick-dim"], ["quick-dim", "maybe"], ["quick-dim", "on", "x"]):
            with self.subTest(argv=argv):
                both(dim_setup(80), argv)
        with Sandbox() as s:
            r = s.rust("quick-dim", "maybe")
            self.assertEqual((r.stdout, r.code), ("usage: quick-dim on|off\n", 2))

    def test_unwritable_run_dir_never_dims(self):
        def attempt(prepare, prefix):
            with Sandbox() as s:
                dim_setup(80)(s)
                run = s._target("run/omarchy-power-hub")
                prepare(run)
                try:
                    r = s.rust("quick-dim", "on")
                finally:
                    if run.is_dir():
                        run.chmod(0o700)
                return r.stdout.startswith(prefix), r.code, [c for c in r.calls if "--no-osd" in c]

        def as_file(run):
            run.parent.mkdir(parents=True, exist_ok=True)
            run.write_text("")

        def read_only(run):
            run.mkdir(parents=True)
            (run / "energy-controls.lock").write_text("")
            run.chmod(0o500)

        self.assertEqual(attempt(as_file, "Quick dim could not save its recovery state: "), (True, 1, []))
        if os.geteuid():  # root ignores directory modes; Python words this case on its own
            self.assertEqual(attempt(read_only, "Could not save brightness restore point: "), (True, 1, []))


PANEL = {**LAPTOP, "x": 0, "y": 0, "scale": 1.25, "description": "BOE NE160"}
PROFILES = ("power-saver", "balanced", "performance")
OLDER = "Travel restore point is from an older version; reboot to clear it"
SAVER_DENIED = 'if [ "$v" = power-saver ]; then echo "authorization denied"; false; else echo $v; fi'
CAP_REFUSED = 'if [ "${v%\\%}" = 40 ]; then echo boom; false; else echo ${v%\\%}; fi'


def panel(**changes):
    return {**PANEL, **changes}


def follow_modes(s):
    """Hyprland applies a mode change: the laptop row reports the refresh the eval asked for."""
    def on_eval(lua):
        m = re.search(r'mode = "\d+x\d+@([\d.]+)"', lua)
        if m:
            rows = json.loads(s.hypr["j/monitors"])
            for row in rows:
                if row["name"].startswith("eDP"):
                    row["refreshRate"] = float(m.group(1))
            s.hypr["j/monitors"] = json.dumps(rows)
    return on_eval


def eval_replies(*replies):
    """The Hyprland reply to each eval in turn, the last one repeating."""
    def setup(s):
        pending = iter(replies)
        mode = follow_modes(s)

        def on_eval(lua):
            s.eval_reply = next(pending, replies[-1])
            mode(lua)
        s.hypr_on_eval = on_eval
    return setup


def travel_env(brightness=80, profile="performance", names=PROFILES, rows=(PANEL,), travel=None,
               set_body="echo ${v%\\%}", profile_set="echo $v", profile_get="cat", replies=("ok",), cfg=None):
    """A laptop on battery; `set_body` and `profile_set` are bash (or a function of the sandbox) run for each set."""
    def setup(s):
        s.profiles(list(names))
        s.config(**(cfg if cfg is not None else {"ac": {"profile": "performance"}}))
        s.discharging()
        dim_setup(brightness, travel=travel, monitors=tuple(dict(r) for r in rows),
                  set_body=set_body(s) if callable(set_body) else set_body)(s)
        state = s.base / "profile.state"
        state.write_text(f"{profile}\n")
        body = profile_set(s) if callable(profile_set) else profile_set
        s._stateful("powerprofilesctl", f'''case "$1" in
  get) {"cat " + str(state) if profile_get == "cat" else profile_get};;
  set) v=$2; out=$({body}) || {{ echo "$out"; exit 1; }}; echo "$out" > {state};;
  *) exit 1;;
esac''')
        eval_replies(*replies)(s)
    return setup


def tweaks_on(abm="0\n", effects=(True, True)):
    def setup(s):
        tweak_setup(effects=effects)(s)
        s.write("/sys/class/drm/card1-eDP-1/amdgpu/panel_power_savings", abm)
    return setup


def no_leftovers(py, rs):
    """Snapshots skip *.tmp, so a temp file left behind by a failed write is only visible here."""
    for s in (py, rs):
        left = [p.name for p in s.run.rglob("*.tmp")]
        assert not left, f"temp files left behind: {left}"


MALFORMED_PENDING_BASE = {
    "on": True, "complete": False, "reduceCpu": True, "source": "battery", "profile": None, "brightness": None,
    "monitor": None, "cpu": {"turbo": None, "epp": []}, "error": "",
    "controls": {key: {"available": False, "detail": "Unavailable"} for key in ("profile", "turbo", "epp", "brightness", "refresh")}}


class TravelParity(unittest.TestCase):
    def check(self, setup, *steps):
        both(setup, *steps, inspect=no_leftovers)

    def test_internal_output_only_is_changed_and_restored(self):
        self.check(travel_env(), ["travel", "on"], ["travel", "off"])

    def test_already_dim_display_stays(self):
        self.check(travel_env(brightness=30), ["travel", "on"], ["travel", "off"])

    def test_refresh_needs_the_same_resolution_and_never_rises(self):
        other_sizes = ["1920x1200@165.00Hz", "1920x1080@60.00Hz", "2560x1600@50.00Hz"]
        rotated = ["1920x1200@165.00Hz", "1200x1920@30.00Hz", "1920x1200@59.94Hz"]
        for rows in ((panel(availableModes=other_sizes),), (panel(transform=1, availableModes=rotated),),
                     (panel(refreshRate=50.0, availableModes=["1920x1200@50.00Hz"]),),
                     (panel(refreshRate=60.0, availableModes=["1920x1200@48.00Hz"]),),
                     (panel(transform=9),)):
            with self.subTest(row=rows[0]):
                self.check(travel_env(rows=rows), ["travel", "on"], ["travel", "off"])

    def test_omarchys_remembered_profiles_are_held_then_restored(self):
        cfg = {"ac": {"profile": "performance"}, "battery": {"profile": "balanced"}}
        self.check(travel_env(cfg=cfg), ["travel", "on"], ["travel", "off"])
        with Sandbox() as s:
            travel_env(cfg=cfg)(s)
            s.rust("travel", "on")
            held = [(s.home / ".local/state/omarchy/powerprofiles" / side).read_text() for side in ("ac", "battery")]
            self.assertEqual(held, ["power-saver\n"] * 2)

    def test_abm_and_effects_are_applied_then_restored(self):
        self.check(after(travel_env(), tweaks_on()), ["travel", "on"], ["travel", "off"])

    def test_tweaks_it_did_not_turn_on_stay(self):
        self.check(after(travel_env(), tweaks_on("3\n", (False, False))), ["travel", "on"], ["travel", "off"])
        self.check(after(travel_env(), tweaks_on("0\n", (True, False))), ["travel", "on"], ["travel", "off"])

    def test_tweak_that_cannot_be_turned_on_is_not_restored(self):
        def pkexec_fails(s):
            tweaks_on("0\n", (False, False))(s)  # abm is the only target, so nothing is saved after it is dropped
            s.stub("pkexec", "", 1, "denied")
        self.check(after(travel_env(), pkexec_fails), ["travel", "on"], ["travel", "off"])

    def test_unreadable_profile_is_skipped(self):
        self.check(travel_env(rows=(), profile_get="echo profile unavailable; exit 1"), ["travel", "on"])
        self.check(travel_env(profile_get="echo profile unavailable; exit 1"), ["travel", "on"], ["travel", "off"])

    def test_without_a_power_saver_profile_only_the_display_changes(self):
        self.check(travel_env(names=("balanced", "performance")), ["travel", "on"], ["travel", "off"])
        self.check(travel_env(names=("balanced", "performance"), rows=()), ["travel", "on"])

    def test_no_controls(self):
        self.check(travel_env(rows=(), names=("balanced",)), ["travel", "on"])
        with Sandbox() as s:
            travel_env(rows=(), names=("balanced",))(s)
            self.assertEqual(s.rust("travel", "on").stdout, "No supported battery-saving controls\n")

    def test_failing_power_saver_rolls_back_to_off_with_the_error(self):
        self.check(travel_env(profile_set=SAVER_DENIED), ["travel", "on"])
        self.check(travel_env(profile_set=SAVER_DENIED, rows=()), ["travel", "on"])

    def test_brightness_failing_after_the_profile_rolls_both_back(self):
        self.check(travel_env(set_body=CAP_REFUSED), ["travel", "on"], ["travel", "off"])

    def test_brightness_that_misses_the_cap_is_unverified(self):
        self.check(travel_env(set_body="echo $((${v%\\%}+2))"), ["travel", "on"])

    def test_monitor_change_failing_rolls_back(self):
        self.check(travel_env(replies=("error: nope", "ok")), ["travel", "on"])
        self.check(travel_env(replies=("error: nope",)), ["travel", "on"], ["travel", "off"])

    def test_monitor_change_failing_on_a_travel_off_retry(self):
        self.check(travel_env(replies=("ok", "error: nope", "ok")), ["travel", "on"], ["travel", "off"], ["travel", "off"])

    def test_rust_waits_for_the_mode_hyprland_applies_after_replying(self):
        """Hyprland answers eval at once and changes the mode about 0.7 s later."""
        with Sandbox() as s:
            travel_env()(s)

            def later(lua):
                rate = float(re.search(r'@([\d.]+)"', lua)[1])
                threading.Timer(0.75, lambda: s.monitors({**PANEL, "refreshRate": rate})).start()
            s.hypr_on_eval = later
            run = s.rust("travel", "on")
            self.assertEqual(run.stdout.strip(), "ok")

    def test_failed_restore_stays_incomplete_then_retries(self):
        def counted(s):
            n = s.base / "sets"
            return (f'n=$(cat {n} 2>/dev/null || echo 0); echo $((n+1)) > {n}; '
                    'if [ $n = 1 ]; then echo restore failed; false; else echo ${v%\\%}; fi')
        self.check(travel_env(set_body=counted), ["travel", "on"], ["travel", "off"], ["travel", "off"])

    def test_restore_that_cannot_set_the_profile_stays_incomplete(self):
        deny_restore = 'if [ "$v" = power-saver ]; then echo $v; else echo "authorization denied"; false; fi'
        self.check(travel_env(profile_set=deny_restore), ["travel", "on"], ["travel", "off"])
        self.check(travel_env(profile_set=deny_restore, set_body=CAP_REFUSED), ["travel", "on"])

    @unittest.skipIf(os.geteuid() == 0, "root ignores file modes")
    def test_unwritable_tweaks_file_keeps_effects_owned(self):
        def read_only(s):
            tweaks_on()(s)
            s.write(TWEAKS_FILE, "{}")
            s._target(TWEAKS_FILE).chmod(0o400)
        both(after(travel_env(), read_only), ["travel", "on"], ["travel", "off"], stdout=False, inspect=no_leftovers)
        with Sandbox() as s:
            after(travel_env(), read_only)(s)
            r = s.rust("travel", "on")
            self.assertEqual(r.code, 1)
            self.assertTrue(r.stdout.startswith("Travel operation could not save its recovery state: "))
            self.assertIn("tweak:effects", json.loads(s._target(TRAVEL_FILE).read_text())["pending"])

    @unittest.skipIf(os.geteuid() == 0, "root ignores file modes")
    def test_tweaks_file_that_turns_read_only_before_travel_off(self):
        def lock_after_on(py, rs):
            for s in (py, rs):
                if s._target(TRAVEL_FILE).exists() and not getattr(s, "locked", False):
                    s._target(TWEAKS_FILE).chmod(0o400)
                    s.locked = True
        both(after(travel_env(), tweaks_on()), ["travel", "on"], ["travel", "off"], stdout=False,
             inspect=lambda py, rs: (no_leftovers(py, rs), lock_after_on(py, rs)))
        with Sandbox() as s:
            after(travel_env(), tweaks_on())(s)
            s.rust("travel", "on")
            s._target(TWEAKS_FILE).chmod(0o400)
            r = s.rust("travel", "off")
            point = json.loads(s._target(TRAVEL_FILE).read_text())
            self.assertEqual((r.code, point["error"], "tweak:effects" in point["pending"]), (1, "", True))
            self.assertTrue(r.stdout.startswith("Travel operation could not save its recovery state: "))

    def test_malformed_state_is_kept_as_it_is(self):
        for raw in ("{not json", "[1]", '{"on": "yes"}', json.dumps(travel_snapshot(source=[]))):
            with self.subTest(raw=raw):
                self.check(travel_env(travel=raw), ["travel", "on"], ["travel", "off"])

    def test_malformed_pending_tokens_are_kept_as_they_are(self):
        for pending in (["epp:0"], ["refresh"], ["unknown"]):
            with self.subTest(pending=pending):
                self.check(travel_env(travel={**MALFORMED_PENDING_BASE, "pending": pending}), ["travel", "off"])
        for changes in ({"source": []}, {"cpu": {"turbo": None, "epp": [{"cpu": 0, "before": [], "target": None}]}}):
            self.check(travel_env(travel={**MALFORMED_PENDING_BASE, **changes, "pending": []}), ["travel", "off"])
        self.check(travel_env(travel={**MALFORMED_PENDING_BASE, "complete": True, "reduceCpu": False, "pending": [],
                                      "brightness": {"monitor": "eDP-1", "before": 80}}), ["travel", "off"])

    def test_a_point_with_nothing_pending_but_the_profile_restores_it(self):
        self.check(travel_env(travel={**MALFORMED_PENDING_BASE, "pending": []}, profile="power-saver"), ["travel", "off"])

    def test_display_only_source_transition(self):
        point = travel_snapshot(reduceCpu=False, profile=None, cpu={"turbo": None, "epp": []}, pending=["brightness"],
                                monitor=None, controls={**FROZEN, "refresh": {"available": False, "detail": "x"}})
        self.check(travel_env(names=("balanced",), travel=point), ["travel", "off"])

    def test_already_on_and_already_off_write_nothing(self):
        self.check(travel_env(travel=travel_snapshot()), ["travel", "on"])
        self.check(travel_env(), ["travel", "off"])

    def test_bad_arguments(self):
        for argv in (["travel"], ["travel", "maybe"], ["travel", "on", "nonsense"], ["travel", "off", "--reduce-cpu"]):
            with self.subTest(argv=argv):
                self.check(travel_env(), argv)
        with Sandbox() as s:
            r = s.rust("travel", "maybe")
            self.assertEqual((r.stdout, r.code), ("usage: travel on|off\n", 2))

    def test_older_restore_points_ask_for_a_reboot(self):
        legacy = {"on": True, "brightness": 80, "monitors": [{"name": "eDP-1", "width": 1920, "height": 1080, "refresh": 165,
                                                                "x": 0, "y": 0, "scale": 1}]}
        epp = travel_snapshot(cpu={"turbo": None, "epp": [{"cpu": 0, "before": "power", "target": None}]},
                              pending=["profile", "epp:0", "brightness", "refresh"])
        for point in (legacy, travel_snapshot(), epp):
            with self.subTest(point=point), Sandbox() as s:
                travel_env(travel=point)(s)
                before = s.snapshot()
                r = s.rust("travel", "off")
                self.assertEqual((r.stdout, r.code, s.snapshot()), (OLDER + "\n", 1, before))
                self.assertEqual([c for c in r.calls if "--no-osd" in c or " set " in c], [])
                self.assertEqual(r.evals, [])

    def test_restore_point_exists_before_the_first_change(self):
        with Sandbox() as s:
            seen = {}

            def probe(name):
                return lambda t: f'[ -e {s.base}/{name} ] || cp {s.run}/omarchy-power-hub/travel.json {s.base}/{name}; ' + t
            travel_env(set_body=lambda s_: probe("seen-brightness")("echo ${v%\\%}"),
                       profile_set=lambda s_: probe("seen-profile")("echo $v"))(s)
            mode = s.hypr_on_eval

            def on_eval(lua):
                seen.setdefault("eval", json.loads((s.run / "omarchy-power-hub/travel.json").read_text()))
                mode(lua)
            s.hypr_on_eval = on_eval
            self.assertEqual(s.rust("travel", "on").code, 0)
            seen["brightness"] = json.loads((s.base / "seen-brightness").read_text())
            seen["profile"] = json.loads((s.base / "seen-profile").read_text())
            self.assertEqual({k: (v["on"], v["complete"]) for k, v in seen.items()}, {k: (True, False) for k in ("eval", "brightness", "profile")})

    def test_unwritable_run_dir_changes_nothing(self):
        def attempt(prepare, prefix):
            with Sandbox() as s:
                travel_env()(s)
                run = s._target("run/omarchy-power-hub")
                prepare(run)
                try:
                    r = s.rust("travel", "on")
                finally:
                    if run.is_dir():
                        run.chmod(0o700)
                return r.stdout.startswith(prefix), r.code, [c for c in r.calls if "--no-osd" in c or " set " in c], r.evals

        def as_file(run):
            run.parent.mkdir(parents=True, exist_ok=True)
            run.write_text("")

        def read_only(run):
            run.mkdir(parents=True)
            (run / "energy-controls.lock").write_text("")
            run.chmod(0o500)

        self.assertEqual(attempt(as_file, "Travel operation could not save its recovery state: "), (True, 1, [], []))
        if os.geteuid():
            self.assertEqual(attempt(read_only, "Could not save Travel restore point: "), (True, 1, [], []))


WRAP = r'''#!/bin/bash
B={base}
if [[ "$*" == *--no-osd* || "$1" == set || "$0" == *pkexec ]]; then
  n=$(( $(cat $B/cnt 2>/dev/null || echo 0) + 1 )); echo $n > $B/cnt
  {{ echo "== $n $(basename $0) $*"; cat $B/run/omarchy-power-hub/travel.json 2>/dev/null; }} >> $B/trace
  k=$(cat $B/kill 2>/dev/null)
  if [ "$k" = "$n:before" ]; then kill -9 $PPID; exit 1; fi
fi
"$0.real" "$@"; rc=$?
if [ -n "$n" ] && [ "$k" = "$n:after" ]; then kill -9 $PPID; fi
exit $rc
'''


def watched(setup):
    """`setup` with every mutating external call (brightness set, profile set, pkexec, eval) logging travel.json
    to `trace` first, and killing the calling process at the call named in `kill` ("N:before" or "N:after")."""
    def instrumented(s):
        setup(s)
        for tool in ("omarchy", "powerprofilesctl", "pkexec"):
            stub = s.stubs / tool
            stub.rename(s.stubs / (tool + ".real"))
            stub.write_text(WRAP.format(base=s.base))
            stub.chmod(0o755)
        inner = s.hypr_on_eval

        def on_eval(lua):
            count = s.base / "cnt"
            n = int(count.read_text()) + 1 if count.exists() else 1
            count.write_text(str(n))
            point = s.run / "omarchy-power-hub/travel.json"
            with open(s.base / "trace", "a") as f:
                f.write(f"== {n} eval {lua}\n" + (point.read_text() if point.exists() else ""))
            kill = (s.base / "kill").read_text() if (s.base / "kill").exists() else ""
            if kill == f"{n}:before":
                kill_travel(s)
                return
            inner(lua)
            if kill == f"{n}:after":
                kill_travel(s)
        s.hypr_on_eval = on_eval
    return instrumented


def kill_travel(s):
    """SIGKILL this sandbox's ops.py or power-hub process, found by the sandbox root in its environment."""
    mark = f"POWER_HUB_ROOT={s.root}".encode()
    for proc in pathlib.Path("/proc").glob("[0-9]*"):
        try:
            argv = (proc / "cmdline").read_bytes().split(b"\0")
            if mark in (proc / "environ").read_bytes().split(b"\0") and (argv[0].endswith(b"power-hub") or any(a.endswith(b"/ops.py") for a in argv[:2])):
                os.kill(int(proc.name), signal.SIGKILL)
        except OSError:
            pass


def trace_of(s):
    path = s.base / "trace"
    lines = path.read_text().replace(str(s.base), "<b>").splitlines() if path.exists() else []
    return [json.dumps(json.loads(l)) if l.startswith("{") else l for l in lines]


def facts(s):
    point = s.run / "omarchy-power-hub/travel.json"
    return (json.loads(point.read_text()).get("on") if point.exists() else None,
            (s.base / "brightness.state").read_text().strip(), (s.base / "profile.state").read_text().strip())


class TravelRecovery(unittest.TestCase):
    """Python and Rust leave the same restore point at every mutating call, and recover the same after a SIGKILL."""
    def run_steps(self, setup, steps, kill=None):
        """Run `steps` in both; `kill` = (step index, "N:before"|"N:after"). Returns the Python sandbox's last call count."""
        with Sandbox() as py, Sandbox() as rs:
            for s in (py, rs):
                watched(setup)(s)
            for i, step in enumerate(steps):
                for s in (py, rs):
                    (s.base / "cnt").write_text("0")
                    (s.base / "kill").write_text(kill[1] if kill and kill[0] == i else "")
                a, b = py.settled(py.python(*step)), rs.settled(rs.rust(*step))
                label = f"step {i} {step} kill={kill}"
                seen = lambda r: (r.code, r.stdout, [c for c in r.calls if not c.startswith(DROPPED)], r.evals)  # noqa: E731
                self.assertEqual(seen(a), seen(b), label)
                assert_same_files(py, rs)
            self.assertEqual(trace_of(py), trace_of(rs), f"restore point at each call, kill={kill}")
            self.assertEqual(facts(py), facts(rs), f"kill={kill}")
            return int((py.base / "cnt").read_text())

    def test_restore_point_at_every_mutating_call(self):
        on, off = ["travel", "on"], ["travel", "off"]
        display_only = travel_snapshot(reduceCpu=False, profile=None, cpu={"turbo": None, "epp": []}, pending=["brightness"],
                                       monitor=None, controls={**FROZEN, "refresh": {"available": False, "detail": "x"}})
        for name, setup, steps in (
                ("plain", after(travel_env(), tweaks_on()), [on, off]),
                ("saver denied", travel_env(profile_set=SAVER_DENIED), [on]),
                ("cap refused", travel_env(set_body=CAP_REFUSED), [on, off]),
                ("mode fails, rollback works", travel_env(replies=("error: nope", "ok")), [on]),
                ("mode fails, rollback fails", travel_env(replies=("error: nope",)), [on, off]),
                ("travel off retried", travel_env(replies=("ok", "error: nope", "ok")), [on, off, off]),
                ("display-only point", travel_env(names=("balanced",), travel=display_only), [off])):
            with self.subTest(name):
                self.run_steps(setup, steps)

    def test_killed_travel_recovers_the_same(self):
        setup = after(travel_env(), tweaks_on())
        steps = [["travel", "on"], ["travel", "off"], ["travel", "off"]]
        for step in (0, 1):
            calls = self.run_steps(setup, steps[:step + 1])
            for n in range(1, calls + 1):
                for mode in ("before", "after"):
                    with self.subTest(step=step, kill=f"{n}:{mode}"):
                        self.run_steps(setup, steps, kill=(step, f"{n}:{mode}"))


CPU_NEW = {"cpu": {"turbo": None, "epp": []}, "pending": ["profile", "brightness", "refresh"]}
NO_PROFILE_CONTROL = travel_snapshot(profile=None, pending=["brightness", "refresh"], cpu=CPU_NEW["cpu"],
                                     controls={**FROZEN, "profile": {"available": False, "detail": "No profile"}})
APPLY_KEYS = {"ac": {"profile": "performance", "screensaver": 600, "lock": 900, "lid": "display-off"},
              "battery": {"profile": "balanced", "screensaver": 120, "lock": 240, "lid": "nothing"}}


def apply_env(travel=None, lid="open", on_battery=False, profile="balanced", keys=APPLY_KEYS):
    def setup(s):
        settings_setup(bindings="bind = x", battery=on_battery)(s)
        s.config(**keys)
        s.lid(lid)
        s.stub("pkexec", "", 0)
        s.stub("omarchy-powerprofiles-set", "", 0)
        s.stateful_profile(profile)
        if travel is not None:
            s.write(TRAVEL_FILE, json.dumps(travel))
    return setup


class ApplyParity(unittest.TestCase):
    def check(self, setup, *steps):
        both(setup, *steps, normalise=mask_exec, inspect=no_leftovers)

    @contextlib.contextmanager
    def checked(self, setup, step):
        self.check(setup, step)
        with Sandbox() as s:
            setup(s)
            yield s, s.rust(*step)

    def test_apply_profile_writes_profile_idle_and_lid_on_either_source(self):
        for battery, side, profile, saver in ((False, "ac", "performance", 600), (True, "battery", "balanced", 120)):
            with self.subTest(side=side), self.checked(apply_env(on_battery=battery), ["apply-profile"]) as (s, r):
                self.assertEqual((r.code, r.stdout), (0, "ok\n"))
                self.assertIn(f"omarchy-powerprofiles-set {side} {profile}", r.calls)
                self.assertEqual(shell_of(s)["idle"]["screensaver"], saver)
                self.assertIn("lid-closed", s._target(BINDINGS_FILE).read_text())

    def test_hibernate_delay_is_applied_only_above_zero(self):
        def setup(after):
            def inner(s):
                apply_env(keys={"ac": {**APPLY_KEYS["ac"], "hibernateAfter": after}})(s)
                s.install_helper()
                s.stub("busctl", "CanHibernate", 0, 's "yes"\n')
            return inner
        for after, expected in ((900, True), (0, False)):
            with self.subTest(after=after), self.checked(setup(after), ["apply-profile"]) as (_, r):
                self.assertEqual(any("hibernate-delay" in c for c in r.calls), expected)

    def test_travel_owning_the_cpu_keeps_the_profile_and_reapplies_the_overlay(self):
        with self.checked(apply_env(travel=travel_snapshot(**CPU_NEW), profile="performance"), ["apply-profile"]) as (_, r):
            self.assertEqual((r.code, r.stdout), (0, "ok\n"))
            self.assertIn("powerprofilesctl set power-saver", r.calls)
            self.assertFalse([c for c in r.calls if "omarchy-powerprofiles-set" in c])

    def test_travel_without_a_profile_control_applies_the_normal_profile_first(self):
        with self.checked(apply_env(travel=NO_PROFILE_CONTROL), ["apply-profile"]) as (_, r):
            self.assertIn("omarchy-powerprofiles-set ac performance", r.calls)
            self.assertFalse([c for c in r.calls if "powerprofilesctl set" in c])

    def test_normal_profile_that_is_not_offered_is_skipped(self):
        def setup(s):
            apply_env(travel=NO_PROFILE_CONTROL)(s)
            s.profiles(["power-saver", "balanced"])
        with self.checked(setup, ["apply-profile"]) as (_, r):
            self.assertFalse([c for c in r.calls if "omarchy-powerprofiles-set" in c])

    def test_incomplete_travel_shows_its_saved_error_and_still_writes_idle(self):
        for error in ("Power saver was replaced by balanced", ""):
            with self.subTest(error=error), self.checked(apply_env(travel=travel_snapshot(complete=False, error=error, **CPU_NEW)), ["apply-profile"]) as (s, r):
                self.assertEqual((r.code, r.stdout.startswith(error or "Travel mode restoration is incomplete")), (1, True))
                self.assertEqual(shell_of(s)["idle"]["screensaver"], 600)
                self.assertFalse([c for c in r.calls if "powerprofilesctl set" in c])

    def test_overlay_failure_marks_travel_incomplete(self):
        def setup(s):
            apply_env(travel=travel_snapshot(**CPU_NEW))(s)
            s._stateful("powerprofilesctl", 'case "$1" in get) echo balanced;; *) echo "authorization denied"; exit 1;; esac')
        with self.checked(setup, ["apply-profile"]) as (s, r):
            saved = json.loads(s._target(TRAVEL_FILE).read_text())
            self.assertEqual((r.code, r.stdout, saved["complete"], saved["error"]), (1, "authorization denied\n", False, "authorization denied"))

    def test_overlay_that_is_replaced_reports_the_replacement(self):
        def setup(s):
            apply_env(travel=travel_snapshot(**CPU_NEW))(s)
            s._stateful("powerprofilesctl", 'case "$1" in get) echo balanced;; *) ;; esac')
        with self.checked(setup, ["apply-profile"]) as (s, r):
            self.assertEqual(r.stdout, "Power saver was replaced by balanced\n")

    def test_unwritable_run_dir_ends_the_verb_with_nothing_to_record(self):
        with Sandbox() as py, Sandbox() as rs:
            for s in (py, rs):
                apply_env(travel=travel_snapshot(**CPU_NEW))(s)
                s._target(TRAVEL_FILE).parent.chmod(0o555)
            try:
                self.assertEqual((py.python("apply-profile").code, rs.rust("apply-profile").code), (1, 1))
            finally:
                for s in (py, rs):
                    s._target(TRAVEL_FILE).parent.chmod(0o755)

    def test_display_only_source_transition(self):
        point = travel_snapshot(reduceCpu=False, profile=None, cpu={"turbo": None, "epp": []}, pending=["brightness"],
                                monitor=None, controls={**FROZEN, "refresh": {"available": False, "detail": "x"}})
        with self.checked(apply_env(travel=point, on_battery=True), ["apply-profile"]) as (s, r):
            self.assertIn("omarchy-powerprofiles-set battery balanced", r.calls)
            self.assertTrue(json.loads(s._target(TRAVEL_FILE).read_text())["on"])

    def test_apply_wakes_the_keyboards_hid_only_with_the_lid_open(self):
        for lid, expected in (("open", True), ("closed", False)):
            with self.subTest(lid=lid), self.checked(apply_env(lid=lid), ["apply"]) as (_, r):
                self.assertEqual(any("hid-wakeup on" in c for c in r.calls), expected)

    def test_apply_joins_each_stage_error_with_spaces(self):
        def setup(s):
            apply_env(travel=travel_snapshot(complete=False, error="E", **CPU_NEW), lid="open")(s)
            s.write(SHELL_FILE, "{broken")
            s.write(TWEAKS_FILE, json.dumps({"wifi": True, "wifiPinned": True}))
            s.stub("pkexec", "", 1, "denied\n")
        with self.checked(setup, ["apply"]) as (_, r):
            self.assertEqual((r.code, r.stdout), (1, "E  shell.json unreadable denied denied\n"))

    def test_apply_is_ok_when_every_stage_is(self):
        with self.checked(apply_env(), ["apply"]) as (_, r):
            self.assertEqual((r.code, r.stdout), (0, "ok\n"))

    def test_unwritable_lid_file_ends_the_verb_like_an_uncaught_error(self):
        def setup(s):
            apply_env()(s)
            s._target(BINDINGS_FILE).chmod(0o444)
            s._target(BINDINGS_FILE).parent.chmod(0o555)
        for verb in ("apply", "apply-profile"):
            with self.subTest(verb=verb), Sandbox() as py, Sandbox() as rs:
                for s in (py, rs):
                    setup(s)
                a, b = py.python(verb), rs.rust(verb)
                for s in (py, rs):
                    s._target(BINDINGS_FILE).parent.chmod(0o755)
                self.assertEqual((a.code, b.code), (1, 1))
                self.assertEqual(a.calls and [c for c in a.calls if "hid-wakeup" in c], [c for c in b.calls if "hid-wakeup" in c])

    def test_unwritable_shell_json_ends_the_verb_like_an_uncaught_error(self):
        def setup(s):
            apply_env()(s)
            s._target(SHELL_FILE).chmod(0o444)
        with Sandbox() as py, Sandbox() as rs:
            for s in (py, rs):
                setup(s)
            a, b = py.python("apply"), rs.rust("apply")
            self.assertEqual((a.code, b.code), (1, 1))
            self.assertFalse([c for c in b.calls if "hid-wakeup" in c or "pkexec" in c])

    def test_older_restore_points_are_left_alone_by_apply(self):
        monitors = [{"name": "eDP-1", "width": 1920, "height": 1080, "refresh": 165, "x": 0, "y": 0, "scale": 1}]
        epp = travel_snapshot(cpu={"turbo": None, "epp": [{"cpu": 0, "before": "power", "target": None}]},
                              pending=["profile", "epp:0", "brightness", "refresh"])
        points = ({"on": True, "brightness": 80, "monitors": monitors},
                  {"on": True, "monitors": [], "complete": True, "controls": []},
                  travel_snapshot(), epp)
        for point in points:
            with self.subTest(point=point), Sandbox() as s:
                apply_env(travel=point)(s)
                remembered = s._target("home/.local/state/omarchy/powerprofiles")
                files = lambda: (s._target(TRAVEL_FILE).read_text(),  # noqa: E731
                                 {p.name: p.read_text() for p in remembered.glob("*")} if remembered.exists() else {})
                before = files()
                for verb in ("apply-profile", "apply"):
                    r = s.rust(verb)
                    self.assertIn(r.code, (0, 1), verb)
                    self.assertFalse([c for c in r.calls if "powerprofilesctl set" in c], verb)
                    self.assertEqual(files(), before, verb)
                r = s.rust("travel", "off")
                self.assertEqual((r.stdout, r.code), (OLDER + "\n", 1))


class HarnessTests(unittest.TestCase):
    def test_harness_sees_a_changed_file(self):
        with Sandbox() as a, Sandbox() as b:
            a.write("home/.config/omarchy/power-hub-tweaks.json", "{}")
            with self.assertRaises(AssertionError):
                assert_same_files(a, b)

    def test_harness_ignores_locks_temp_files_and_the_base_path(self):
        with Sandbox() as a, Sandbox() as b:
            for s in (a, b):
                s.write("run/omarchy-power-hub/energy-controls.lock", "")
                s.write("run/x.tmp", str(s.base))
                s.write("home/y.json", json.dumps({"p": str(s.base / "z")}))
            assert_same_files(a, b)

    def test_harness_json_key_order_and_types_matter(self):
        for left, right in (('{"z":1,"a":2}', '{"a":2,"z":1}'), ('{"a":true}', '{"a":1}'), ('{"a":1}', '{"a":1.0}')):
            with Sandbox() as a, Sandbox() as b:
                a.write("home/s.json", left)
                b.write("home/s.json", right)
                with self.assertRaises(AssertionError):
                    assert_same_files(a, b)
        with Sandbox() as a, Sandbox() as b:
            a.write("home/s.json", '{"a": 1}')
            b.write("home/s.json", '{"a":1}\n')
            assert_same_files(a, b)

    def test_harness_sees_a_symlink_target(self):
        with Sandbox() as a, Sandbox() as b:
            (a.home / "l").symlink_to("x")
            (b.home / "l").symlink_to("y")
            with self.assertRaises(AssertionError):
                assert_same_files(a, b)

    def test_harness_normalise_hook_fixes_file_content(self):
        with Sandbox() as a, Sandbox() as b:
            a.write("home/u.service", "ExecStart=python\n")
            b.write("home/u.service", "ExecStart=rust\n")
            with self.assertRaises(AssertionError):
                assert_same_files(a, b)
            assert_same_files(a, b, normalise=lambda path, text: text.split("=")[0])

    def test_harness_both_runs_a_verb_in_each_sandbox(self):
        both(lambda s: s.config(), ["config"], (["ac"], ["ac"]), stdout=False)

    def test_harness_both_fails_on_different_stdout(self):
        with self.assertRaises(AssertionError):
            both(lambda s: s.config(), (["ac"], ["config"]))

    def test_harness_stateful_stubs_remember_the_last_set(self):
        with Sandbox() as s:
            s.stateful_profile("balanced")
            s.stateful_brightness(70)
            s.stub("sudo", "", 0, "")
            for argv in (["powerprofilesctl", "set", "power-saver"], ["omarchy", "brightness", "display", "--no-osd", "--monitor", "eDP-1", "40%"]):
                subprocess.run(argv, env=s._env(), check=True)
            get = lambda *argv: subprocess.run(argv, env=s._env(), text=True, capture_output=True).stdout.strip()  # noqa: E731
            self.assertEqual((get("powerprofilesctl", "get"), get("omarchy", "brightness", "display", "--monitor", "eDP-1")), ("power-saver", "40"))

    def test_harness_hypr_on_eval_runs_before_the_reply(self):
        with Sandbox() as s:
            s.monitors({"name": "eDP-1"})
            s.hypr_on_eval = lambda lua: s.monitors({"name": lua})
            subprocess.run(["hyprctl", "eval", "X"], env=s._env(), check=True)
            self.assertEqual(json.loads(s.hypr["j/monitors"]), [{"name": "X"}])

    def test_rust_waits_for_the_energy_lock_python_holds(self):
        for argv in (["quick-dim", "off"], ["tweak", "turbo", "on"], ["cpu", "turbo", "on"]):
            with self.subTest(argv=argv), Sandbox() as s:
                s.config()
                with s.hold_flock("run/omarchy-power-hub/energy-controls.lock"):
                    p = s.rust_start(*argv)
                    time.sleep(0.2)
                    self.assertIsNone(p.poll())
                p.communicate(timeout=5)


DROPPERS_FILE = "home/.local/state/omarchy-power-hub/bus-droppers.json"
FLIP = "RTK FlipGo-A2 8R33926O00QS"
SAVER_CLEARED = 'hl.config({ cursor = { invisible = false } })'


def head(name, lit=True, **extra):
    return {"name": name, "dpmsStatus": lit, "width": 2560, "height": 1600, "refreshRate": 60, "x": 0, "y": 0, "scale": 1.6} | extra


def displays_setup(*monitors, state=None, droppers=None, lid="open", **keys):
    """A non-Framework machine, so the lights are a no-op."""
    def setup(s):
        settings_setup(**keys)(s)
        s.monitors(*monitors)
        s.lid(lid)
        if state is not None:
            s.write(STATE_FILE, state if isinstance(state, str) else json.dumps(state))
        if droppers is not None:
            s.write(DROPPERS_FILE, droppers if isinstance(droppers, str) else json.dumps(droppers))
    return setup


def same_requests(py, rs):
    """The exact socket traffic, reads and dispatches included, in order."""
    unittest.TestCase().assertEqual(py._requests, rs._requests)


class DisplayParity(unittest.TestCase):
    def test_on_restores_parked_heads_then_enables_every_monitor_and_clears_state(self):
        parked = [{"name": "DP-1", "output": "desc:" + FLIP, "width": 2560, "height": 1600, "refreshRate": 60, "x": 0, "y": 0, "scale": 1.6},
                  {"name": "DP-2", "width": 1920, "height": 1080}, "HDMI-A-1", {"name": "bad name"}]
        mons = [head("eDP-1", False), head("DP-1", False), head("DP-2", False, disabled=True), head("bad name")]

        def check(py, rs):
            same_requests(py, rs)
            self.assertEqual(rs.read_state(), {"disabled": []})
            self.assertEqual(rs._evals[0], SAVER_CLEARED)
            self.assertEqual(rs._evals[-1], 'hl.monitor({ output = "HDMI-A-1", disabled = false })')
            self.assertIn('dispatch hl.dsp.dpms({ action = "enable", monitor = "eDP-1" })', rs._requests)
            self.assertNotIn('dispatch hl.dsp.dpms({ action = "enable", monitor = "bad name" })', rs._requests)
        both(displays_setup(*mons, state={"disabled": parked}), ["displays", "on"], inspect=check)

    def test_on_applies_the_saver_the_displays_off_setting_allows(self):
        for keys in ({"screensaver": 150, "displaysOff": 200}, {"screensaver": 300, "displaysOff": 200}, {"displaysOff": 0}):
            with self.subTest(keys=keys):
                both(displays_setup(head("eDP-1"), **{"ac": {"profile": "balanced", "lock": 60, **keys}}), ["displays", "on"], ["displays", "off"], ["displays", "on"])

    def test_off_blanks_every_head_and_parks_the_ones_dpms_cannot_reach(self):
        flip = {"description": FLIP}
        shapes = {
            "one head": ([head("eDP-1")], None),
            "two heads": ([head("eDP-1"), head("DP-1")], None),
            "three heads": ([head("eDP-1"), head("DP-1"), head("DP-2")], None),
            "dropper": ([head("eDP-1"), head("DP-1", **flip)], [FLIP]),
            "dropper listed, not connected": ([head("eDP-1"), head("DP-1")], [FLIP]),
            "every head a dropper": ([head("DP-1", **flip), head("DP-2", **flip)], [FLIP]),
            "dropper with an unsafe description": ([head("eDP-1"), head("DP-1", description='bad" desc')], ['bad" desc']),
            "laptop panel is kept, listed second": ([head("DP-1"), head("eDP-1")], None),
            "deaf head, panel kept": ([head("eDP-1"), head("DP-1", True), head("DP-2", False)], None),
            "dark and disabled heads": ([head("eDP-1", False), head("DP-1", disabled=True), head("DP-2")], None),
            "unsafe name": ([head("eDP-1"), head("bad name"), head("DP-2", **flip)], [FLIP]),
            "no monitors": ([], [FLIP]),
        }
        for label, (mons, droppers) in shapes.items():
            for verb in ("off", "disable"):
                with self.subTest(shape=label, verb=verb):
                    both(displays_setup(*mons, droppers=droppers), ["displays", verb], inspect=same_requests)

    def test_off_keeps_heads_that_were_already_parked(self):
        parked = [{"name": "DP-9", "width": 100, "height": 100}, "DP-8", {"name": "DP-2", "width": 1}]
        both(displays_setup(head("eDP-1"), head("DP-2", description=FLIP), head("DP-3"), state={"disabled": parked}, droppers=[FLIP]),
             ["displays", "off"], ["displays", "off"], ["displays", "on"], inspect=same_requests)

    def test_off_writes_the_parked_heads_with_the_eight_snapshot_keys(self):
        def check(py, rs):
            self.assertEqual(rs.read_state(), py.read_state())
            self.assertEqual(list(rs.read_state()["disabled"][0]),
                             ["name", "output", "width", "height", "refreshRate", "x", "y", "scale"])
        both(displays_setup(head("eDP-1"), head("DP-1", description=FLIP, width=0, scale=0, refreshRate=None), droppers=[FLIP]),
             ["displays", "off"], inspect=check)

    def test_screensaver_is_dismissed_before_anything_else(self):
        def check(py, rs):
            self.assertEqual(rs._evals[0], SAVER_CLEARED)
            self.assertEqual([c for c in rs.calls_log.read_text().split("\n") if c.startswith("pkill")], ["pkill -x ttfx", "pkill -f [o]rg.omarchy.screensaver"])
        seen = []

        def setup(s):
            displays_setup(head("eDP-1"))(s)
            # What the pkill stub had logged when the cursor eval arrived.
            s.hypr_on_eval = lambda lua: seen.append((lua, [c for c in s.calls_log.read_text().split("\n") if c.startswith("pkill")]))
        for verb in ("on", "off"):
            seen.clear()
            both(setup, ["displays", verb], inspect=check)
            self.assertEqual(seen[0][0], SAVER_CLEARED)
            self.assertEqual({tuple(p) for _, p in seen[:2]}, {("pkill -x ttfx", "pkill -f [o]rg.omarchy.screensaver")})

    def test_park_a_head_that_came_back_lit_while_the_others_sleep(self):
        mons = [head("eDP-1", False), head("DP-1", description=FLIP), head("DP-2", description="b")]
        after_each = iter([(["DP-1"], [FLIP]), (["DP-1"], [FLIP]), (["DP-1", "DP-2"], [FLIP, "b"])])

        def check(py, rs):
            same_requests(py, rs)
            names, droppers = next(after_each)
            self.assertEqual(([s["name"] for s in rs.read_state()["disabled"]], json.loads(rs._target(DROPPERS_FILE).read_text())), (names, droppers))
        both(displays_setup(*mons), ["displays", "park", "DP-1"], ["displays", "park", "DP-1"],
             ["displays", "park", "DP-2"], inspect=check)

    def test_park_skips(self):
        cases = {
            "all lit": ([head("eDP-1"), head("DP-1")], "open", "DP-1"),
            "unknown": ([head("eDP-1", False), head("DP-1")], "open", "DP-7"),
            "unsafe name": ([head("eDP-1", False), head("bad name")], "open", "bad name"),
            "last head": ([head("eDP-1", disabled=True), head("DP-1")], "open", "DP-1"),
            "already disabled": ([head("eDP-1", False), head("DP-1", disabled=True)], "open", "DP-1"),
            "lid closed": ([head("eDP-1", False), head("DP-1")], "closed", "DP-1"),
            "no monitors": ([], "open", "DP-1"),
        }
        for label, (mons, lid, name) in cases.items():
            with self.subTest(label):
                def check(py, rs):
                    same_requests(py, rs)
                    self.assertEqual((rs._evals, rs.read_state()["disabled"] if rs._target(STATE_FILE).exists() else []), ([], []))
                both(displays_setup(*mons, lid=lid), ["displays", "park", name], inspect=check)

    def test_park_prints_parked(self):
        def check(py, rs):
            self.assertEqual(rs.read_state()["disabled"][0]["name"], "DP-1")
        both(displays_setup(head("eDP-1", False), head("DP-1")), ["displays", "park", "DP-1"], inspect=check)

    def test_dpms_is_an_alias(self):
        both(displays_setup(head("eDP-1"), head("DP-1")), ["dpms", "off"], ["dpms", "on"], ["dpms", "sync"], inspect=same_requests)

    def test_unopenable_lock_fails_closed(self):
        with Sandbox() as s:
            displays_setup(head("eDP-1"))(s)
            for verb in (["on"], ["off"], ["park", "DP-1"]):
                lock = s._target("run/omarchy-power-hub/display-off.lock")
                lock.mkdir(parents=True)
                r = s.rust("displays", *verb)
                lock.rmdir()
                self.assertEqual((r.code, r.stdout, r.evals), (1, "", []), verb)


class DisplayHardening(unittest.TestCase):
    """Corrupt state never stops a wake; Python raised on some of these."""

    STATES = ["", "nope", "[]", "5", '"x"', "null", '{"disabled": 1}', '{"disabled": "x"}', '{"disabled": {"a": 1}}',
              '[{"name": "DP-1"}]', '{"disabled": [null, 5, [1], {"name": "bad name"}, {"output": 3}]}']
    DROPPERS = ["", "nope", "{}", "5", '"x"', "[1, null, {}]", '[["a"]]']

    def wake(self, s):
        r = s.rust("displays", "on")
        self.assertEqual((r.code, r.stdout.strip()), (0, "ok"))
        for name in ("eDP-1", "DP-1"):
            self.assertIn(f'dispatch hl.dsp.dpms({{ action = "enable", monitor = "{name}" }})', s._requests)
        return r

    def test_on_enables_every_monitor_for_any_state_file(self):
        for text in self.STATES:
            with self.subTest(state=text), Sandbox() as s:
                displays_setup(head("eDP-1", False), head("DP-1", False), state=text)(s)
                self.wake(s)
                self.assertEqual(s.read_state(), {"disabled": []})

    def test_any_droppers_file_still_wakes_and_blanks(self):
        for text in self.DROPPERS:
            with self.subTest(droppers=text), Sandbox() as s:
                displays_setup(head("eDP-1", False), head("DP-1", False), droppers=text)(s)
                self.wake(s)
                self.assertEqual(s.rust("displays", "off").code, 0)

    def test_sync_of_a_non_list_is_idle(self):
        with Sandbox() as s:
            displays_setup(head("eDP-1"), state='{"disabled": 1}')(s)
            r = s.rust("displays", "sync")
            self.assertEqual((r.code, r.stdout.strip(), r.evals), (0, "idle", []))

    def test_replies_held_past_two_seconds_still_light_and_blank_every_head(self):
        """A read that gave up at 2 s would see no monitors and send no enable: the display would stay dark."""
        with Sandbox() as s:
            displays_setup(head("eDP-1", False))(s)
            s.reply_delay = 2.2
            r = s.rust("displays", "on")
            self.assertEqual((r.code, r.stdout.strip()), (0, "ok"))
            self.assertEqual(r.evals, [SAVER_CLEARED])
            self.assertEqual([q for q in s._requests if q.startswith("dispatch")], ['dispatch hl.dsp.dpms({ action = "enable", monitor = "eDP-1" })'])
            displays_setup(head("eDP-1"), head("DP-1"))(s)
            r = s.rust("displays", "off")
            self.assertEqual((r.code, r.stdout.strip()), (0, "ok"))
            self.assertEqual([q for q in s._requests if q.startswith("dispatch")],
                             [f'dispatch hl.dsp.dpms({{ action = "disable", monitor = "{n}" }})' for n in ("eDP-1", "DP-1")])
            self.assertEqual(r.evals, [SAVER_CLEARED, 'hl.monitor({ output = "DP-1", disabled = true })'])
            self.assertEqual([x["name"] for x in s.read_state()["disabled"]], ["DP-1"])

    def test_bad_modes_print_usage_and_touch_nothing(self):
        with Sandbox() as s:
            displays_setup(head("eDP-1", False), state={"disabled": [{"name": "DP-1"}]})(s)
            for argv in (["displays"], ["displays", "bogus"], ["displays", ""], ["displays", "park"], ["dpms", "ON"]):
                r = s.rust(*argv)
                self.assertEqual((r.code, r.evals, r.calls), (2, [], []), argv)
                self.assertTrue(r.stdout.startswith("usage: displays"), argv)
            self.assertEqual(s.read_state(), {"disabled": [{"name": "DP-1"}]})


KBLIGHT_FILE = "run/omarchy-power-hub/kblight.json"
LIGHT_TOOLS = ("qmk_hid", "framework_tool", "sudo")
LED = "sys/class/leds/chromeos:white:power"


def qmk_keys(s, level):
    """A Laptop 16 keyboard whose backlight rounds down to 0, 33, 67 or 100."""
    state = s.base / "keys.state"
    state.write_text(f"{level}\n")
    s._stateful("qmk_hid", '''case "$*" in
  "via --backlight") echo "Brightness: $(<STATE)%";;
  "via --backlight "*) v=$3; if [ "$v" -ge 100 ]; then n=100; elif [ "$v" -ge 67 ]; then n=67; elif [ "$v" -ge 34 ]; then n=33; else n=0; fi; echo $n > STATE;;
  *) exit 1;;
esac'''.replace("STATE", str(state)))


def ec_tool(s, keys=30, fp="Requested: Auto\nBrightness: 10%\n"):
    s.stub("framework_tool", "--fp-led-level", 0, "")
    s.stub("framework_tool", "--fp-brightness", 0, fp)
    s.stub("framework_tool", "--kblight", 0, f"Keyboard backlight: {keys}%\n")


def lights_setup(*tools, model="Laptop 16", vendor="Framework", led=None, lux=None, scale=None, daylight=False):
    def setup(s):
        displays_setup(head("eDP-1"))(s)
        s.path = str(s.stubs)  # the host's own light tools must not be found
        s.dmi(vendor, model)
        for tool in tools:
            tool(s)
        if led:
            s.power_led(*led)
        if lux is not None:
            s.light_sensor(lux, scale)
        if daylight:
            s.write("home/.config/omarchy/power-hub-tweaks.json", json.dumps({"daylight": True}))
    return setup


def keys_level(s):
    return int((s.base / "keys.state").read_text())


def led_file(s, name="brightness"):
    return (s.root / LED / name).read_text().strip()


def kblight_saved(s):
    p = s._target(KBLIGHT_FILE)
    return json.loads(p.read_text()) if p.exists() else None


def light_calls(s):
    return [c for c in s.calls_log.read_text().splitlines() if c.startswith(LIGHT_TOOLS)]


def same_lights(py, rs):
    """The fake keyboard and power button end up identical."""
    def lights(s):
        files = [*(s.root / LED).glob("*"), *s.base.glob("keys.state")]
        return {p.name: p.read_text() for p in files}
    unittest.TestCase().assertEqual(lights(py), lights(rs))


class LightsParity(unittest.TestCase):
    OFF, ON = ["displays", "off"], ["displays", "on"]

    def test_laptop_16_levels_survive_a_sleep_because_the_wake_writes_one_more(self):
        for step in (33, 67, 100):
            with self.subTest(step=step):
                both(lights_setup(lambda s: qmk_keys(s, step)), self.OFF,
                     lambda s: self.assertEqual((keys_level(s), kblight_saved(s)), (0, {"level": step})),
                     self.ON,
                     lambda s: self.assertEqual((keys_level(s), kblight_saved(s)), (step, None)),
                     inspect=same_lights)

    def test_laptop_16_wake_writes_the_level_plus_one(self):
        both(lights_setup(lambda s: qmk_keys(s, 67)), self.OFF, self.ON,
             lambda s: self.assertIn("qmk_hid via --backlight 68", s.calls_log.read_text()))

    def test_laptop_12_and_13_use_the_ec_keys_and_fingerprint_led(self):
        for model in ("Laptop 13 (Intel Core Ultra Series 1)", "Laptop 12"):
            with self.subTest(model=model):
                both(lights_setup(ec_tool, model=model), self.OFF,
                     lambda s: self.assertEqual(kblight_saved(s), {"level": 30, "fp": {"requested": "auto", "brightness": 10}}),
                     self.ON,
                     lambda s: self.assertEqual(light_calls(s), ["framework_tool --kblight 30", "framework_tool --fp-led-level auto"]))

    def test_ec_power_button_restores_a_custom_brightness(self):
        for fp, want in (("Requested: Custom\nBrightness: 55%\n", "framework_tool --fp-brightness 55"),
                         ("Brightness: 0%\n", "framework_tool --fp-brightness 0")):
            with self.subTest(fp=fp):
                both(lights_setup(lambda s: ec_tool(s, fp=fp), model="Laptop 13"), self.OFF, self.ON,
                     lambda s: self.assertEqual(light_calls(s)[-1], want))

    def test_a_failing_tool_is_retried_with_sudo_then_the_helper(self):
        def fails(s):
            s.stub("sudo", "ops-priv kblight", 0, "Keyboard backlight: 30%\n")
            s.stub("sudo", "ops-priv fp-brightness", 0, "Brightness: 10%\n")
        for installed in (False, True):
            with self.subTest(installed=installed):
                def setup(s, installed=installed):
                    lights_setup(fails, lambda s: s._write_stub("framework_tool"), model="Laptop 13")(s)
                    if installed:
                        s.install_helper()
                both(setup, self.OFF,
                     lambda s: self.assertIn(f"sudo -n {s.helper_dir if installed else s.bin}/ops-priv kblight 0", s.calls_log.read_text()),
                     self.ON)

    def test_sudo_alone_is_enough(self):
        both(lights_setup(lambda s: s.stub("sudo", "framework_tool --kblight", 0, "Keyboard backlight: 30%\n"),
                          lambda s: s._write_stub("framework_tool"), model="Laptop 13"), self.OFF,
             lambda s: self.assertEqual(kblight_saved(s), {"level": 30}),
             self.ON)

    def test_tool_errors_in_a_zero_exit_are_failures(self):
        for text in ("[ERROR] no", "Failed to communicate", "Not a Framework Laptop"):
            with self.subTest(text=text):
                both(lights_setup(lambda s: s.stub("framework_tool", "--kblight", 0, text), model="Laptop 13"), self.OFF,
                     lambda s: self.assertIsNone(kblight_saved(s)), self.ON)

    def test_a_failed_read_does_not_invent_a_restore_value(self):
        both(lights_setup(lambda s: s._write_stub("qmk_hid")), self.OFF,
             lambda s: self.assertIsNone(kblight_saved(s)),
             self.ON, lambda s: self.assertEqual(light_calls(s), []))

    def test_power_button_goes_dark_and_comes_back_with_its_trigger(self):
        both(lights_setup(lambda s: qmk_keys(s, 40), led=("chromeos-auto", 80, 100)), self.OFF,
             lambda s: self.assertEqual((led_file(s), kblight_saved(s)["fp"]),
                                        ("0", {"sys": True, "trigger": "chromeos-auto", "brightness": 80})),
             self.ON,
             lambda s: self.assertEqual((led_file(s), led_file(s, "trigger")), ("80", "chromeos-auto")),
             inspect=same_lights)

    def test_power_button_brightness_falls_back_to_the_maximum_then_100(self):
        for brightness, mx, want in (("junk", 60, 60), ("junk", "junk", 100), (200, 100, None), ("junk", 200, None)):
            with self.subTest(brightness=brightness, max=mx):
                both(lights_setup(lambda s: qmk_keys(s, 40), led=("chromeos-auto", brightness, mx)), self.OFF,
                     lambda s: self.assertEqual(kblight_saved(s)["fp"].get("brightness"), want),
                     self.ON, inspect=same_lights)

    def test_power_button_uses_tee_when_sysfs_is_root_only(self):
        def setup(s):
            lights_setup(lambda s: qmk_keys(s, 40), lambda s: s.stub("sudo", "tee", 0, ""), led=("chromeos-auto", 80, 100))(s)
            (s.root / LED / "brightness").chmod(0o444)
        both(setup, self.OFF,
             lambda s: self.assertIn(f"sudo -n tee {s.root}/{LED}/brightness", s.calls_log.read_text()),
             self.ON, inspect=same_lights)

    def test_the_first_power_button_in_name_order_wins(self):
        def setup(s):
            lights_setup(lambda s: qmk_keys(s, 40))(s)
            for name, value in (("chromeos:white:power", 11), ("chromeos:amber:power", 22), ("chromeos:blue:power", 33)):
                s.write(f"sys/class/leds/{name}/brightness", f"{value}\n")
        both(setup, self.OFF, lambda s: self.assertEqual(kblight_saved(s)["fp"]["brightness"], 22))

    def test_no_tools_and_no_power_button_touch_nothing(self):
        both(lights_setup(), self.OFF, self.ON, lambda s: self.assertEqual(light_calls(s), []))
        both(lights_setup(lambda s: s._write_stub("qmk_hid"), model="Laptop 13"), self.OFF, self.ON,
             lambda s: self.assertEqual(light_calls(s), []))

    def test_machines_that_are_not_a_framework_laptop_are_left_alone(self):
        for vendor, model in (("LENOVO", "Laptop 16"), ("Framework", "Desktop"), ("Framework", "Laptop 15")):
            with self.subTest(vendor=vendor, model=model):
                both(lights_setup(lambda s: qmk_keys(s, 40), lambda s: s._write_stub("framework_tool"),
                                  vendor=vendor, model=model, led=("chromeos-auto", 80, 100)),
                     self.OFF, self.ON,
                     lambda s: self.assertEqual((light_calls(s), led_file(s), kblight_saved(s)), ([], "80", None)))

    def test_vendor_matches_framework_in_any_case(self):
        both(lights_setup(lambda s: qmk_keys(s, 67), vendor="FRAMEWORK"), self.OFF,
             lambda s: self.assertEqual(keys_level(s), 0), self.ON)

    def test_daylight_keeps_the_keys_off_from_100_lux_up(self):
        for lux, scale, daylight, kept in ((3355, None, True, True), (100, None, True, True), (99, None, True, False),
                                           (None, None, True, False), (3355, None, False, False),
                                           (50, 2.5, True, True), (49, 2.0, True, False)):
            with self.subTest(lux=lux, scale=scale, daylight=daylight):
                both(lights_setup(lambda s: qmk_keys(s, 67), led=("chromeos-auto", 80, 100), lux=lux, scale=scale, daylight=daylight),
                     self.OFF, self.ON,
                     lambda s: self.assertEqual(
                         (keys_level(s), led_file(s), kblight_saved(s)),
                         (0, "80", {"level": 67, "held": True}) if kept else (67, "80", None)),
                     inspect=same_lights)

    def test_a_bright_wake_keeps_the_level_for_the_next_wake_in_a_darker_room(self):
        both(lights_setup(lambda s: qmk_keys(s, 67), led=("chromeos-auto", 80, 100), lux=3355, daylight=True),
             self.OFF, self.ON,
             lambda s: self.assertEqual((keys_level(s), kblight_saved(s)), (0, {"level": 67, "held": True})),
             self.OFF,
             lambda s: self.assertEqual(led_file(s), "0"),
             lambda s: s.light_sensor(37), self.ON,
             lambda s: self.assertEqual((keys_level(s), led_file(s), kblight_saved(s)), (67, "80", None)),
             inspect=same_lights)

    def test_a_level_set_by_hand_in_daylight_replaces_the_saved_one(self):
        both(lights_setup(lambda s: qmk_keys(s, 67), lux=3355, daylight=True),
             self.OFF, self.ON, lambda s: (s.base / "keys.state").write_text("33\n"), self.OFF,
             lambda s: s.light_sensor(37), self.ON,
             lambda s: self.assertEqual(keys_level(s), 33))

    def test_a_second_off_keeps_the_saved_light_levels(self):
        """A second off would read the darkened lights and lose the levels to restore."""
        saved = {"level": 67, "fp": {"sys": True, "trigger": "chromeos-auto", "brightness": 80}}
        both(lights_setup(lambda s: qmk_keys(s, 67), led=("chromeos-auto", 80, 100)), self.OFF,
             lambda s: self.assertEqual(kblight_saved(s), saved),
             self.OFF,
             lambda s: self.assertEqual(kblight_saved(s), saved),
             self.ON,
             lambda s: self.assertEqual((keys_level(s), led_file(s), led_file(s, "trigger")), (67, "80", "chromeos-auto")),
             inspect=same_lights)

    def test_a_wake_without_saved_levels_does_nothing(self):
        both(lights_setup(lambda s: qmk_keys(s, 67), led=("chromeos-auto", 80, 100)), self.ON,
             lambda s: self.assertEqual((light_calls(s), led_file(s)), ([], "80")))

    def test_saved_files_of_any_shape_wake_the_lights_or_nothing(self):
        for text in ("", "nope", "[]", "5", '"x"', "null", '{"level": 150}', '{"level": 40.0}', '{"level": true}', '{"fp": 3}',
                     '{"level": 40, "fp": {"requested": 5, "trigger": 5, "brightness": 200, "sys": "yes"}}',
                     '{"level": 40, "held": 1, "fp": {"requested": "custom", "brightness": 7}}'):
            with self.subTest(text=text):
                def setup(s, text=text):
                    lights_setup(lambda s: qmk_keys(s, 0), led=("chromeos-auto", 80, 100))(s)
                    s.write(KBLIGHT_FILE, text)
                both(setup, self.ON, inspect=same_lights)


if __name__ == "__main__":
    unittest.main()
