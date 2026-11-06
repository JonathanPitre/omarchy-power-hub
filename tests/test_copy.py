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
        self.assertIn('{ state: "PLUGGED IN · CHARGING", time: "" }', model)
        self.assertIn('{ state: "PLUGGED IN · NOT CHARGING", time: "" }', model)
        self.assertIn('{ state: "PLUGGED IN", time: "" }', model)
        self.assertIn('{ state: "DISCHARGING", time: t ? t + " REMAINING" : "" }', model)
        self.assertNotIn("ON AC", model)
        self.assertNotIn("ON BATTERY", model)
        self.assertIn(': "Plugged in"', panel)
        self.assertNotIn("Running on AC", panel)

    def test_charge_section_names_backends(self):
        src = (ROOT / "PlanSection.qml").read_text()
        self.assertIn("CHARGE LIMIT", src)
        self.assertIn("BATTERY HEALTH", src)
        self.assertNotIn("Desk use", src)
        self.assertIn("Set the BIOS charge limit to match this value", src)
        self.assertIn('"Needs permission"', src)
        self.assertIn('root.call(["health"', src)


class PowerPanelCleanupTests(unittest.TestCase):
    def test_model_helpers_exist(self):
        model = (ROOT / "Model.js").read_text()
        self.assertIn("function drawValue(b)", model)
        self.assertIn("function historyBars(points, now)", model)
        self.assertNotIn("function drawLabel(b)", model)
        self.assertNotIn("function ecLabel(raw)", model)

    def test_panel_time_and_watts_prefer_sysfs_poll(self):
        # UPower has no rate for ~90 s after unplug and refreshes every ~30 s.
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn("Number(service.battery.watts) || Math.abs(Number(dev.changeRate)", panel)
        self.assertIn("service.battery.status === root.batStatus && Number(service.battery.minutes)", panel)

    def test_travel_labels_and_auto_off_on_plug_in(self):
        plan = (ROOT / "PlanSection.qml").read_text()
        self.assertIn('label: "Max brightness"', plan)
        self.assertIn('label: "Max refresh rate"', plan)
        service = (ROOT / "Service.qml").read_text()
        self.assertIn("root.lastBatteryState === UPowerDeviceState.Discharging", service)
        self.assertIn('command: ["python3", root.ops, "travel", "off"]', service)

    def test_no_process_polling_at_idle(self):
        # A python3 and a login bash every 2 s kept the CPU awake with the panel closed.
        service = (ROOT / "Service.qml").read_text()
        self.assertNotIn("lid-state", service)
        self.assertNotIn("pgrep", service)
        self.assertIn('appId === "org.omarchy.screensaver"', service)

    def test_power_profile_follows_battery_state_not_only_on_battery(self):
        service = (ROOT / "Service.qml").read_text()
        i = service.index("function onStateChanged()")
        self.assertIn("applyProfileDebounce.restart()", service[i:i + 500])

    def test_pinned_tweaks_read_the_saved_prefs_not_tlp(self):
        data = (ROOT / "bin/battery-plus-data").read_text()
        self.assertIn("power-hub-tweaks.json", data)
        self.assertNotIn("/etc/tlp.d/02-battery-plus.conf", data)

    def test_new_power_tweaks_are_listed_and_routed(self):
        model = (ROOT / "Model.js").read_text()
        self.assertIn('key: "abm"', model)
        self.assertIn('key: "effects"', model)
        self.assertIn("wifi|usb|pcie|audio|abm|effects)", (ROOT / "bin/battery-plus-action").read_text())
        self.assertIn("  abm)\n", (ROOT / "bin/ops-priv").read_text())

    def test_panel_holds_estimate_after_plug_or_unplug(self):
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn("estimating: root.estimating", panel)
        self.assertIn("interval: 30000; onTriggered: root.estimating = false", panel)
        self.assertIn('if (root.lastStatus !== "Unknown")', panel)

    def test_panel_battery_usage_not_sparkline(self):
        panel = (ROOT / "Panel.qml").read_text()
        self.assertIn('text: "BATTERY USAGE"', panel)
        self.assertIn('label: root.charging ? "Charging at" : "Power draw"', panel)
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
