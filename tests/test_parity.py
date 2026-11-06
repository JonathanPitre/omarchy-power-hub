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


if __name__ == "__main__":
    unittest.main()
