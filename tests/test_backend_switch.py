#!/usr/bin/env python3
import hashlib
import os
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class BackendSwitch(unittest.TestCase):
    def test_hot_paths_use_the_binary(self):
        service = (ROOT / "Service.qml").read_text()
        for verb in ('"config"', '"boost-check"', '"led-sudo-needed"', '"helper-needed"',
                     '"idle-wake-poll"', '"displays", "sync"'):
            self.assertNotIn(f'"python3", root.ops, {verb}', service)
            self.assertIn(f'root.hub, {verb}', service)
        plan = (ROOT / "PlanSection.qml").read_text()
        self.assertIn('[root.hub, "status"]', plan)
        self.assertIn('[root.hub, "gpu"]', plan)
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn('"battery-history"]', panel)
        self.assertNotIn('"python3", root.planOpsBin, "battery-history"', panel)

    def test_only_stage_3_verbs_still_use_python(self):
        plan = (ROOT / "PlanSection.qml").read_text()
        self.assertNotIn("python3", plan)
        self.assertNotIn("ops.py", plan)
        stage3 = ('"displays"', '"recover-ucsi"', '"saver-past-inhibitor"', '"idle-lock"')
        for line in (ROOT / "Service.qml").read_text().splitlines():
            if '"python3"' in line:
                self.assertTrue(any(v in line for v in stage3), line)

    def test_energy_results_read_the_verb_from_the_new_position(self):
        plan = (ROOT / "PlanSection.qml").read_text()
        self.assertIn('act.command[1] === "travel" || act.command[1] === "quick-dim"', plan)
        self.assertNotIn("act.command[2]", plan)

    def test_dark_displays_are_not_blanked_again(self):
        service = (ROOT / "Service.qml").read_text()
        self.assertIn('out === "blank" && root.displaysOff && root.displaysIdle', service)
        self.assertNotIn('\n        if (root.displaysOff && root.displaysIdle', service)

    def test_poll_uses_the_binary(self):
        svc = (ROOT / "PowerService.qml").read_text()
        self.assertIn('[root.binDir + "/power-hub", "data"]', svc)
        self.assertFalse((ROOT / "bin/battery-plus-data").exists())
        self.assertIn("data::collect()", (ROOT / "src/action.rs").read_text())

    def test_binary_is_executable_and_matches_checksum(self):
        hub = ROOT / "bin/power-hub"
        self.assertTrue(os.access(hub, os.X_OK))
        want = (ROOT / "bin/power-hub.sha256").read_text().split()[0]
        self.assertEqual(hashlib.sha256(hub.read_bytes()).hexdigest(), want)

    def test_failed_backend_shows_one_clear_line(self):
        self.assertIn("property bool backendFailed", (ROOT / "PowerService.qml").read_text())
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn("service.backendFailed", panel)
        self.assertIn("Power Hub needs an x86-64 system to run its backend", panel)


if __name__ == "__main__":
    unittest.main()
