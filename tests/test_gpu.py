#!/usr/bin/env python3
import os
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402


def pci_dev(root, addr, cls="0x030000", boot_vga=None, runtime="active"):
    d = root / "bus" / "pci" / "devices" / addr
    (d / "power").mkdir(parents=True)
    (d / "class").write_text(cls + "\n")
    (d / "power" / "runtime_status").write_text(runtime + "\n")
    if boot_vga is not None:
        (d / "boot_vga").write_text(boot_vga + "\n")


class GpuStatusTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def test_suspended_discrete_gpu_is_asleep(self):
        pci_dev(self.root, "0000:c1:00.0", boot_vga="1")
        pci_dev(self.root, "0000:c2:00.0", boot_vga="0", runtime="suspended")
        self.assertEqual(ops.gpu_status(self.root), {"present": True, "asleep": True})

    def test_active_discrete_gpu_is_awake(self):
        pci_dev(self.root, "0000:c1:00.0", boot_vga="1", runtime="suspended")
        pci_dev(self.root, "0000:c2:00.0", cls="0x030200", boot_vga="0")
        self.assertEqual(ops.gpu_status(self.root), {"present": True, "asleep": False})

    def test_integrated_only_is_absent(self):
        pci_dev(self.root, "0000:00:02.0", boot_vga="1")
        pci_dev(self.root, "0000:00:03.0")
        pci_dev(self.root, "0000:01:00.0", cls="0x020000", boot_vga="0")
        self.assertFalse(ops.gpu_status(self.root)["present"])

    def test_missing_pci_bus_is_absent(self):
        self.assertFalse(ops.gpu_status(self.root)["present"])


if __name__ == "__main__":
    unittest.main()
