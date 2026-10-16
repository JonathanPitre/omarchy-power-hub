#!/usr/bin/env python3
import io
import os
import pathlib
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import logi  # noqa: E402
import ops  # noqa: E402

# Captured from an MX Anywhere 3S (slot 1, UNIFIED_BATTERY at feature 9) on a
# Bolt receiver while its USB-C cable went in and out.
MOUSE_CHARGING = bytes.fromhex("1101090041080101000000000000000000000000")
MOUSE_UNPLUGGED = bytes.fromhex("1101090041080000000000000000000000000000")


class BatteryTests(unittest.TestCase):
    def test_status_bytes_map_to_battery_vocabulary(self):
        cases = {0: "Discharging", 1: "Charging", 2: "Charging", 3: "Full", 4: "Charging", 5: "", 6: ""}
        for byte, want in cases.items():
            self.assertEqual(logi.battery(bytes([35, 4, byte, 0])), {"percent": 35, "status": want}, byte)

    def test_rejects_crossed_reply_shapes(self):
        self.assertIsNone(logi.battery(None))
        self.assertIsNone(logi.battery(b"\x41\x08"))
        self.assertIsNone(logi.battery(bytes([150, 8, 0, 0])))
        self.assertIsNone(logi.battery(bytes([50, 8, 9, 0])))

    def test_level_flags_stand_in_for_missing_percent(self):
        self.assertEqual(logi.battery(bytes([0, 4, 0, 0]))["percent"], 50)
        self.assertIsNone(logi.battery(bytes([0, 0, 0, 0])))


class ReportTests(unittest.TestCase):
    def test_battery_events_from_the_mouse(self):
        self.assertEqual(logi.report(MOUSE_CHARGING, {1: 9}),
                         ("battery", 1, {"percent": 65, "status": "Charging"}))
        self.assertEqual(logi.report(MOUSE_UNPLUGGED, {1: 9}),
                         ("battery", 1, {"percent": 65, "status": "Discharging"}))

    def test_replies_to_requests_are_ignored(self):
        reply = bytearray(MOUSE_CHARGING)
        reply[3] = 0x1B
        self.assertIsNone(logi.report(bytes(reply), {1: 9}))

    def test_other_features_are_ignored(self):
        self.assertIsNone(logi.report(MOUSE_CHARGING, {1: 7}))
        self.assertIsNone(logi.report(MOUSE_CHARGING, {}))

    def test_link_notices(self):
        self.assertEqual(logi.report(bytes.fromhex("10024110400c1b"), {}), ("link", 2, False))
        self.assertEqual(logi.report(bytes.fromhex("10024110000c1b"), {}), ("link", 2, True))

    def test_short_reads_are_ignored(self):
        self.assertIsNone(logi.report(b"\x10\x02\x41", {}))


class FakeDevice:
    """Answers HID++ 2.0 requests the way an MX Keys Mini does."""

    def __init__(self, name="MX Keys Mini", kind=0, battery=7, status=b"\x32\x04\x01\x01", cross=False):
        self.name, self.kind, self.batt, self.status, self.cross = name.encode(), kind, battery, status, cross
        self.calls = []
        self.lookups = 0

    def __call__(self, slot, rid, *params):
        self.calls.append((slot, rid) + params)
        if rid == 0x0000:
            feature = params[0] << 8 | params[1]
            if feature == 0x1004:
                self.lookups += 1
                if self.cross and self.lookups > 1:
                    return bytes([3])
                return bytes([self.batt])
            return bytes([2]) if feature == 0x0005 else b"\0"
        if rid == 0x0200:
            return bytes([len(self.name)])
        if rid == 0x0210:
            return self.name[params[0]:params[0] + 16]
        if rid == 0x0220:
            return bytes([self.kind])
        if rid == self.batt << 8 | 0x10:
            return self.status
        return None


class IdentifyTests(unittest.TestCase):
    def test_reads_name_kind_and_battery_slot(self):
        self.assertEqual(logi.identify(FakeDevice(), 2),
                         {"battery": 7, "name": "MX Keys Mini", "kind": "keyboard"})

    def test_long_names_arrive_in_chunks(self):
        dev = FakeDevice(name="Logitech MX Anywhere 3S for Mac", kind=3)
        self.assertEqual(logi.identify(dev, 1)["name"], "MX Anywhere 3S for Mac")
        self.assertEqual(logi.identify(dev, 1)["kind"], "pointer")

    def test_empty_slot(self):
        self.assertIsNone(logi.identify(lambda *a: None, 3))

    def test_device_without_unified_battery(self):
        with self.assertRaises(logi.Unsupported):
            logi.identify(FakeDevice(battery=0), 1)

    def test_crossed_reply_is_dropped(self):
        self.assertIsNone(logi.identify(FakeDevice(cross=True), 2))


class ReadSlotTests(unittest.TestCase):
    def test_known_device_costs_one_request(self):
        dev = FakeDevice()
        ident = {"battery": 7, "name": "MX Keys Mini", "kind": "keyboard"}
        got, row = logi.read_slot(dev, 2, ident)
        self.assertEqual(len(dev.calls), 1)
        self.assertEqual(got, ident)
        self.assertEqual(row, {"name": "MX Keys Mini", "kind": "keyboard", "percent": 50, "status": "Charging"})

    def test_stale_identity_is_refreshed(self):
        dev = FakeDevice()
        got, row = logi.read_slot(dev, 2, {"battery": 9, "name": "Old", "kind": "pointer"})
        self.assertEqual(got["battery"], 7)
        self.assertEqual(row["name"], "MX Keys Mini")

    def test_offline_device(self):
        self.assertEqual(logi.read_slot(lambda *a: None, 2, {"battery": 7, "name": "X", "kind": "keyboard"}),
                         (None, None))


class FallbackTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.saved = ops.RUN, ops.STATE, ops.solaar_devices, logi._solaar_base
        ops.RUN = pathlib.Path(self.tmp.name) / "run"
        ops.STATE = pathlib.Path(self.tmp.name) / "state"
        self.slow = [{"name": "Old Mouse", "percent": 40, "kind": "pointer", "status": ""}]
        ops.solaar_devices = lambda: self.slow

    def tearDown(self):
        ops.RUN, ops.STATE, ops.solaar_devices, logi._solaar_base = self.saved
        self.tmp.cleanup()

    def broken(self):
        raise ImportError("no solaar")

    def test_read_falls_back_without_solaar_library(self):
        logi._solaar_base = self.broken
        self.assertEqual(logi.read(), self.slow)

    def test_watch_falls_back_and_exits(self):
        logi._solaar_base = self.broken
        buf = io.StringIO()
        with redirect_stdout(buf):
            self.assertEqual(logi.watch(), 0)
        self.assertIn("Old Mouse", buf.getvalue())

    def test_read_falls_back_for_device_without_unified_battery(self):
        class Base:
            def receivers(self):
                return [type("Info", (), {"path": "/dev/hidraw9"})()]

            def open_path(self, path):
                return 1

            def close(self, h):
                pass

            def request(self, h, slot, rid, *params):
                return FakeDevice(battery=0)(slot, rid, *params)

        logi._solaar_base = Base
        self.assertEqual(logi.read(), self.slow)

    def test_read_caches_identity(self):
        dev = FakeDevice()

        class Base:
            def receivers(self):
                return [type("Info", (), {"path": "/dev/hidraw9"})()]

            def open_path(self, path):
                return 1

            def close(self, h):
                pass

            def request(self, h, slot, rid, *params):
                return dev(slot, rid, *params) if slot == 2 else None

        logi._solaar_base = Base
        rows = logi.read()
        self.assertEqual(rows, [{"name": "MX Keys Mini", "percent": 50, "kind": "keyboard", "status": "Charging"}])
        dev.calls.clear()
        logi.read()
        self.assertEqual([c for c in dev.calls if c[0] == 2], [(2, 0x0710)])


if __name__ == "__main__":
    unittest.main()
