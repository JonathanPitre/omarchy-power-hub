#!/usr/bin/env python3
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bin"))
import ops  # noqa: E402

CFG = {
    "ac": {"screensaver": 60, "displaysOff": 300, "lock": 300, "sleep": 0,
           "hibernateAfter": 0, "lid": "nothing", "profile": "performance"},
    "battery": {"screensaver": 0, "displaysOff": 60, "lock": 60, "sleep": 300,
                "hibernateAfter": 60, "lid": "nothing", "profile": "balanced"},
}

# Keep Omarchy's remembered power profiles (~/.local/state/omarchy/powerprofiles)
# out of reach: ops writes them through XDG_STATE_HOME.
_state_home = None


def setUpModule():
    global _state_home
    _state_home = mock.patch.dict(os.environ, {"XDG_STATE_HOME": tempfile.mkdtemp()})
    _state_home.start()


def tearDownModule():
    _state_home.stop()


class IdleSetTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name)
        self.saved = {k: getattr(ops, k) for k in
                      ("CFG", "SHELL", "RUN", "HIBERNATE_DROPIN", "SYS_POWER_DISK",
                       "run", "priv",
                       "active_side", "listed_profiles", "write_lid_block")}
        ops.CFG = root / "cfg.json"
        ops.SHELL = root / "shell.json"
        ops.RUN = root / "run"
        ops.HIBERNATE_DROPIN = root / "90.conf"
        ops.SYS_POWER_DISK = root / "disk"
        ops.SYS_POWER_DISK.write_text("platform [shutdown]\n")
        ops.CFG.write_text(json.dumps(CFG))
        ops.SHELL.write_text(json.dumps({"idle": {"screensaver": ops.WEEK, "lock": 60}}))
        self.calls = []
        ops.run = lambda args, timeout=4: self.calls.append(list(args)) or (0, "yes")
        ops.priv = lambda args: self.calls.append(["priv", *args]) or ""
        ops.active_side = lambda cfg=None: "battery"
        ops.write_lid_block = lambda action: self.calls.append(["lid", action])

    def tearDown(self):
        for k, v in self.saved.items():
            setattr(ops, k, v)
        self.tmp.cleanup()

    def test_timer_click_on_active_side_touches_nothing_else(self):
        before = ops.SHELL.read_text()
        self.assertEqual(ops.set_key("battery", "displaysOff", "300"), "")
        self.assertEqual(ops.set_key("battery", "sleep", "1800"), "")
        self.assertEqual(self.calls, [])
        self.assertEqual(ops.SHELL.read_text(), before)
        cfg = json.loads(ops.CFG.read_text())
        self.assertEqual(cfg["battery"]["displaysOff"], 300)
        self.assertEqual(cfg["battery"]["sleep"], 1800)

    def test_lock_click_only_rewrites_shell_idle(self):
        self.assertEqual(ops.set_key("battery", "lock", "600"), "")
        self.assertEqual(self.calls, [])
        self.assertEqual(json.loads(ops.SHELL.read_text())["idle"]["lock"], 600)

    def test_hibernate_delay_skips_privilege_when_already_applied(self):
        ops.HIBERNATE_DROPIN.write_text("[Sleep]\nHibernateDelaySec=300\n")
        self.assertEqual(ops.set_key("battery", "hibernateAfter", "300"), "")
        self.assertEqual(self.calls, [])
        ops.set_key("battery", "hibernateAfter", "600")
        self.assertIn(["priv", "hibernate-delay", "600"], self.calls)

    def test_inactive_side_click_applies_nothing(self):
        ops.set_key("ac", "lock", "600")
        ops.set_key("ac", "hibernateAfter", "600")
        self.assertEqual(self.calls, [])

    def test_load_skips_profile_list_when_profiles_saved(self):
        def boom():
            raise AssertionError("listed_profiles called")
        ops.listed_profiles = boom
        cfg = ops.load()
        self.assertEqual(cfg["ac"]["profile"], "performance")
        self.assertEqual(cfg["battery"]["lock"], 60)


    def test_lid_defaults_to_sleep_and_migrates_system_default(self):
        ops.CFG.write_text(json.dumps({"ac": {"lid": "system-default"}}))
        cfg = ops.load()
        self.assertEqual(cfg["ac"]["lid"], "suspend")
        self.assertEqual(cfg["battery"]["lid"], "suspend")
        self.assertEqual(ops.set_key("ac", "lid", "system-default"), "bad lid action")


def run_model(expr):
    src = (ROOT / "Model.js").read_text().replace(".pragma library", "")
    script = src + f"\nconsole.log(JSON.stringify({expr}))"
    out = subprocess.run(["node", "-e", script], capture_output=True, text=True, check=True).stdout
    return json.loads(out)


@unittest.skipUnless(shutil.which("node"), "node not installed")
class IdleOptionTests(unittest.TestCase):
    def test_labels_read_like_windows_and_macos(self):
        self.assertEqual(run_model("[0, 60, 300, 150, 45, 3600, 7200, 5400].map(fmtIdleLong)"),
                         ["Never", "1 minute", "5 minutes", "2 min 30 s", "45 seconds",
                          "1 hour", "2 hours", "1 h 30 min"])

    def test_saved_value_off_the_list_is_inserted_in_order(self):
        opts = run_model("idleOptions(IDLE_STOPS, 150)")
        self.assertEqual([o["value"] for o in opts],
                         ["0", "60", "120", "150", "180", "300", "600", "900", "1200", "1800"])
        self.assertEqual(opts[3]["label"], "2 min 30 s")

    def test_saved_stop_is_not_duplicated(self):
        opts = run_model("idleOptions(HIBERNATE_STOPS, 3600)")
        self.assertEqual(len(opts), 17)
        self.assertEqual(run_model("idleOptions(HIBERNATE_STOPS, 60)")[1]["label"], "1 minute")


if __name__ == "__main__":
    unittest.main()
