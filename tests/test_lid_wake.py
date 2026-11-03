#!/usr/bin/env python3
import os
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import ops  # noqa: E402
import idle_wake as iw  # noqa: E402

# Keep Omarchy's remembered power profiles (~/.local/state/omarchy/powerprofiles)
# out of reach: ops writes them through XDG_STATE_HOME.
_state_home = None


def setUpModule():
    global _state_home
    _state_home = mock.patch.dict(os.environ, {"XDG_STATE_HOME": tempfile.mkdtemp()})
    _state_home.start()


def tearDownModule():
    _state_home.stop()


class LidWakeBindTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self._tmp.name)
        self._bindings = ops.BINDINGS
        self._run = ops.run
        self._priv = ops.priv
        ops.BINDINGS = self.root / "bindings.lua"
        ops.BINDINGS.write_text("-- hypr\n")
        ops.run = lambda args, timeout=4: (0, "")
        self.priv_calls = []
        ops.priv = lambda args: self.priv_calls.append(list(args)) or ""

    def tearDown(self):
        ops.BINDINGS = self._bindings
        ops.run = self._run
        ops.priv = self._priv
        self._tmp.cleanup()

    def test_lid_block_binds_open_and_closed(self):
        ops.write_lid_block("suspend")
        text = ops.BINDINGS.read_text()
        self.assertIn("switch:on:Lid Switch", text)
        self.assertIn("switch:off:Lid Switch", text)
        self.assertIn("lid-closed", text)
        self.assertIn("lid-opened", text)

    def test_lid_closed_disables_pointer_wakeup_before_suspend(self):
        runs = []
        ops.run = lambda args, timeout=4: runs.append(list(args)) or (0, "")
        with mock.patch.object(ops, "load", return_value={
            "ac": {"lid": "suspend"},
            "battery": {"lid": "suspend"},
        }), mock.patch.object(ops, "active_side", return_value="ac"):
            ops.lid_closed()
        self.assertEqual(self.priv_calls[0], ["hid-wakeup", "off"])
        self.assertTrue(any(c[:2] == ["systemctl", "suspend"] for c in runs))

    def test_lid_closed_display_off_blanks_without_disabling(self):
        calls = []
        with mock.patch.object(ops, "load", return_value={
            "ac": {"lid": "display-off"},
            "battery": {"lid": "display-off"},
        }), mock.patch.object(ops, "active_side", return_value="ac"), \
             mock.patch.object(ops.display_power, "state_lock", mock.MagicMock()), \
             mock.patch.object(ops, "set_displays", side_effect=lambda *a, **k: calls.append((a, k)) or ""), \
             mock.patch.object(ops.kblight, "sync"):
            ops.lid_closed()
        self.assertEqual(calls[0][0], ("off",))
        self.assertEqual(calls[0][1].get("disable"), False)

    def test_park_skips_when_lid_closed(self):
        with mock.patch.object(ops.idle_wake, "acpi_lid_text", return_value="state: closed"), \
             mock.patch.object(ops.display_power, "state_lock", mock.MagicMock()), \
             mock.patch.object(ops.display_power, "park") as park:
            self.assertEqual(ops.main(["ops.py", "displays", "park", "DP-2"]), 0)
            park.assert_not_called()

    def _lid_opened(self, state, monitors):
        calls = []
        with mock.patch.object(ops.display_power, "state_lock", mock.MagicMock()), \
             mock.patch.object(ops.display_power, "load_state", return_value=state), \
             mock.patch.object(ops.display_power, "default_monitors", return_value=monitors), \
             mock.patch.object(ops, "set_displays", side_effect=lambda m: calls.append(m) or ""), \
             mock.patch.object(ops.kblight, "sync") as kb:
            ops.lid_opened()
        return calls, kb

    def test_lid_opened_enables_hid_wakeup(self):
        self._lid_opened({"disabled": []}, [])
        self.assertEqual(self.priv_calls, [["hid-wakeup", "on"]])

    def test_lid_opened_restores_disabled_heads(self):
        calls, kb = self._lid_opened(
            {"disabled": [{"name": "DP-3"}]},
            [{"name": "eDP-1", "disabled": False, "dpmsStatus": True}],
        )
        self.assertEqual(calls, ["on"])
        self.assertEqual(kb.call_args.args[0], "on")

    def test_lid_opened_wakes_dpms_blanked_heads(self):
        calls, _ = self._lid_opened(
            {"disabled": []},
            [{"name": "eDP-1", "disabled": False, "dpmsStatus": False}],
        )
        self.assertEqual(calls, ["on"])

    def test_lid_opened_leaves_lit_displays_alone(self):
        calls, kb = self._lid_opened(
            {"disabled": []},
            [{"name": "eDP-1", "disabled": False, "dpmsStatus": True}],
        )
        self.assertEqual(calls, [])
        kb.assert_not_called()


class IdleWakePollTests(unittest.TestCase):
    def test_blanked_dpms_on_is_wake_when_lid_open(self):
        mons = [{"name": "eDP-1", "disabled": False, "dpmsStatus": True}]
        self.assertEqual(
            iw.poll(lid_text="state: open", cursor="1,1", prev_cursor="1,1",
                    monitors=mons, blanked=True),
            "wake",
        )

    def test_screensaver_needs_cursor_not_dpms(self):
        mons = [{"name": "eDP-1", "disabled": False, "dpmsStatus": True}]
        self.assertEqual(
            iw.poll(lid_text="state: open", cursor="1,1", prev_cursor="1,1",
                    monitors=mons, blanked=False),
            "hold",
        )
        self.assertEqual(
            iw.poll(lid_text="state: open", cursor="2,1", prev_cursor="1,1",
                    monitors=mons, blanked=False),
            "wake",
        )

    def test_lid_closed_poll_is_closed_even_if_dpms_on(self):
        mons = [{"name": "eDP-1", "disabled": False, "dpmsStatus": True}]
        self.assertEqual(
            iw.poll(lid_text="state: closed", cursor="2,1", prev_cursor="1,1",
                    monitors=mons, blanked=True),
            "closed",
        )


if __name__ == "__main__":
    unittest.main()
