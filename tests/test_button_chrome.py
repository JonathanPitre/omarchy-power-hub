#!/usr/bin/env python3
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


def button_bodies(text: str) -> list[str]:
    bodies = []
    for m in re.finditer(r"\bAccentButton\s*\{", text):
        i = m.end() - 1
        depth = 0
        while i < len(text):
            if text[i] == "{":
                depth += 1
            elif text[i] == "}":
                depth -= 1
                if depth == 0:
                    bodies.append(text[m.end() : i])
                    break
            i += 1
    return bodies


class ButtonChromeTests(unittest.TestCase):
    def test_plan_and_panel_buttons_use_accent_button(self):
        for name in ("PlanSection.qml", "Panel.qml"):
            src = (ROOT / name).read_text()
            self.assertTrue(button_bodies(src), name)
            self.assertNotRegex(src, r"(?m)^\s*Button\s*\{", name)
        src = (ROOT / "AccentButton.qml").read_text()
        self.assertIn("bordered: true", src)
        self.assertIn("hot || selected || active", src)

    # Bare `Color` does not resolve to the theme singleton inside plugins.
    def test_theme_colors_use_qualified_singleton(self):
        for name in ("PlanSection.qml", "Panel.qml"):
            src = (ROOT / name).read_text()
            self.assertNotRegex(src, r"(^|[^.\w])Color\.", name)

    def test_idle_grid_shows_plugged_in_then_battery(self):
        src = (ROOT / "PlanSection.qml").read_text()
        self.assertLess(src.index('side: "ac", title: "PLUGGED IN"'),
                        src.index('side: "battery", title: "BATTERY"'))
        self.assertIn('icon: "󰚥"', src)
        self.assertIn('icon: "󰁹"', src)
        self.assertIn("delegate: Dropdown {", src)
        self.assertIn('text: "POWER & BATTERY"', src)
        self.assertIn('{ key: "profile", label: "Power mode" }', src)
        self.assertNotIn("NOW", src)
        self.assertNotIn("sidePicked", src)
        self.assertNotIn("system-default", src)

    # Dropdown sets its own value before emitting changed, so comparing
    # against it would skip every save.
    def test_grid_dropdown_saves_against_config_not_own_value(self):
        src = (ROOT / "PlanSection.qml").read_text()
        self.assertNotIn("v !== value", src)
        self.assertIn('if (v !== String(saved)) root.call(["set"', src)

    def test_bar_icon_is_not_a_chip(self):
        src = (ROOT / "Panel.qml").read_text()
        m = re.search(r"BarIconButton\s*\{([^}]*)\}", src)
        self.assertIsNotNone(m)
        self.assertNotIn("bordered: true", m.group(1))


if __name__ == "__main__":
    unittest.main()
