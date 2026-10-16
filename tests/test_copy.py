#!/usr/bin/env python3
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class CopyGlossaryTests(unittest.TestCase):
    def test_plan_section_uses_glossary(self):
        src = (ROOT / "PlanSection.qml").read_text()
        self.assertIn('side: "ac", title: "PLUGGED IN"', src)
        self.assertIn('side: "battery", title: "BATTERY"', src)
        model = (ROOT / "Model.js").read_text()
        self.assertIn('{ value: "power-saver", label: "󰌪 Power saver"', model)
        self.assertIn('{ key: "displaysOff", label: "Displays off"', src)
        self.assertIn('{ value: "display-off", label: "Displays off" }', model)
        self.assertIn('{ value: "nothing", label: "Do nothing" }', model)
        self.assertNotIn('{ side: "ac", title: "AC" }', src)
        self.assertNotIn("On battery", src)
        self.assertNotIn("Blank screens", src)
        self.assertNotIn('label: "Power-saver"', src)
        self.assertNotIn('{ value: "nothing", label: "Nothing" }', src)

    def test_hero_and_tooltip_use_glossary(self):
        model = (ROOT / "Model.js").read_text()
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn('return "PLUGGED IN · CHARGING"', model)
        self.assertIn('return "PLUGGED IN · DISCHARGING"', model)
        self.assertIn('return "PLUGGED IN · NOT CHARGING"', model)
        self.assertIn('return "PLUGGED IN";', model)
        self.assertIn('return "BATTERY · DISCHARGING"', model)
        self.assertNotIn("ON AC", model)
        self.assertNotIn("ON BATTERY", model)
        self.assertIn(': "Plugged in"', panel)
        self.assertNotIn("Running on AC", panel)

    def test_charge_section_names_backends(self):
        src = (ROOT / "PlanSection.qml").read_text()
        self.assertIn("CHARGE LIMIT", src)
        self.assertIn("BATTERY HEALTH", src)
        self.assertIn("Desk use: 80%. Turn Battery Extender off in BIOS.", src)
        self.assertIn('root.call(["health"', src)


class PowerPanelCleanupTests(unittest.TestCase):
    def test_model_helpers_exist(self):
        model = (ROOT / "Model.js").read_text()
        self.assertIn("function drawValue(b)", model)
        self.assertIn("function historyBars(points, now)", model)
        self.assertNotIn("function drawLabel(b)", model)
        self.assertNotIn("function ecLabel(raw)", model)

    def test_panel_battery_usage_not_sparkline(self):
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn('text: "BATTERY USAGE"', panel)
        self.assertIn('label: "Power draw"', panel)
        self.assertIn("property bool historyOpen: false", panel)
        self.assertIn("Model.drawValue", panel)
        self.assertIn("battery-history", panel)
        self.assertIn("24h ago", panel)
        self.assertNotIn("function drawLabel", panel)
        self.assertNotIn("ecLabel", panel)
        self.assertNotIn('label: "Temp"', panel)

    def test_service_drops_since_open_history(self):
        svc = (ROOT / "PowerService.qml").read_text()
        self.assertNotIn("property var history:", svc)
        self.assertNotIn("historyMax", svc)


if __name__ == "__main__":
    unittest.main()
