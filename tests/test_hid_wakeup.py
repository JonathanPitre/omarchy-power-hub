#!/usr/bin/env python3
import os
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import hid_wakeup as hw  # noqa: E402


def usb_dev(root, name, vendor="1d6b", product="0002", interfaces=(), wakeup=True):
    d = root / "bus" / "usb" / "devices" / name
    (d / "power").mkdir(parents=True)
    (d / "idVendor").write_text(vendor + "\n")
    (d / "idProduct").write_text(product + "\n")
    if wakeup:
        (d / "power" / "wakeup").write_text("disabled\n")
    for i, (cls, proto) in enumerate(interfaces):
        iface = d / f"{name}:{i}.0"
        iface.mkdir()
        (iface / "bInterfaceClass").write_text(f"{cls}\n")
        (iface / "bInterfaceProtocol").write_text(f"{proto}\n")
    return d


class HidWakeupTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)

    def tearDown(self):
        self._tmp.cleanup()

    def test_selects_mouse_and_logitech_receiver_not_xhci(self):
        usb_dev(self.root, "usb1", vendor="1d6b", product="0002", interfaces=[])
        usb_dev(self.root, "1-3", vendor="046d", product="c52b",
                interfaces=[("03", "01"), ("03", "02")])
        usb_dev(self.root, "1-4", vendor="1a2c", product="4c5e",
                interfaces=[("03", "02")])
        usb_dev(self.root, "1-8", vendor="0bda", product="8153",
                interfaces=[("ff", "00")])
        names = {p.parent.parent.name for p in hw.wakeup_nodes(self.root)}
        self.assertEqual(names, {"1-3", "1-4"})

    def test_selects_i2c_touchpad_wakeup(self):
        d = self.root / "bus" / "i2c" / "devices" / "i2c-PIXA3854:00"
        (d / "power").mkdir(parents=True)
        (d / "name").write_text("PIXA3854:00\n")
        (d / "power" / "wakeup").write_text("disabled\n")
        names = {p.parent.parent.name for p in hw.wakeup_nodes(self.root)}
        self.assertIn("i2c-PIXA3854:00", names)

    def test_lid_closed_disables_keyboard_and_pointer_hid(self):
        usb_dev(self.root, "1-2", vendor="32ac", product="0018",
                interfaces=[("03", "01")])
        usb_dev(self.root, "1-3", vendor="046d", product="c52b",
                interfaces=[("03", "01"), ("03", "02")])
        kb = self.root / "bus" / "usb" / "devices" / "1-2" / "power" / "wakeup"
        mouse = self.root / "bus" / "usb" / "devices" / "1-3" / "power" / "wakeup"
        hw.apply("on", self.root)
        hw.apply("off", self.root)
        self.assertEqual(kb.read_text().strip(), "disabled")
        self.assertEqual(mouse.read_text().strip(), "disabled")

    def test_apply_writes_enabled_or_disabled(self):
        usb_dev(self.root, "1-3", vendor="046d", product="c52b",
                interfaces=[("03", "02")])
        node = self.root / "bus" / "usb" / "devices" / "1-3" / "power" / "wakeup"
        hw.apply("on", self.root)
        self.assertEqual(node.read_text().strip(), "enabled")
        hw.apply("off", self.root)
        self.assertEqual(node.read_text().strip(), "disabled")


if __name__ == "__main__":
    unittest.main()
