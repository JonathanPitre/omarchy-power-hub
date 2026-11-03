#!/usr/bin/env python3
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402

# Keep Omarchy's remembered power profiles (~/.local/state/omarchy/powerprofiles)
# out of reach: ops writes them through XDG_STATE_HOME.
_state_home = None


def setUpModule():
    global _state_home
    _state_home = mock.patch.dict(os.environ, {"XDG_STATE_HOME": tempfile.mkdtemp()})
    _state_home.start()


def tearDownModule():
    _state_home.stop()


class EnergyControlsTests(unittest.TestCase):
    def test_brightness_read_uses_installed_display_read_form(self):
        calls = []

        def run(args, timeout=4):
            calls.append(args)
            return 0, "63\n"

        with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
            ops, "monitors", return_value=[{"name": "eDP-1"}]
        ):
            self.assertEqual(ops.brightness_get(), 63)
        self.assertEqual(calls, [["omarchy", "brightness", "display", "--monitor", "eDP-1"]])

    def test_brightness_read_rejects_failed_or_out_of_range_results(self):
        with mock.patch.object(ops, "monitors", return_value=[{"name": "eDP-1"}]):
            for result in ((1, "80"), (0, "brightness: 80%"), (0, "101"), (0, "-1")):
                with self.subTest(result=result), mock.patch.object(ops, "run", return_value=result):
                    self.assertIsNone(ops.brightness_get())

    def test_travel_changes_only_internal_output_and_restores_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            brightness = [82]
            live = [
                {"name": "eDP-1", "description": "Laptop", "width": 1920,
                 "height": 1080, "refreshRate": 165.0, "availableModes": ["1920x1080@60.00Hz"],
                 "x": 0, "y": 0, "scale": 1, "transform": 0, "disabled": False},
                {"name": "DP-1", "description": "External", "width": 2560,
                 "height": 1440, "refreshRate": 144.0, "x": 1920, "y": 0,
                 "scale": 1, "transform": 0, "disabled": False},
            ]
            calls = []

            def run(args, timeout=4):
                calls.append(args)
                if args[:3] == ["omarchy", "brightness", "display"]:
                    if args[-1].endswith("%"):
                        brightness[0] = int(args[-1][:-1])
                    return 0, str(brightness[0])
                if args[:2] == ["hyprctl", "eval"]:
                    if 'output = "desc:Laptop"' in args[-1]:
                        live[0]["refreshRate"] = float(args[-1].split("@", 1)[1].split('"', 1)[0])
                    return 0, "ok"
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", side_effect=lambda: [dict(row) for row in live]
                ), mock.patch.object(ops, "active_side", return_value="battery"), mock.patch.object(ops, "priv") as privileged:
                    cfg = {"battery": {"profile": "balanced"}}
                    self.assertEqual(ops.travel(True, cfg), "")
                    self.assertEqual(brightness[0], 40)
                    self.assertEqual(live[0]["refreshRate"], 60)
                    self.assertEqual(live[1]["refreshRate"], 144.0)
                    self.assertEqual(ops.travel(False, cfg), "")
                    self.assertEqual(brightness[0], 82)
                    self.assertEqual(live[0]["refreshRate"], 165.0)
                    self.assertEqual(live[1]["refreshRate"], 144.0)
                    self.assertFalse(any(call[:2] == ["powerprofilesctl", "set"] for call in calls))
                    privileged.assert_not_called()
            finally:
                ops.RUN = old_run
    def test_travel_does_not_increase_an_already_dim_display(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            brightness = [20]
            screens = [{"name": "eDP-1", "output": "eDP-1", "description": "",
                        "width": 1920, "height": 1080, "refreshRate": 60,
                        "x": 0, "y": 0, "scale": 1, "transform": 0,
                        "disabled": False}]

            def run(args, timeout=4):
                if args[:3] == ["omarchy", "brightness", "display"]:
                    if args[-1].endswith("%"):
                        brightness[0] = int(args[-1][:-1])
                    return 0, str(brightness[0])
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", return_value=screens
                ), mock.patch.object(
                    ops, "_cpu_available", return_value={"profile": True, "turbo": True, "epp": True}
                ), mock.patch.object(ops, "active_side", return_value="battery"), mock.patch.object(
                    ops, "priv"
                ) as privileged:
                    cfg = {"battery": {"profile": "balanced"}}
                    self.assertEqual(ops.travel(True, cfg), "")
                    self.assertEqual(brightness[0], 20)
                    self.assertEqual(ops.travel(False, cfg), "")
                    self.assertEqual(brightness[0], 20)
                    privileged.assert_not_called()
            finally:
                ops.RUN = old_run


    def test_quick_dim_caps_and_restores_brightness(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            brightness = [82]

            def run(args, timeout=4):
                if args[-1].endswith("%"):
                    brightness[0] = int(args[-1][:-1])
                    return 0, "ok"
                return 0, str(brightness[0])

            monitors = [{"name": "eDP-1", "description": "", "width": 1920,
                         "height": 1080, "refreshRate": 60, "x": 0, "y": 0,
                         "scale": 1, "transform": 0, "disabled": False}]
            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", return_value=monitors
                ):
                    self.assertEqual(ops.quick_dim(True), "")
                    self.assertEqual(brightness[0], 40)
                    self.assertEqual(ops.quick_dim(False), "")
                    self.assertEqual(brightness[0], 82)
                    brightness[0] = 20
                    self.assertEqual(ops.quick_dim(True), "")
                    self.assertEqual(brightness[0], 20)
                    self.assertEqual(ops.quick_dim(False), "")
                    self.assertEqual(brightness[0], 20)
            finally:
                ops.RUN = old_run

    def test_quick_dim_does_not_save_when_internal_monitor_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            appeared = [{"name": "eDP-1"}]
            try:
                with mock.patch.object(ops, "monitors", side_effect=[[], appeared, appeared]), mock.patch.object(
                    ops, "run", return_value=(0, "80")
                ):
                    self.assertEqual(ops.quick_dim(True), "Laptop display is unavailable")
                    self.assertFalse((ops.RUN / "quick-dim.json").exists())
            finally:
                ops.RUN = old_run

    def test_quick_dim_rejects_travel_ownership_without_losing_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            ops.RUN.mkdir()
            (ops.RUN / "travel.json").write_text(json.dumps({"on": True, "complete": True}))
            try:
                self.assertEqual(ops.quick_dim(True), "Turn off Travel mode before changing Quick dim")
                self.assertFalse((ops.RUN / "quick-dim.json").exists())
            finally:
                ops.RUN = old_run

    def test_quick_dim_and_travel_keep_nested_restore_points_independent(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            brightness = [82]
            screens = [{"name": "eDP-1", "output": "eDP-1", "description": "",
                        "width": 1920, "height": 1080, "refreshRate": 60,
                        "x": 0, "y": 0, "scale": 1, "transform": 0,
                        "disabled": False}]

            def run(args, timeout=4):
                if args[:3] == ["omarchy", "brightness", "display"]:
                    if args[-1].endswith("%"):
                        brightness[0] = int(args[-1][:-1])
                    return 0, str(brightness[0])
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", return_value=screens
                ):
                    cfg = {"battery": {"profile": "balanced"}}
                    self.assertEqual(ops.quick_dim(True), "")
                    self.assertEqual(brightness[0], 40)
                    self.assertEqual(ops.travel(True, cfg), "")
                    self.assertEqual(ops.quick_dim(False), "Turn off Travel mode before changing Quick dim")
                    self.assertTrue(ops.quick_dim_state()["on"])
                    self.assertEqual(ops.travel(False, cfg), "")
                    self.assertEqual(brightness[0], 40)
                    self.assertEqual(ops.quick_dim(False), "")
                    self.assertEqual(brightness[0], 82)
            finally:
                ops.RUN = old_run

    def test_travel_uses_power_saver_and_leaves_boost_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            old_run, old_cpu_root = ops.RUN, ops.CPU_ROOT
            ops.RUN, ops.CPU_ROOT = root / "run", root / "cpu"
            boost = ops.CPU_ROOT / "cpufreq/boost"
            boost.parent.mkdir(parents=True, exist_ok=True)
            boost.write_text("1\n")
            brightness = [85]
            profile = ["performance"]
            source = ["battery"]
            live = [{"name": "eDP-1", "description": "", "width": 1920,
                     "height": 1080, "refreshRate": 60.0, "x": 0, "y": 0,
                     "scale": 1.0, "transform": 0, "disabled": False}]

            def run(args, timeout=4):
                if args[:3] == ["omarchy", "brightness", "display"]:
                    if args[-1].endswith("%"):
                        brightness[0] = int(args[-1][:-1])
                    return 0, str(brightness[0])
                if args[:2] == ["hyprctl", "eval"]:
                    return 0, "ok"
                if args[0:2] == ["powerprofilesctl", "get"]:
                    return 0, profile[0]
                if args[0:2] == ["powerprofilesctl", "set"]:
                    # amd_pstate: with global boost off, power-profiles-daemon's per-policy
                    # boost write fails with EINVAL for every profile.
                    if boost.read_text().strip() == "0":
                        return 1, "Failed to activate CPU driver 'amd_pstate': Error writing '.../policy11/boost': Invalid argument (13)"
                    profile[0] = args[2]
                    return 0, ""
                if args[:2] == ["hyprctl", "monitors"]:
                    return 0, json.dumps(live)
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", return_value=live
                ), mock.patch.object(ops, "listed_profiles", return_value={"power-saver", "balanced", "performance"}), mock.patch.object(
                    ops, "active_side", side_effect=lambda *_: source[0]
                ), mock.patch.object(ops, "priv", return_value="") as priv, mock.patch.object(
                    ops, "shell_idle", return_value=""
                ), mock.patch.object(ops, "write_lid_block"), mock.patch.object(
                    ops, "apply_profile", return_value=""
                ) as apply_profile:
                    cfg = {
                        "battery": {"profile": "balanced", "screensaver": 0, "lock": 0, "lid": "nothing"},
                        "ac": {"profile": "performance", "screensaver": 0, "lock": 0, "lid": "nothing"},
                    }
                    self.assertEqual(ops.travel(True, cfg), "")
                    self.assertEqual(profile[0], "power-saver")
                    self.assertEqual(ops.travel_status()["controls"]["profile"]["available"], True)
                    # A power source change keeps Power saver while Travel is on.
                    source[0] = "ac"
                    profile[0] = "performance"
                    self.assertEqual(ops.apply_active(cfg), "")
                    self.assertEqual(profile[0], "power-saver")
                    self.assertEqual(ops.travel(False, cfg), "")
                    self.assertEqual(profile[0], "performance")
                    self.assertEqual(boost.read_text().strip(), "1")
                    priv.assert_not_called()
                    apply_profile.assert_not_called()
            finally:
                ops.RUN, ops.CPU_ROOT = old_run, old_cpu_root

    def test_display_only_source_transition_applies_current_normal_profile(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp)
            detail = {"available": False, "detail": "Unavailable"}
            state = {
                "on": True, "complete": True, "reduceCpu": False, "source": "battery",
                "profile": None, "brightness": {"monitor": "eDP-2", "before": 80},
                "monitor": None, "cpu": {"turbo": None, "epp": []}, "pending": ["brightness"],
                "controls": {key: dict(detail) for key in ("profile", "turbo", "epp", "brightness", "refresh")},
                "error": "",
            }
            state["controls"]["brightness"] = {"available": True, "detail": "Laptop brightness capped at 40%"}
            ops.atomic_state(ops.travel_path(), state)
            cfg = {
                "battery": {"profile": "balanced", "screensaver": 0, "lock": 0,
                            "hibernateAfter": 0, "lid": "nothing"},
                "ac": {"profile": "performance", "screensaver": 0, "lock": 0,
                       "hibernateAfter": 0, "lid": "nothing"},
            }
            try:
                with mock.patch.object(ops, "active_side", return_value="battery"), mock.patch.object(
                    ops, "apply_profile", return_value=""
                ) as apply_profile, mock.patch.object(ops, "shell_idle", return_value=""), mock.patch.object(
                    ops, "write_lid_block"
                ):
                    self.assertEqual(ops.apply_active(cfg), "")
                apply_profile.assert_called_once_with("battery", "balanced")
                self.assertTrue(ops.travel_state()["on"])
            finally:
                ops.RUN = old_run



    def test_invalid_travel_cli_arguments_exit_without_mutation(self):
        with mock.patch.object(ops, "travel") as travel:
            self.assertEqual(ops.main(["ops.py", "travel", "off", "--reduce-cpu"]), 2)
            self.assertEqual(ops.main(["ops.py", "travel", "on", "nonsense"]), 2)
            travel.assert_not_called()

    def test_epp_cpu_helper_rejects_unsafe_arguments_before_sysfs_access(self):
        helper = pathlib.Path(ops.__file__).with_name("ops-priv")
        for args in (["../etc", "power"], ["1x", "power"], ["0", "performance", "extra"]):
            with self.subTest(args=args):
                result = subprocess.run([str(helper), "epp-cpu", *args],
                                        capture_output=True, text=True, check=False)
                self.assertEqual(result.returncode, 2)
        missing = subprocess.run([str(helper), "epp-cpu", "999999", "power"],
                                 capture_output=True, text=True, check=False)
        self.assertEqual(missing.returncode, 1)
        self.assertIn("no cpu preference", missing.stderr)

    def test_failed_apply_rolls_back_brightness_and_records_off_state(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            brightness = [80]
            live = [{"name": "eDP-1", "description": "", "width": 1920,
                     "height": 1080, "refreshRate": 60.0, "x": 0, "y": 0,
                     "scale": 1.0, "transform": 0, "disabled": False}]

            def run(args, timeout=4):
                if args[:3] == ["omarchy", "brightness", "display"]:
                    if args[-1].endswith("%"):
                        brightness[0] = int(args[-1][:-1])
                        return (1, "apply failed") if brightness[0] == 40 else (0, "ok")
                    return 0, str(brightness[0])
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", return_value=live
                ), mock.patch.object(ops, "active_side", return_value="battery"):
                    self.assertIn("apply failed", ops.travel(True, {"battery": {"profile": "balanced"}}))
                    self.assertEqual(brightness[0], 80)
                    self.assertFalse(ops.travel_state()["on"])
                    self.assertEqual(ops.travel_status()["error"], "apply failed")
            finally:
                ops.RUN = old_run

    def test_travel_holds_omarchy_remembered_profiles_until_restore(self):
        # Omarchy's battery service runs `omarchy-powerprofiles-set <side>` on every
        # power-source change (and at shell start), which applies the profile stored in
        # ~/.local/state/omarchy/powerprofiles/<side>. Travel must hold those files.
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            state_dir = pathlib.Path(tmp) / "state" / "omarchy" / "powerprofiles"
            state_dir.mkdir(parents=True)
            (state_dir / "ac").write_text("performance\n")
            (state_dir / "battery").write_text("balanced\n")
            profile = ["balanced"]

            def omarchy_set(side):
                profile[0] = (state_dir / side).read_text().strip()

            def run(args, timeout=4):
                if args[:2] == ["powerprofilesctl", "get"]:
                    return 0, profile[0]
                if args[:2] == ["powerprofilesctl", "set"]:
                    profile[0] = args[2]
                    return 0, ""
                return 1, "unexpected command"

            cfg = {
                "battery": {"profile": "balanced", "screensaver": 0, "lock": 0, "lid": "nothing"},
                "ac": {"profile": "performance", "screensaver": 0, "lock": 0, "lid": "nothing"},
            }
            try:
                with mock.patch.dict(os.environ, {"XDG_STATE_HOME": str(pathlib.Path(tmp) / "state")}), \
                        mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "listed_profiles", return_value={"power-saver", "balanced", "performance"}
                ), mock.patch.object(ops, "active_side", return_value="battery"), mock.patch.object(
                    ops, "monitors", return_value=[]
                ), mock.patch.object(ops, "_cpu_available", return_value={"profile": True, "turbo": False, "epp": False}):
                    self.assertEqual(ops.travel(True, cfg), "")
                    omarchy_set("ac")
                    self.assertEqual(profile[0], "power-saver")
                    omarchy_set("battery")
                    self.assertEqual(profile[0], "power-saver")
                    self.assertEqual(ops.travel(False, cfg), "")
                    self.assertEqual(profile[0], "balanced")
                    self.assertEqual((state_dir / "ac").read_text().strip(), "performance")
                    self.assertEqual((state_dir / "battery").read_text().strip(), "balanced")
            finally:
                ops.RUN = old_run

    def test_travel_skips_profile_it_cannot_read(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            set_profiles = []

            def run(args, timeout=4):
                if args[:2] == ["powerprofilesctl", "get"]:
                    return 1, "profile unavailable"
                if args[:2] == ["powerprofilesctl", "set"]:
                    set_profiles.append(args[2])
                    return 0, ""
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "listed_profiles", return_value={"power-saver", "balanced", "performance"}
                ), mock.patch.object(ops, "active_side", return_value="battery"), mock.patch.object(
                    ops, "monitors", return_value=[]
                ):
                    self.assertEqual(ops.travel(True, {"battery": {"profile": "balanced"}}),
                                     "No supported battery-saving controls")
                    self.assertEqual(set_profiles, [])
                    self.assertFalse(ops.travel_state()["on"])
            finally:
                ops.RUN = old_run

    def test_power_saver_failure_rolls_back_to_off_state_with_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            profile = ["performance"]

            def run(args, timeout=4):
                if args[:2] == ["powerprofilesctl", "get"]:
                    return 0, profile[0]
                if args[:2] == ["powerprofilesctl", "set"]:
                    if args[2] == "power-saver":
                        return 1, "authorization denied"
                    profile[0] = args[2]
                    return 0, ""
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "listed_profiles", return_value={"power-saver", "balanced", "performance"}
                ), mock.patch.object(ops, "active_side", return_value="battery"), mock.patch.object(
                    ops, "monitors", return_value=[]
                ):
                    self.assertIn("authorization denied", ops.travel(True, {"battery": {"profile": "balanced"}}))
                    self.assertFalse(ops.travel_state()["on"])
                    self.assertEqual(profile[0], "performance")
                    self.assertEqual(ops.travel_status()["error"], "authorization denied")
            finally:
                ops.RUN = old_run

    def test_failed_restore_stays_incomplete_and_retries(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            brightness = [80]
            fail_restore = [False]
            live = [{"name": "eDP-1", "description": "", "width": 1920,
                     "height": 1080, "refreshRate": 60.0, "x": 0, "y": 0,
                     "scale": 1.0, "transform": 0, "disabled": False}]

            def run(args, timeout=4):
                if args[:3] == ["omarchy", "brightness", "display"]:
                    if args[-1].endswith("%"):
                        target = int(args[-1][:-1])
                        if fail_restore[0] and target == 80:
                            return 1, "restore failed"
                        brightness[0] = target
                        return 0, "ok"
                    return 0, str(brightness[0])
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "monitors", return_value=live
                ), mock.patch.object(ops, "active_side", return_value="battery"):
                    cfg = {"battery": {"profile": "balanced"}}
                    self.assertEqual(ops.travel(True, cfg), "")
                    fail_restore[0] = True
                    self.assertIn("restore failed", ops.travel(False, cfg))
                    self.assertEqual(ops.travel_state()["complete"], False)
                    fail_restore[0] = False
                    self.assertEqual(ops.travel(False, cfg), "")
                    self.assertEqual(brightness[0], 80)
                    self.assertFalse(ops.travel_state()["on"])
            finally:
                ops.RUN = old_run



    def test_travel_without_power_saver_profile_changes_only_the_display(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp) / "run"
            set_profiles = []

            def run(args, timeout=4):
                if args[:2] == ["powerprofilesctl", "get"]:
                    return 0, "balanced"
                if args[:2] == ["powerprofilesctl", "set"]:
                    set_profiles.append(args[2])
                    return 0, ""
                return 1, "unexpected command"

            try:
                with mock.patch.object(ops, "monitors", return_value=[]), mock.patch.object(
                    ops, "listed_profiles", return_value={"balanced", "performance"}
                ), mock.patch.object(ops, "run", side_effect=run), mock.patch.object(
                    ops, "active_side", return_value="battery"
                ):
                    self.assertEqual(ops.travel(True, {"battery": {"profile": "balanced"}}),
                                     "No supported battery-saving controls")
                    self.assertEqual(set_profiles, [])
            finally:
                ops.RUN = old_run

    def test_intel_no_turbo_node_uses_inverted_values(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_cpu_root = ops.CPU_ROOT
            ops.CPU_ROOT = pathlib.Path(tmp) / "sys"
            node = ops.CPU_ROOT / "intel_pstate/no_turbo"
            node.parent.mkdir(parents=True)
            try:
                node.write_text("0\n")
                self.assertIs(ops._turbo_value(), True)
                node.write_text("1\n")
                self.assertIs(ops._turbo_value(), False)
            finally:
                ops.CPU_ROOT = old_cpu_root

    def test_refresh_cap_requires_same_resolution_and_never_increases(self):
        current = {"width": 2560, "height": 1600, "refreshRate": 165,
                   "transform": 0,
                   "availableModes": ["1920x1080@60Hz", "2560x1600@50Hz"]}
        self.assertEqual(ops._target_refresh(current), 50)
        current["refreshRate"] = 50
        self.assertIsNone(ops._target_refresh(current))
        current["refreshRate"] = 165
        current["availableModes"] = ["1920x1080@60Hz"]
        self.assertIsNone(ops._target_refresh(current))

    def test_performance_only_epp_is_not_a_saving_capability(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_root = ops.CPU_ROOT
            ops.CPU_ROOT = pathlib.Path(tmp)
            pref = ops.CPU_ROOT / "cpu0/cpufreq"
            pref.mkdir(parents=True)
            (pref / "energy_performance_preference").write_text("performance\n")
            (pref / "energy_performance_available_preferences").write_text("performance\n")
            try:
                self.assertFalse(ops._epp_saving_supported())
            finally:
                ops.CPU_ROOT = old_root

    def test_malformed_travel_state_is_preserved_on_restore(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp)
            state = ops.RUN / "travel.json"
            state.write_text("{not json")
            try:
                error = ops.travel(False, {"battery": {"profile": "balanced"}})
                self.assertIn("unreadable", error)
                self.assertEqual(state.read_text(), "{not json")
            finally:
                ops.RUN = old_run

    def test_legacy_travel_restore_recovers_brightness_and_every_saved_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp)
            state_path = ops.RUN / "travel.json"
            saved_monitors = [
                {"name": "eDP-1", "width": 1920, "height": 1080, "refresh": 165,
                 "x": 0, "y": 0, "scale": 1},
                {"name": "HDMI-A-1", "width": 2560, "height": 1440, "refresh": 144,
                 "x": 1920, "y": 0, "scale": 1},
            ]
            state_path.write_text(json.dumps({"on": True, "brightness": 80,
                                               "monitors": saved_monitors}))
            brightness = [35]
            restored = []
            try:
                with mock.patch.object(ops, "brightness_set",
                                       side_effect=lambda monitor, value: brightness.__setitem__(0, value) or ""), mock.patch.object(
                    ops, "brightness_get", side_effect=lambda monitor: brightness[0]
                ), mock.patch.object(
                    ops, "_monitor_change", side_effect=lambda spec, refresh: restored.append(
                        (spec["name"], refresh)
                    ) or ""
                ), mock.patch.object(
                    ops, "active_side", return_value="ac"
                ), mock.patch.object(ops, "apply_profile", return_value="") as apply_profile:
                    self.assertEqual(ops.travel(False, {"ac": {"profile": "performance"}}), "")
                self.assertEqual(brightness[0], 80)
                self.assertEqual(restored, [("eDP-1", 165), ("HDMI-A-1", 144)])
                apply_profile.assert_called_once_with("ac", "performance")
                self.assertFalse(ops.travel_state()["on"])
            finally:
                ops.RUN = old_run

    def test_malformed_pending_restore_tokens_are_preserved(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp)
            state_path = ops.RUN / "travel.json"
            cfg = {"battery": {"profile": "balanced"}}
            control = {"available": False, "detail": "Unavailable"}
            base = {
                "on": True, "complete": False, "reduceCpu": True, "source": "battery",
                "profile": None, "brightness": None, "monitor": None,
                "cpu": {"turbo": None, "epp": []},
                "controls": {key: dict(control) for key in ("profile", "turbo", "epp", "brightness", "refresh")},
                "error": "",
            }
            try:
                for pending in (["epp:0"], ["refresh"], ["unknown"]):
                    with self.subTest(pending=pending):
                        state = dict(base, pending=pending)
                        raw = json.dumps(state)
                        state_path.write_text(raw)
                        error = ops.travel(False, cfg)
                        self.assertIn("malformed", error)
                        self.assertEqual(state_path.read_text(), raw)
                malformed_states = (
                    dict(base, source=[]),
                    dict(base, cpu={"turbo": None, "epp": [{"cpu": 0, "before": [], "target": None}]}),
                    dict(base, cpu={"turbo": None, "epp": [{"cpu": 0, "before": "power", "target": {}}]}),
                )
                for state in malformed_states:
                    with self.subTest(state=state):
                        raw = json.dumps(dict(state, pending=[]))
                        state_path.write_text(raw)
                        error = ops.travel(False, cfg)
                        self.assertIn("malformed", error)
                        self.assertEqual(state_path.read_text(), raw)
                state = dict(base, complete=True, reduceCpu=False,
                             brightness={"monitor": "eDP-1", "before": 80}, pending=[])
                raw = json.dumps(state)
                state_path.write_text(raw)
                error = ops.travel(False, cfg)
                self.assertIn("malformed", error)
                self.assertEqual(state_path.read_text(), raw)
            finally:
                ops.RUN = old_run

    def test_already_low_refresh_without_brightness_is_not_an_actionable_mode(self):
        with tempfile.TemporaryDirectory() as tmp:
            old_run = ops.RUN
            ops.RUN = pathlib.Path(tmp)
            screens = [{"name": "eDP-1", "output": "eDP-1", "width": 1920,
                        "height": 1080, "refreshRate": 50.0,
                        "availableModes": ["1920x1080@50Hz"], "x": 0, "y": 0,
                        "scale": 1, "transform": 0, "disabled": False}]
            try:
                with mock.patch.object(ops, "monitors", return_value=screens), mock.patch.object(
                    ops, "brightness_get", return_value=None
                ), mock.patch.object(
                    ops, "_cpu_available", return_value={"profile": False, "turbo": False, "epp": False}
                ):
                    status = ops.travel_status()
                    self.assertTrue(status["controls"]["refresh"]["available"])
                    self.assertIn("already at or below 60", status["controls"]["refresh"]["detail"])
                    self.assertEqual(ops.travel(True, {"battery": {"profile": "balanced"}}),
                                     "No supported battery-saving controls")
                    self.assertFalse((ops.RUN / "travel.json").exists())
            finally:
                ops.RUN = old_run

if __name__ == "__main__":
    unittest.main()
