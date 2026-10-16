#!/usr/bin/env python3
import json
import os
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402


class ProfileTests(unittest.TestCase):
    def setUp(self):
        self._cfg = ops.CFG
        self._run = ops.RUN
        self._listed = ops.listed_profiles
        self._active = ops.active_side
        self._state_home = os.environ.get("XDG_STATE_HOME")
        ops.listed_profiles = lambda: {"power-saver", "balanced", "performance"}

    def tearDown(self):
        ops.CFG = self._cfg
        ops.RUN = self._run
        ops.listed_profiles = self._listed
        ops.active_side = self._active
        if self._state_home is None:
            os.environ.pop("XDG_STATE_HOME", None)
        else:
            os.environ["XDG_STATE_HOME"] = self._state_home

    def test_legacy_names_map_to_real_profiles(self):
        self.assertEqual(ops.canonical_profile("auto", "ac"), "balanced")
        self.assertEqual(ops.canonical_profile("saver", "battery"), "power-saver")
        self.assertEqual(ops.canonical_profile("full", "ac"), "performance")
        self.assertEqual(ops.canonical_profile("balanced", "battery"), "balanced")
        self.assertEqual(ops.canonical_profile("power-saver", "ac"), "power-saver")
        self.assertEqual(ops.canonical_profile("performance", "battery"), "performance")

    def test_missing_profile_defaults_by_side(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            ops.CFG = root / "cfg.json"
            ops.RUN = root / "run"
            ops.CFG.write_text(json.dumps({
                "ac": {"screensaver": 10},
                "battery": {"profile": "auto"},
            }))
            cfg = ops.load()
        self.assertEqual(cfg["ac"]["profile"], "performance")
        self.assertEqual(cfg["battery"]["profile"], "balanced")

    def test_ac_default_is_balanced_without_performance(self):
        ops.listed_profiles = lambda: {"balanced", "power-saver"}
        self.assertEqual(ops.default_profile("ac"), "balanced")
        self.assertEqual(ops.default_profile("battery"), "balanced")

    def test_inactive_profile_is_remembered_without_setting_live(self):
        calls = []
        original = ops.run
        ops.run = lambda args, timeout=4: calls.append(list(args)) or (0, "")
        ops.active_side = lambda cfg=None: "ac"
        try:
            with tempfile.TemporaryDirectory() as tmp:
                os.environ["XDG_STATE_HOME"] = tmp
                err = ops.apply_profile("battery", "full")
                saved = (pathlib.Path(tmp) / "omarchy" / "powerprofiles" / "battery").read_text()
        finally:
            ops.run = original
            ops.active_side = self._active
        self.assertEqual(err, "")
        self.assertEqual(saved.strip(), "performance")
        self.assertFalse(any(c and c[0] == "omarchy-powerprofiles-set" for c in calls))

    def test_idle_for_blank_mutes_screensaver_once(self):
        old_shell = ops.SHELL
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = pathlib.Path(tmp)
                ops.CFG = root / "cfg.json"
                ops.SHELL = root / "shell.json"
                ops.CFG.write_text(json.dumps({
                    "ac": {"screensaver": 60, "lock": 0, "displaysOff": 300,
                           "sleep": 0, "hibernateAfter": 0, "lid": "system-default",
                           "profile": "performance"},
                    "battery": {"screensaver": 0, "lock": 300, "displaysOff": 0,
                                "sleep": 0, "hibernateAfter": 0, "lid": "nothing",
                                "profile": "balanced"},
                }))
                ops.SHELL.write_text(json.dumps({"idle": {"screensaver": 60, "lock": 604800}}))
                ops.active_side = lambda cfg=None: "ac"
                self.assertEqual(ops.idle_for_blank(True), "")
                idle = json.loads(ops.SHELL.read_text())["idle"]
                self.assertEqual(idle["screensaver"], 7 * 24 * 3600)
                before = ops.SHELL.read_text()
                self.assertEqual(ops.idle_for_blank(True), "")
                self.assertEqual(ops.SHELL.read_text(), before)
                self.assertEqual(ops.idle_for_blank(False), "")
                self.assertEqual(json.loads(ops.SHELL.read_text())["idle"]["screensaver"], 60)
        finally:
            ops.SHELL = old_shell


if __name__ == "__main__":
    unittest.main()
