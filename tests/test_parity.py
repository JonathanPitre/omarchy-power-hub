#!/usr/bin/env python3
import contextlib
import json
import os
import subprocess
import sys
import threading
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))
from parity_env import Sandbox, assert_same, assert_same_files, both, build_rust  # noqa: E402


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
        # No missing-tool case: the host's own /usr/bin/framework_tool stays on the sandbox PATH.
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
    unittest.TestCase().assertEqual(py_file, rs_file)
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
            self.assertEqual((r.stdout.strip(), f), ("restored", '{"disabled": []}\n'))
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


if __name__ == "__main__":
    unittest.main()


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

    @unittest.skip("passes once Task 5 adds quick-dim")
    def test_rust_waits_for_the_energy_lock_python_holds(self):
        with Sandbox() as s:
            with s.hold_flock("run/omarchy-power-hub/energy-controls.lock"):
                p = s.rust_start("quick-dim", "off")
                time.sleep(0.2)
                self.assertIsNone(p.poll())
            self.assertEqual(p.wait(timeout=5), 0)
