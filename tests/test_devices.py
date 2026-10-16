#!/usr/bin/env python3
import json
import os
import pathlib
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402

SOLAAR = """\
  Serial       : ABC
  1: MX Anywhere 3S
     Codename     : MX Anywhere 3
         9: UNIFIED BATTERY        {1004} V3
            Battery: 80%, BatteryStatus.DISCHARGING.
     Battery: 80%, BatteryStatus.DISCHARGING.
  2: MX Keys Mini
     Codename     : MX Keys Mini
     Battery: 45%, BatteryStatus.DISCHARGING.
"""

HS_UNAVAILABLE = """\
Found 1 supported device(s):
 SteelSeries Arctis Nova Pro Wireless (Arctis Nova Pro Wireless) [0x1038:0x12e0]
Battery:
	Status: BATTERY_UNAVAILABLE
Error: [battery] Device is offline or not responding
"""

HS_OK = """\
Found 1 supported device(s):
 SteelSeries Arctis Nova Pro Wireless (Arctis Nova Pro Wireless) [0x1038:0x12e0]
Battery:
	Level: 72%
	Status: BATTERY_AVAILABLE
"""

HS_JSON_UNAVAILABLE = """\
{"devices":[{"device":"SteelSeries Arctis Nova Pro Wireless","battery":{"status":"BATTERY_UNAVAILABLE","level":-1}}]}
"""

HS_JSON_OK = """\
{"devices":[{"device":"SteelSeries Arctis Nova Pro Wireless","battery":{"status":"BATTERY_AVAILABLE","level":64}}]}
"""


class MergeDevicesTests(unittest.TestCase):
    def test_dedupes_by_name(self):
        rows = ops.merge_devices(
            [{"name": "Arctis", "percent": 10, "kind": "headset"}],
            [{"name": "Arctis", "percent": 10, "kind": "headset"}],
        )
        self.assertEqual(len(rows), 1)

    def test_orders_keyboard_then_mouse_then_headset(self):
        rows = ops.merge_devices(
            [{"name": "MX Anywhere 3S", "percent": 80, "kind": "pointer"}],
            [{"name": "Arctis", "percent": 87, "kind": "headset"}],
            [{"name": "MX Keys Mini", "percent": 45, "kind": "keyboard"}],
        )
        self.assertEqual([r["name"] for r in rows], ["MX Keys Mini", "MX Anywhere 3S", "Arctis"])


class ParseSolaarTests(unittest.TestCase):
    def test_solaar_maps_mouse_and_keyboard_percent(self):
        rows = ops.parse_solaar(SOLAAR)
        self.assertEqual(
            rows,
            [
                {"name": "MX Anywhere 3S", "percent": 80, "kind": "pointer", "status": "Discharging"},
                {"name": "MX Keys Mini", "percent": 45, "kind": "keyboard", "status": "Discharging"},
            ],
        )

    def test_solaar_status_maps_to_battery_vocabulary(self):
        cases = {"RECHARGING": "Charging", "SLOW_RECHARGE": "Charging", "ALMOST_FULL": "Charging",
                 "FULL": "Full", "DISCHARGING": "Discharging", "THERMAL_ERROR": ""}
        for raw, want in cases.items():
            text = f"  2: MX Keys Mini\n     Battery: 35%, BatteryStatus.{raw}.\n"
            self.assertEqual(ops.parse_solaar(text)[0]["status"], want, raw)

    def test_solaar_summary_battery_line_wins(self):
        text = (
            "  1: MX Anywhere 3S\n"
            "         9: UNIFIED BATTERY        {1004} V3\n"
            "            Battery: 65%, BatteryStatus.DISCHARGING.\n"
            "     Battery: 65%, BatteryStatus.RECHARGING.\n"
        )
        rows = ops.parse_solaar(text)
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["status"], "Charging")

    def test_solaar_status_survives_merge(self):
        text = "  2: MX Keys Mini\n     Battery: 35%, BatteryStatus.RECHARGING.\n"
        self.assertEqual(ops.merge_devices(ops.parse_solaar(text))[0]["status"], "Charging")

    def test_solaar_strips_logitech_prefix(self):
        text = "  1: Logitech G502\n     Kind : mouse\n     Battery: 50%\n"
        self.assertEqual(ops.parse_solaar(text)[0]["name"], "G502")

    def test_solaar_skips_nul_name_from_receiver_race(self):
        text = "  1: \0\0\0\0\0\0\n     Kind : mouse\n     Battery: 75%\n"
        self.assertEqual(ops.parse_solaar(text), [])

    def test_solaar_skips_device_without_battery(self):
        text = "  1: MX Keys Mini\n     Codename : MX Keys Mini\n"
        self.assertEqual(ops.parse_solaar(text), [])

    def test_solaar_kind_field_beats_name(self):
        text = (
            "  1: Office Board\n"
            "     Kind         : keyboard\n"
            "     Battery: 12%, BatteryStatus.DISCHARGING.\n"
        )
        self.assertEqual(ops.parse_solaar(text)[0]["kind"], "keyboard")

    def test_solaar_mouse_kind_is_pointer_not_mouse(self):
        rows = ops.parse_solaar(SOLAAR)
        self.assertIn("pointer", [r["kind"] for r in rows])
        self.assertNotIn("mouse", [r["kind"] for r in rows])


class ParseHeadsetTests(unittest.TestCase):
    def test_unavailable_text_is_empty(self):
        self.assertEqual(ops.parse_headsetcontrol(HS_UNAVAILABLE), [])

    def test_does_not_take_found_count_as_percent(self):
        rows = ops.parse_headsetcontrol(HS_UNAVAILABLE)
        self.assertFalse(any(r.get("percent") == 1 for r in rows))

    def test_level_percent_from_text(self):
        self.assertEqual(
            ops.parse_headsetcontrol(HS_OK),
            [{"name": "Arctis Nova Pro Wireless", "percent": 72, "kind": "headset", "status": "Discharging"}],
        )

    def test_charging_status_from_text_and_json(self):
        self.assertEqual(ops.parse_headsetcontrol(HS_OK.replace("AVAILABLE", "CHARGING"))[0]["status"], "Charging")
        self.assertEqual(ops.parse_headsetcontrol(HS_JSON_OK.replace("AVAILABLE", "CHARGING"))[0]["status"], "Charging")

    def test_json_unavailable_is_empty(self):
        self.assertEqual(ops.parse_headsetcontrol(HS_JSON_UNAVAILABLE), [])

    def test_json_level(self):
        self.assertEqual(
            ops.parse_headsetcontrol(HS_JSON_OK),
            [{"name": "Arctis Nova Pro Wireless", "percent": 64, "kind": "headset", "status": "Discharging"}],
        )


class SolaarSharedCacheTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self._run, self._RUN, self._STATE, self._which = ops.run, ops.RUN, ops.STATE, ops.shutil.which
        ops.RUN = pathlib.Path(self.tmp.name) / "run"
        ops.STATE = pathlib.Path(self.tmp.name) / "state"
        ops.shutil.which = lambda n: "/usr/bin/" + n
        self.calls = 0

    def tearDown(self):
        ops.run, ops.RUN, ops.STATE, ops.shutil.which = self._run, self._RUN, self._STATE, self._which
        self.tmp.cleanup()

    def test_last_good_devices_survive_reboot(self):
        self.fake(0, SOLAAR)
        good = ops.solaar_devices()
        shutil.rmtree(ops.RUN)
        cache = ops.STATE / "solaar.json"
        cache.write_text(json.dumps({"t": 0, "rows": good}))
        self.fake(1, "  1: MX Keys Mini\nsolaar: error: Traceback\nKeyError: False\n")
        self.assertEqual(ops.solaar_devices(), good)

    def fake(self, code, out):
        def run(args, timeout=4):
            self.calls += 1
            return code, out
        ops.run = run

    def test_fresh_result_is_shared_without_rerunning_solaar(self):
        self.fake(0, SOLAAR)
        first = ops.solaar_devices()
        second = ops.solaar_devices()
        self.assertEqual(first, second)
        self.assertEqual(self.calls, 1)

    def test_failed_run_keeps_last_good_devices(self):
        self.fake(0, SOLAAR)
        good = ops.solaar_devices()
        cache = ops.STATE / "solaar.json"
        data = json.loads(cache.read_text())
        data["t"] = 0
        cache.write_text(json.dumps(data))
        self.fake(1, "solaar: error: Traceback\nFeatureCallError")
        self.assertEqual(ops.solaar_devices(), good)
        self.assertEqual(len(good), 2)

    def test_traceback_with_exit_zero_is_not_cached(self):
        self.fake(0, SOLAAR)
        good = ops.solaar_devices()
        cache = ops.STATE / "solaar.json"
        cache.write_text(json.dumps({"t": 0, "rows": good}))
        corrupt = SOLAAR.replace("MX Anywhere 3S", "MX Anywhere 3").split("  2:")[0]
        self.fake(0, corrupt + "solaar: error: Traceback (most recent call last):\nKeyError: False\n")
        self.assertEqual(ops.solaar_devices(), good)
        self.assertEqual(json.loads(cache.read_text())["rows"], good)

    def test_partial_failed_run_does_not_replace_last_good(self):
        self.fake(0, SOLAAR)
        good = ops.solaar_devices()
        cache = ops.STATE / "solaar.json"
        cache.write_text(json.dumps({"t": 0, "rows": good}))
        mouse_only = SOLAAR[SOLAAR.index("  2:"):]
        self.fake(1, mouse_only + "\nTypeError: 'NoneType' object")
        self.assertEqual(ops.solaar_devices(), good)
        self.assertEqual(json.loads(cache.read_text())["rows"], good)


class DepsNeededTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.tmp.name)

    def tearDown(self):
        self.tmp.cleanup()

    def add_usb(self, name, vendor):
        d = self.root / name
        d.mkdir()
        (d / "idVendor").write_text(vendor + "\n")

    def test_logitech_needs_solaar_when_missing(self):
        self.add_usb("1-2", "046d")
        self.assertEqual(ops.deps_needed(self.root, which=lambda _: None), ["solaar"])

    def test_steelseries_needs_headsetcontrol_when_missing(self):
        self.add_usb("1-3", "1038")
        self.assertEqual(ops.deps_needed(self.root, which=lambda _: None), ["headsetcontrol"])

    def test_skips_installed_and_unknown_usb(self):
        self.add_usb("1-2", "046d")
        self.add_usb("1-4", "1234")
        self.assertEqual(ops.deps_needed(self.root, which=lambda n: n), [])

    def test_no_matching_hardware_is_empty(self):
        self.add_usb("1-4", "1234")
        self.assertEqual(ops.deps_needed(self.root, which=lambda _: None), [])


ROOT = pathlib.Path(__file__).resolve().parents[1]


class DevicesCliTests(unittest.TestCase):
    def test_devices_command_prints_json(self):
        saved = ops.solaar_devices, ops.headsetcontrol_devices, ops.gpu_status
        ops.solaar_devices = lambda: self.fail("Logitech rows come from logi.py")
        ops.headsetcontrol_devices = lambda: [
            {"name": "Arctis Nova Pro Wireless", "percent": 62, "kind": "headset", "status": "Charging"}
        ]
        ops.gpu_status = lambda: {"present": False}
        try:
            from io import StringIO
            from contextlib import redirect_stdout
            buf = StringIO()
            with redirect_stdout(buf):
                code = ops.main(["ops.py", "devices"])
            self.assertEqual(code, 0)
            data = json.loads(buf.getvalue())
            self.assertEqual(data["devices"][0]["name"], "Arctis Nova Pro Wireless")
            self.assertEqual(data["devices"][0]["status"], "Charging")
        finally:
            ops.solaar_devices, ops.headsetcontrol_devices, ops.gpu_status = saved


class DeviceUiTests(unittest.TestCase):
    def test_service_installs_missing_deps_once(self):
        src = (ROOT / "Service.qml").read_text()
        self.assertIn("deps-needed", src)
        self.assertIn('["omarchy", "pkg", "add"]', src)

    def test_plan_devices_row_has_icons_and_bar(self):
        src = (ROOT / "PlanSection.qml").read_text()
        kind = src.find("Model.kindIcon")
        bar = src.find("modelData.percent / 100")
        pct = src.find('modelData.percent + "%"')
        batt = src.find("text: Model.batteryIcon")
        self.assertNotEqual(kind, -1)
        self.assertLess(kind, bar)
        self.assertLess(bar, pct)
        self.assertLess(pct, batt)
        self.assertNotIn("ListView", src)
        self.assertNotIn("height: contentHeight", src)
        devices = src[src.find('text: "DEVICES"'):src.find("POWER & BATTERY")]
        self.assertIn("PanelActionButton", devices)
        self.assertIn("\\uf021", devices)
        self.assertIn("Re-scan peripheral batteries", devices)
        self.assertNotIn('text: "Refresh"', devices)
        self.assertIn("No battery readings", devices)
        self.assertNotIn("visible: root.devices.length > 0", devices)
        rows = src[src.find("model: root.devices"):src.find("POWER & BATTERY")]
        self.assertNotIn("anchors.right: parent.right", rows)

    def test_charge_glyphs_are_battery_charging_not_bells(self):
        src = (ROOT / "Model.js").read_text()
        block = src[src.find("var CHARGE"):src.find("];", src.find("var CHARGE"))]
        want = [0xF089C, 0xF0086, 0xF0087, 0xF0088, 0xF089D, 0xF0089, 0xF089E, 0xF008A, 0xF008B, 0xF0085]
        self.assertEqual([ord(c) for c in block if ord(c) > 0xFFFF], want)


    def test_refresh_reruns_devices_only(self):
        src = (ROOT / "PlanSection.qml").read_text()
        start = src.find("function refreshDevices(")
        self.assertNotEqual(start, -1)
        nxt = src.find("\n  function ", start + 1)
        body = src[start:nxt if nxt != -1 else start + 400]
        self.assertIn("devProc.running", body)
        self.assertNotIn("loadStatus", body)
        self.assertNotIn("root.devices = []", body)
        self.assertIn("onClicked: root.rescan()", src)
        rescan = src[src.find("function rescan()"):]
        rescan = rescan[:rescan.find("\n  }\n")]
        self.assertIn("root.refreshDevices()", rescan)
        self.assertIn("root.startLogi(true)", rescan)
        self.assertIn("Model.batteryIcon(modelData.percent, modelData.status)", src)

    def test_logitech_watcher_lives_only_while_open(self):
        src = (ROOT / "PlanSection.qml").read_text()
        start = src[src.find("function startLogi("):]
        start = start[:start.find("\n  }\n")]
        self.assertIn("if (!root.opened) return", start)
        self.assertIn('"watch"', start)
        self.assertIn('"--identify"', start)
        opened = src[src.find("onOpenedChanged"):]
        opened = opened[:opened.find("\n  }\n")]
        self.assertIn("root.startLogi(false)", opened)
        self.assertIn("logiProc.running = false", opened)
        self.assertIn("SplitParser", src)
        self.assertIn("logiRows.concat(headsetRows)", src)
        timer = src[src.find("Timer {"):]
        timer = timer[:timer.find("\n  }\n")]
        self.assertIn("running: root.opened", timer)
        self.assertIn("root.refreshDevices()", timer)
        self.assertIn("interval: root.headsetPollMs", timer)
        self.assertIn("headsetPollMs: 1000", src)
        panel = (ROOT / "Panel.qml").read_text()
        plan = panel[panel.find("PlanSection {"):]
        plan = plan[:plan.find("}")]
        self.assertIn("opened: root.opened", plan)
        self.assertIn("pollMs: service.pollInterval", plan)

    def test_opening_panel_reloads_devices(self):
        src = (ROOT / "Panel.qml").read_text()
        opened = src[src.find("onOpenedChanged"):]
        opened = opened[:opened.find("\n  }\n")]
        self.assertIn("planSection.reload()", opened)
        self.assertIn("id: planSection", src)

    def test_readme_documents_usb_packages(self):
        src = (ROOT / "README.md").read_text()
        self.assertIn("solaar", src)
        self.assertIn("headsetcontrol", src)
        self.assertIn("046d", src)
        self.assertIn("1038", src)
        self.assertIn("Re-scan peripheral batteries", src)

    def test_changelog_notes_peripheral_fix(self):
        src = (ROOT / "CHANGELOG.md").read_text()
        self.assertIn("solaar", src)
        self.assertIn("headsetcontrol", src)


if __name__ == "__main__":
    unittest.main()
