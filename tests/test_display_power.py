#!/usr/bin/env python3
import os
import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "bin"))
import display_power as dp  # noqa: E402


def mon(name, dpms=True, disabled=False, **extra):
    row = {
        "name": name,
        "dpmsStatus": dpms,
        "disabled": disabled,
        "width": 2560,
        "height": 1600,
        "refreshRate": 60,
        "x": 0,
        "y": 0,
        "scale": 1.6,
    }
    row.update(extra)
    return row


class DisplayPowerTests(unittest.TestCase):
    def test_dpms_lua_names_the_monitor(self):
        lua = dp.dpms_lua("disable", "DP-3")
        self.assertIn('monitor = "DP-3"', lua)
        self.assertIn('action = "disable"', lua)
        self.assertNotIn("eDP-2", lua)

    def test_rejects_unsafe_monitor_names(self):
        self.assertTrue(dp.safe_name("eDP-1"))
        self.assertTrue(dp.safe_name("DP-12"))
        self.assertTrue(dp.safe_name("HDMI-A-1"))
        self.assertFalse(dp.safe_name('eDP-1"; os.execute("x")'))
        self.assertFalse(dp.safe_name(""))
        self.assertFalse(dp.safe_name(None))

    def test_plan_off_covers_every_enabled_head(self):
        names = [c[1] for c in dp.plan_off([
            mon("eDP-1"),
            mon("DP-2"),
            mon("DP-3"),
            mon("HDMI-A-1", disabled=True),
        ])]
        self.assertEqual(names, ["eDP-1", "DP-2", "DP-3"])
        self.assertNotIn("HDMI-A-1", names)

    def test_still_on_is_dpms_deaf_heads(self):
        leftover = dp.still_on([
            mon("eDP-2", dpms=False),
            mon("DP-2", dpms=True),
            mon("DP-3", dpms=True),
        ])
        self.assertEqual([m["name"] for m in leftover], ["DP-2", "DP-3"])

    def test_fallback_keeps_one_enabled_output(self):
        heads = [mon("eDP-1", dpms=False), mon("DP-2"), mon("DP-3")]
        names = [m["name"] for m in dp.fallback_disable(heads)]
        self.assertEqual(names, ["DP-2", "DP-3"])

        only_deaf = [mon("DP-2"), mon("DP-3")]
        names = [m["name"] for m in dp.fallback_disable(only_deaf)]
        self.assertEqual(len(names), 1)
        self.assertIn(names[0], ("DP-2", "DP-3"))

        lone = dp.fallback_disable([mon("DP-2")])
        self.assertEqual(lone, [])

    def test_laptop_monitor_is_not_hardcoded(self):
        self.assertEqual(dp.laptop_monitor([mon("HDMI-A-1"), mon("eDP-1")]), "eDP-1")
        self.assertEqual(dp.laptop_monitor([mon("eDP-2"), mon("DP-2")]), "eDP-2")
        self.assertEqual(dp.laptop_monitor([mon("LVDS-1")]), "LVDS-1")
        self.assertIsNone(dp.laptop_monitor([mon("DP-1"), mon("HDMI-A-1")]))

    def test_set_displays_off_dispatches_per_monitor_then_disables_deaf(self):
        calls = []
        live = [
            mon("eDP-2"),
            mon("DP-2"),
            mon("DP-3"),
        ]

        def run(args, timeout=4):
            calls.append(args)
            lua = " ".join(args)
            if "dpms" in lua and "DP-2" in lua:
                live[1]["dpmsStatus"] = True
            if "dpms" in lua and "eDP-2" in lua:
                live[0]["dpmsStatus"] = False
            if "dpms" in lua and "DP-3" in lua:
                live[2]["dpmsStatus"] = True
            if "disabled = true" in lua and "DP-2" in lua:
                live[1]["disabled"] = True
            if "disabled = true" in lua and "DP-3" in lua:
                live[2]["disabled"] = True
            return 0, ""

        state = {}
        dp.set_displays(
            "off",
            run=run,
            monitors_fn=lambda: [dict(m) for m in live],
            state_fn=lambda: state,
            save_state_fn=lambda s: state.update(s),
        )
        lua_calls = [" ".join(c) for c in calls]
        self.assertTrue(any('monitor = "eDP-2"' in c for c in lua_calls))
        self.assertTrue(any('monitor = "DP-2"' in c for c in lua_calls))
        self.assertTrue(any('monitor = "DP-3"' in c for c in lua_calls))
        self.assertFalse(any(c[1:3] == ["dispatch", "dpms"] for c in calls))
        names = [d["name"] if isinstance(d, dict) else d for d in state.get("disabled", [])]
        self.assertIn("DP-2", names)
        self.assertIn("DP-3", names)

    def test_set_displays_on_restores_disabled_heads(self):
        calls = []
        live = [mon("eDP-2", dpms=False), mon("DP-2", disabled=True), mon("DP-3", disabled=True)]
        saved = {
            "disabled": [
                {"name": "DP-2", "width": 2560, "height": 1600, "refreshRate": 60,
                 "x": 1600, "y": 0, "scale": 1.6},
                {"name": "DP-3", "width": 2560, "height": 1600, "refreshRate": 60,
                 "x": 1600, "y": 1000, "scale": 1.6},
            ]
        }

        def run(args, timeout=4):
            calls.append(args)
            return 0, ""

        dp.set_displays(
            "on",
            run=run,
            monitors_fn=lambda: live,
            state_fn=lambda: saved,
            save_state_fn=lambda s: saved.clear() or saved.update(s),
        )
        joined = "\n".join(" ".join(c) for c in calls)
        self.assertIn("DP-2", joined)
        self.assertIn("DP-3", joined)
        self.assertIn("disabled = false", joined)
        self.assertEqual(saved.get("disabled"), [])

    def test_set_displays_off_dismisses_screensaver_before_dpms(self):
        calls = []
        live = [mon("eDP-2")]

        def run(args, timeout=4):
            calls.append(args)
            return 0, ""

        dp.set_displays(
            "off",
            run=run,
            monitors_fn=lambda: live,
            state_fn=lambda: {"disabled": []},
            save_state_fn=lambda s: None,
        )
        self.assertIn(["pkill", "-x", "ttfx"], calls)
        self.assertIn(["pkill", "-f", "[o]rg.omarchy.screensaver"], calls)
        pkill_at = next(i for i, c in enumerate(calls) if c[0] == "pkill")
        dpms_at = next(i for i, c in enumerate(calls) if "dpms" in " ".join(c))
        self.assertLess(pkill_at, dpms_at)

    def test_set_displays_on_dismisses_screensaver_when_unlocked(self):
        calls = []

        def run(args, timeout=4):
            calls.append(args)
            if args[:4] == ["omarchy-shell", "-q", "lock", "isLocked"]:
                return 0, "false\n"
            return 0, ""

        dp.set_displays(
            "on",
            run=run,
            monitors_fn=lambda: [mon("eDP-1", dpms=False)],
            state_fn=lambda: {"disabled": []},
            save_state_fn=lambda s: None,
        )
        self.assertIn(["pkill", "-x", "ttfx"], calls)
        self.assertIn(["pkill", "-f", "[o]rg.omarchy.screensaver"], calls)
        pkill_at = next(i for i, c in enumerate(calls) if c[0] == "pkill")
        dpms_at = next(i for i, c in enumerate(calls) if "dpms" in " ".join(c))
        self.assertLess(pkill_at, dpms_at)

    def test_set_displays_on_dismisses_screensaver_even_when_locked(self):
        calls = []

        def run(args, timeout=4):
            calls.append(args)
            if args[:4] == ["omarchy-shell", "-q", "lock", "isLocked"]:
                return 0, "true\n"
            return 0, ""

        dp.set_displays(
            "on",
            run=run,
            monitors_fn=lambda: [mon("eDP-1", dpms=False)],
            state_fn=lambda: {"disabled": []},
            save_state_fn=lambda s: None,
        )
        self.assertIn(["pkill", "-x", "ttfx"], calls)
        self.assertFalse(any(c[:3] == ["omarchy-shell", "-q", "lock"] for c in calls))


class ParkTests(unittest.TestCase):
    def harness(self, live, state=None):
        calls = []
        store = {"disabled": []} if state is None else state

        def run(args, timeout=4):
            calls.append(" ".join(args))
            return 0, ""

        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        kw = dict(
            run=run,
            monitors_fn=lambda: live,
            state_fn=lambda: store,
            save_state_fn=lambda s: store.clear() or store.update(s),
            droppers_path=pathlib.Path(tmp.name) / "droppers.json",
        )
        return calls, store, kw

    def test_parked_head_is_disabled_directly_on_next_blank(self):
        flip = "RTK FlipGo-A2 8R33926O00QS"
        live = [mon("eDP-2", dpms=False), mon("DP-2", description=flip)]
        calls, store, kw = self.harness(live)
        self.assertTrue(dp.park("DP-2", **kw))
        self.assertEqual(dp.load_droppers(kw["droppers_path"]), [flip])

        live[:] = [mon("eDP-2"), mon("DP-2", description=flip)]
        calls.clear()
        store["disabled"] = []
        dp.set_displays("off", **kw)
        self.assertTrue(any(f'output = "desc:{flip}", disabled = true' in c for c in calls))
        self.assertEqual([s["name"] for s in store["disabled"]], ["DP-2"])

    def test_direct_disable_keeps_one_output(self):
        heads = [mon("DP-2", description="a"), mon("DP-3", description="b")]
        self.assertEqual([m["name"] for m in dp.plan_direct(heads, ["a", "b"])], ["DP-3"])
        self.assertEqual(dp.plan_direct([mon("eDP-1"), mon("DP-1")], ["a"]), [])

    def test_parks_head_that_returns_while_others_are_blanked(self):
        live = [
            mon("eDP-2", dpms=False),
            mon("DP-2", description="RTK FlipGo-A2 8R33926O00QS"),
            mon("DP-3", description="RTK FlipGo-A1 8R33926O00QS"),
        ]
        calls, store, kw = self.harness(live)
        self.assertTrue(dp.park("DP-2", **kw))
        self.assertIn('output = "desc:RTK FlipGo-A2 8R33926O00QS", disabled = true', calls[-1])
        self.assertEqual([s["name"] for s in store["disabled"]], ["DP-2"])
        self.assertTrue(dp.park("DP-3", **kw))
        self.assertEqual([s["name"] for s in store["disabled"]], ["DP-2", "DP-3"])

    def test_does_not_park_when_session_is_awake(self):
        calls, store, kw = self.harness([mon("eDP-1"), mon("HDMI-A-1")])
        self.assertFalse(dp.park("HDMI-A-1", **kw))
        self.assertEqual(calls, [])

    def test_never_parks_the_last_enabled_output(self):
        calls, store, kw = self.harness([mon("eDP-1", disabled=True), mon("DP-1")])
        self.assertFalse(dp.park("DP-1", **kw))
        self.assertEqual(calls, [])

    def test_rejects_unsafe_description_and_falls_back_to_name(self):
        live = [mon("eDP-1", dpms=False), mon("DP-1", description='bad" desc')]
        calls, store, kw = self.harness(live)
        self.assertTrue(dp.park("DP-1", **kw))
        self.assertIn('output = "DP-1", disabled = true', calls[-1])

    def test_sync_waits_until_blanked_heads_wake(self):
        live = [mon("eDP-2", dpms=False), mon("DP-2", disabled=True)]
        parked = {"disabled": [dict(dp.snapshot(mon("DP-2", description="RTK FlipGo-A2")), x=1600)]}
        calls, store, kw = self.harness(live, parked)
        kw.pop("droppers_path")
        self.assertEqual(dp.sync(**kw), "waiting")
        self.assertEqual(calls, [])

        live[0]["dpmsStatus"] = True
        self.assertEqual(dp.sync(**kw), "restored")
        self.assertIn('output = "desc:RTK FlipGo-A2"', calls[-1])
        self.assertIn('position = "1600x0"', calls[-1])
        self.assertIn("disabled = false", calls[-1])
        self.assertEqual(store["disabled"], [])
        self.assertEqual(dp.sync(**kw), "idle")

    def test_displays_off_keeps_already_parked_heads(self):
        live = [mon("eDP-2"), mon("DP-3")]
        parked = {"disabled": [dp.snapshot(mon("DP-2"))]}
        calls, store, kw = self.harness(live, parked)
        dp.set_displays("off", **kw)
        self.assertIn("DP-2", [s["name"] for s in store["disabled"]])

    def test_lid_blank_skips_disable_and_only_sends_dpms(self):
        flip = "RTK FlipGo-A2 8R33926O00QS"
        live = [mon("eDP-2"), mon("DP-2", description=flip)]
        calls, store, kw = self.harness(live)
        dp.set_displays("off", disable=False, **kw)
        self.assertTrue(any("dpms" in c for c in calls))
        self.assertFalse(any("disabled = true" in c for c in calls))
        self.assertEqual(store.get("disabled", []), [])


if __name__ == "__main__":
    unittest.main()
