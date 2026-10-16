#!/usr/bin/env python3
import pathlib
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "bin"))
import idle_wake  # noqa: E402
import ops  # noqa: E402


class HibernateAvailableTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name)
        self.saved = {k: getattr(ops, k) for k in
                      ("SYS_POWER_DISK", "LOCKDOWN", "CMDLINE", "PROC", "run")}
        ops.SYS_POWER_DISK = root / "disk"
        ops.LOCKDOWN = root / "lockdown"
        ops.CMDLINE = root / "cmdline"
        ops.PROC = root / "proc"
        ops.PROC.mkdir()
        ops.LOCKDOWN.write_text("[none] integrity confidentiality\n")
        ops.CMDLINE.write_text("root=/dev/mapper/root rw\n")
        self.busctl = 's "yes"\n'
        ops.run = lambda args, timeout=4: (0, self.busctl)

    def tearDown(self):
        for k, v in self.saved.items():
            setattr(ops, k, v)
        self.tmp.cleanup()

    def _proc(self, pid, cmdline):
        d = ops.PROC / str(pid)
        d.mkdir()
        (d / "cmdline").write_bytes(cmdline.replace(" ", "\0").encode() + b"\0")

    def test_disabled_names_bitwarden(self):
        ops.SYS_POWER_DISK.write_text("[disabled]\n")
        self._proc(2021, "/usr/lib/electron39/electron /usr/lib/bitwarden/app.asar")
        info = ops.hibernate_available()
        self.assertFalse(info["available"])
        self.assertIn("Bitwarden", info["reason"])
        self.assertIn("SECURE_KEY_CONTAINER_BACKEND=mlock", info["reason"])

    def test_disabled_lockdown(self):
        ops.SYS_POWER_DISK.write_text("[disabled]\n")
        ops.LOCKDOWN.write_text("none [integrity] confidentiality\n")
        info = ops.hibernate_available()
        self.assertFalse(info["available"])
        self.assertIn("lockdown", info["reason"].lower())

    def test_disabled_nohibernate(self):
        ops.SYS_POWER_DISK.write_text("[disabled]\n")
        ops.CMDLINE.write_text("root=/dev/mapper/root nohibernate rw\n")
        info = ops.hibernate_available()
        self.assertFalse(info["available"])
        self.assertIn("nohibernate", info["reason"])

    def test_disabled_secret_memory(self):
        ops.SYS_POWER_DISK.write_text("[disabled]\n")
        info = ops.hibernate_available()
        self.assertFalse(info["available"])
        self.assertIn("secret memory", info["reason"].lower())

    def test_logind_na_without_disk_disabled(self):
        ops.SYS_POWER_DISK.write_text("platform [shutdown]\n")
        self.busctl = 's "na"\n'
        info = ops.hibernate_available()
        self.assertFalse(info["available"])
        self.assertIn("swap", info["reason"].lower())
        self.assertNotIn('s "na"', info["reason"])

    def test_available(self):
        ops.SYS_POWER_DISK.write_text("platform [shutdown]\n")
        info = ops.hibernate_available()
        self.assertTrue(info["available"])
        self.assertEqual(info["reason"], "")


class LidHibernateFallbackTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = pathlib.Path(self.tmp.name)
        self.saved = {k: getattr(ops, k) for k in
                      ("CFG", "run", "priv", "load", "active_side")}
        self.saved_mice = idle_wake.set_mice_enabled
        self.saved_kb = idle_wake.set_keyboards_enabled
        ops.CFG = root / "cfg.json"
        self.calls = []

        def fake_run(args, timeout=4):
            self.calls.append(list(args))
            if args[:2] == ["systemctl", "hibernate"]:
                return 1, "not supported"
            return 0, "{}" if args[:2] == ["hyprctl", "devices"] else ""

        ops.run = fake_run
        ops.priv = lambda args: ""
        ops.load = lambda: {"battery": {"lid": "hibernate"}}
        ops.active_side = lambda cfg=None: "battery"
        idle_wake.set_mice_enabled = lambda *a, **k: None
        idle_wake.set_keyboards_enabled = lambda *a, **k: None

    def tearDown(self):
        for k, v in self.saved.items():
            setattr(ops, k, v)
        idle_wake.set_mice_enabled = self.saved_mice
        idle_wake.set_keyboards_enabled = self.saved_kb
        self.tmp.cleanup()

    def test_lid_hibernate_falls_back_to_suspend(self):
        self.assertEqual(ops.lid_closed(), 0)
        self.assertIn(["systemctl", "hibernate"], self.calls)
        self.assertIn(["systemctl", "suspend"], self.calls)


if __name__ == "__main__":
    unittest.main()
