#!/usr/bin/env python3
import os
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import kblight  # noqa: E402


def only_qmk(name):
    return "/usr/bin/qmk_hid" if name == "qmk_hid" else None


def both_tools(name):
    return "/usr/bin/" + name if name in {"qmk_hid", "framework_tool"} else None


def fake_led(root, brightness="80", trigger="[chromeos-auto] none"):
    led = pathlib.Path(root) / "chromeos:white:power"
    led.mkdir(parents=True)
    (led / "brightness").write_text(brightness + "\n")
    (led / "max_brightness").write_text("100\n")
    (led / "trigger").write_text(trigger + "\n")
    return led


class KblightTests(unittest.TestCase):
    def test_off_reads_then_blanks_and_stores(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            if args == ["qmk_hid", "via", "--backlight"]:
                return 0, "Brightness: 40%\n"
            if args == ["qmk_hid", "via", "--backlight", "0"]:
                return 0, ""
            return 1, "unexpected"

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight.sync("off", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertEqual(kblight._load(path), 40)
        self.assertEqual(calls, [
            ["qmk_hid", "via", "--backlight"],
            ["qmk_hid", "via", "--backlight", "0"],
        ])
        self.assertFalse(any("--save" in c for c in calls))

    def test_on_restores_saved_percent(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            return 0, ""

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight._save(path, {"level": 40})
            kblight.sync("on", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertFalse(os.path.exists(path))
        self.assertEqual(calls, [["qmk_hid", "via", "--backlight", "40"]])

    def test_failed_read_does_not_invent_a_restore_value(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            return 1, "Permission denied"

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight.sync("off", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertFalse(os.path.exists(path))
            calls.clear()
            kblight.sync("on", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
        self.assertEqual(calls, [])

    def test_missing_binary_is_a_noop(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            return 0, ""

        with tempfile.TemporaryDirectory() as tmp:
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight.sync("off", run=run, which=lambda _n: None, priv_bin="", model_fn=lambda: "16", leds=leds)
            kblight.sync("on", run=run, which=lambda _n: None, priv_bin="", model_fn=lambda: "16", leds=leds)
        self.assertEqual(calls, [])

    def test_sudo_fallback_when_user_call_fails(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            if args[0] == "sudo":
                if args[-1] == "--backlight":
                    return 0, "Brightness: 15%\n"
                return 0, ""
            return 1, "Permission denied"

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight.sync("off", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertEqual(kblight._load(path), 15)
        self.assertEqual(calls[0], ["qmk_hid", "via", "--backlight"])
        self.assertEqual(calls[1][:2], ["sudo", "-n"])
        self.assertNotIn("--save", calls[1])

    def test_power_button_sysfs_off_and_restore(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            if args == ["qmk_hid", "via", "--backlight"]:
                return 0, "Brightness: 40%\n"
            return 0, ""

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            led = fake_led(leds)
            kblight.sync("off", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertEqual(led.joinpath("brightness").read_text().strip(), "0")
            self.assertEqual(kblight._load_state(path)["fp"], {
                "sys": True, "trigger": "chromeos-auto", "brightness": 80,
            })
            kblight.sync("on", run=run, which=only_qmk, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertEqual(led.joinpath("brightness").read_text().strip(), "80")
            self.assertEqual(led.joinpath("trigger").read_text().strip(), "chromeos-auto")
        self.assertFalse(any("framework_tool" in c for c in calls))

    def test_power_button_uses_tee_when_sysfs_is_root_only(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append((list(args), input))
            return 0, ""

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            led = fake_led(leds)
            led.joinpath("brightness").chmod(0o444)
            kblight.sync("off", run=run, which=lambda _n: None, path=path, priv_bin="", model_fn=lambda: "16", leds=leds)
            self.assertTrue(any(c[0][:2] == ["sudo", "-n"] and c[0][2] == "tee" for c in calls))
            self.assertTrue(any(c[1] == "0" for c in calls))

    def test_failed_power_button_read_does_not_invent_a_restore(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            return 1, "Permission denied"

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight.sync("off", run=run, which=lambda name: "/usr/bin/framework_tool" if name == "framework_tool" else None, path=path, priv_bin="/usr/local/bin/ops-priv", model_fn=lambda: "16", leds=leds)
            self.assertFalse(os.path.exists(path))
            calls.clear()
            kblight.sync("on", run=run, which=lambda name: "/usr/bin/framework_tool" if name == "framework_tool" else None, path=path, priv_bin="/usr/local/bin/ops-priv", model_fn=lambda: "16", leds=leds)
        self.assertEqual(calls, [])

    def test_model_from_dmi(self):
        def read(rows):
            return lambda name: rows.get(name, "")

        self.assertEqual(kblight.model(read({"sys_vendor": "Framework", "product_name": "Laptop 16 (AMD Ryzen AI 300 Series)"})), "16")
        self.assertEqual(kblight.model(read({"sys_vendor": "Framework", "product_name": "Laptop 13 (Intel Core Ultra Series 1)"})), "ec")
        self.assertEqual(kblight.model(read({"sys_vendor": "Framework", "product_name": "Laptop 12"})), "ec")
        self.assertEqual(kblight.model(read({"sys_vendor": "Framework", "product_name": "Desktop"})), "")
        self.assertEqual(kblight.model(read({"sys_vendor": "LENOVO", "product_name": "Laptop 16"})), "")

    def test_non_framework_does_not_touch_lights(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            return 0, "Brightness: 40%\n"

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            fake_led(leds)
            kblight._save(path, {"level": 40, "fp": {"requested": "auto", "brightness": 55}})
            kblight.sync("off", run=run, which=both_tools, path=path, priv_bin="", model_fn=lambda: "", leds=leds)
            kblight.sync("on", run=run, which=both_tools, path=path, priv_bin="", model_fn=lambda: "", leds=leds)
            self.assertEqual(kblight._load(path), 40)
            self.assertEqual(pathlib.Path(leds, "chromeos:white:power", "brightness").read_text().strip(), "80")
        self.assertEqual(calls, [])

    def test_framework_13_uses_ec_backlight(self):
        calls = []

        def run(args, timeout=4, input=None):
            calls.append(list(args))
            if args == ["framework_tool", "--kblight"]:
                return 0, "Keyboard backlight: 30%\n"
            if args == ["framework_tool", "--fp-brightness"]:
                return 0, "Requested: Auto\nBrightness: 10%\n"
            return 0, ""

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "kblight.json")
            leds = os.path.join(tmp, "leds")
            os.makedirs(leds)
            kblight.sync("off", run=run, which=both_tools, path=path, priv_bin="", model_fn=lambda: "ec", leds=leds)
            self.assertEqual(kblight._load(path), 30)
            calls.clear()
            kblight.sync("on", run=run, which=both_tools, path=path, priv_bin="", model_fn=lambda: "ec", leds=leds)
        self.assertEqual(calls, [
            ["framework_tool", "--kblight", "30"],
            ["framework_tool", "--fp-led-level", "auto"],
        ])
        self.assertFalse(any("qmk_hid" in c for c in calls))


if __name__ == "__main__":
    unittest.main()
