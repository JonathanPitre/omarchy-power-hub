#!/usr/bin/env python3
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import idle_wake as iw  # noqa: E402


class IdleWakeTests(unittest.TestCase):
    def test_lid_open_pointer_motion_restores(self):
        self.assertTrue(iw.restore_from_pointer(
            lid_open=True, cursor_changed=True, dpms_on=False))

    def test_lid_open_hypr_dpms_on_restores(self):
        self.assertTrue(iw.restore_from_pointer(
            lid_open=True, cursor_changed=False, dpms_on=True))

    def test_lid_closed_pointer_does_not_restore(self):
        self.assertFalse(iw.restore_from_pointer(
            lid_open=False, cursor_changed=True, dpms_on=True))

    def test_lid_open_without_activity_does_not_restore(self):
        self.assertFalse(iw.restore_from_pointer(
            lid_open=True, cursor_changed=False, dpms_on=False))

    def test_acpi_lid_state(self):
        self.assertTrue(iw.lid_is_open("state:      open\n"))
        self.assertFalse(iw.lid_is_open("state:      closed\n"))

    def test_cursor_changed(self):
        self.assertTrue(iw.cursor_changed("10,20", "11,20"))
        self.assertFalse(iw.cursor_changed("10,20", "10,20"))
        self.assertFalse(iw.cursor_changed(None, "10,20"))

    def test_mice_stay_enabled_unless_lid_closed_and_blanked(self):
        self.assertTrue(iw.pointers_enabled(lid_open=True, blanked=True))
        self.assertTrue(iw.pointers_enabled(lid_open=False, blanked=False))
        self.assertFalse(iw.pointers_enabled(lid_open=False, blanked=True))

    def test_keyboard_names_skip_power_and_gpio(self):
        names = iw.keyboard_names({
            "keyboards": [
                {"name": "framework-laptop-16-keyboard-module---ansi-keyboard"},
                {"name": "logitech-usb-receiver"},
                {"name": "power-button"},
                {"name": "gpio-keys"},
                {"name": "video-bus"},
            ]
        })
        self.assertEqual(names, [
            "framework-laptop-16-keyboard-module---ansi-keyboard",
            "logitech-usb-receiver",
        ])

    def test_mouse_names_skip_unsafe(self):
        names = iw.mouse_names({
            "mice": [
                {"name": "logitech-usb-receiver-mouse"},
                {"name": 'bad"; os.execute("x")'},
            ]
        })
        self.assertEqual(names, ["logitech-usb-receiver-mouse"])

    def test_set_mice_eval_keeps_colon_names(self):
        calls = []
        iw.set_mice_enabled(
            False,
            {"mice": [{"name": "pixa3854:00-093a:0274-mouse"}]},
            lambda args, timeout=4: calls.append(args) or (0, ""),
        )
        self.assertEqual(calls[0][:2], ["hyprctl", "eval"])
        self.assertIn("pixa3854:00-093a:0274-mouse", calls[0][2])
        self.assertIn("enabled = false", calls[0][2])


if __name__ == "__main__":
    unittest.main()
