"""Monitor-agnostic display power for Hyprland 0.56+ Lua dispatchers."""
import contextlib
import fcntl
import json
import os
import pathlib
import re
import subprocess

INTERNAL = re.compile(r"^(eDP|LVDS|DSI)-")
SAFE = re.compile(r"^[A-Za-z0-9._-]+$")
DESC_SAFE = re.compile(r"^[A-Za-z0-9 ._()+-]+$")
STATE_PATH = (
    pathlib.Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    / "omarchy-power-hub"
    / "display-off.json"
)
DROPPERS_PATH = (
    pathlib.Path(os.environ.get("XDG_STATE_HOME") or pathlib.Path.home() / ".local/state")
    / "omarchy-power-hub"
    / "bus-droppers.json"
)



def safe_name(name):
    return bool(name) and bool(SAFE.match(str(name)))


def safe_output(output):
    output = str(output or "")
    if output.startswith("desc:"):
        return bool(DESC_SAFE.match(output[5:]))
    return safe_name(output)


# Config rules usually match external heads by description, so a runtime rule
# has to use the same selector to win across hotplugs.
def selector(m):
    desc = str(m.get("description") or "")
    if desc and DESC_SAFE.match(desc):
        return f"desc:{desc}"
    return m.get("name")


def name_of(spec):
    return spec.get("name") if isinstance(spec, dict) else spec


def blanked(monitors):
    return any(not m.get("disabled") and not m.get("dpmsStatus") for m in monitors or [])


def awake(monitors):
    enabled = [m for m in monitors or [] if not m.get("disabled")]
    return bool(enabled) and all(m.get("dpmsStatus") for m in enabled)


def dpms_lua(action, monitor):
    return f'hl.dsp.dpms({{ action = "{action}", monitor = "{monitor}" }})'


def dpms_argv(action, monitor):
    return ["hyprctl", "dispatch", dpms_lua(action, monitor)]


def eval_argv(lua):
    return ["hyprctl", "eval", lua]


def laptop_monitor(monitors):
    for m in monitors or []:
        name = m.get("name") if isinstance(m, dict) else None
        if name and INTERNAL.match(name) and safe_name(name):
            return name
    return None


def plan_off(monitors):
    cmds = []
    for m in monitors or []:
        name = m.get("name")
        if not safe_name(name) or m.get("disabled"):
            continue
        cmds.append(("dpms", name, "disable"))
    return cmds


def still_on(monitors):
    out = []
    for m in monitors or []:
        if m.get("disabled"):
            continue
        if not m.get("dpmsStatus"):
            continue
        if not safe_name(m.get("name")):
            continue
        out.append(m)
    return out


def fallback_disable(monitors):
    enabled = [
        m for m in (monitors or [])
        if not m.get("disabled") and safe_name(m.get("name"))
    ]
    if len(enabled) <= 1:
        return []
    keep = laptop_monitor(enabled) or enabled[0]["name"]
    deaf = [m for m in enabled if m.get("dpmsStatus")]
    chosen = []
    remaining = len(enabled)
    for m in deaf:
        if m["name"] == keep:
            continue
        if remaining - 1 < 1:
            continue
        chosen.append(m)
        remaining -= 1
    return chosen


def snapshot(m):
    return {
        "name": m.get("name"),
        "output": selector(m),
        "width": m.get("width") or 0,
        "height": m.get("height") or 0,
        "refreshRate": m.get("refreshRate") or 60,
        "x": m.get("x") or 0,
        "y": m.get("y") or 0,
        "scale": m.get("scale") or 1,
    }


def disable_lua(output):
    return f'hl.monitor({{ output = "{output}", disabled = true }})'


def restore_lua(spec):
    name = spec.get("output") or spec["name"]
    w, h = spec.get("width") or 0, spec.get("height") or 0
    r = spec.get("refreshRate") or 60
    x, y = spec.get("x") or 0, spec.get("y") or 0
    scale = spec.get("scale") or 1
    if w and h:
        return (
            f'hl.monitor({{ output = "{name}", mode = "{w}x{h}@{r}", '
            f'position = "{x}x{y}", scale = {scale}, disabled = false }})'
        )
    return f'hl.monitor({{ output = "{name}", disabled = false }})'


def default_run(args, timeout=4):
    try:
        p = subprocess.run(args, text=True, capture_output=True, timeout=timeout)
        return p.returncode, (p.stdout or "") + (p.stderr or "")
    except (OSError, subprocess.TimeoutExpired) as e:
        return 1, str(e)


def default_monitors(run=default_run):
    code, out = run(["hyprctl", "monitors", "-j"], timeout=4)
    if code != 0:
        return []
    try:
        data = json.loads(out)
    except json.JSONDecodeError:
        return []
    return data if isinstance(data, list) else []


def load_state(path=None):
    p = path or STATE_PATH
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return {"disabled": []}


def dump_state(state, path=None):
    p = path or STATE_PATH
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(state) + "\n")


def load_droppers(path=None):
    try:
        data = json.loads((path or DROPPERS_PATH).read_text())
    except (OSError, json.JSONDecodeError):
        return []
    return [d for d in data if isinstance(d, str)] if isinstance(data, list) else []


def remember_dropper(desc, path=None):
    p = path or DROPPERS_PATH
    known = load_droppers(p)
    if not desc or desc in known:
        return
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps(known + [desc]) + "\n")


@contextlib.contextmanager
def state_lock(path=None):
    p = (path or STATE_PATH).with_suffix(".lock")
    p.parent.mkdir(parents=True, exist_ok=True)
    with open(p, "w") as fh:
        fcntl.flock(fh, fcntl.LOCK_EX)
        yield


def _deps(run, monitors_fn, state_fn, save_state_fn):
    run = run or default_run
    return (
        run,
        monitors_fn or (lambda: default_monitors(run)),
        state_fn or load_state,
        save_state_fn or dump_state,
    )


def _restore(state, run):
    for spec in state.get("disabled") or []:
        spec = spec if isinstance(spec, dict) else {"name": spec}
        if not safe_name(spec.get("name")) or not safe_output(spec.get("output") or spec["name"]):
            continue
        run(eval_argv(restore_lua(spec)))


def plan_direct(monitors, droppers):
    enabled = [m for m in monitors or [] if not m.get("disabled") and safe_name(m.get("name"))]
    chosen = [m for m in enabled if m.get("description") in droppers]
    if len(chosen) == len(enabled):
        chosen = chosen[1:]
    return chosen


def dismiss_screensaver(run=None):
    run = run or default_run
    run(["pkill", "-x", "ttfx"])
    run(["pkill", "-f", "[o]rg.omarchy.screensaver"])
    run(["hyprctl", "eval", "hl.config({ cursor = { invisible = false } })"])


def set_displays(mode, run=None, monitors_fn=None, state_fn=None, save_state_fn=None,
                 droppers_path=None, disable=True):
    run, monitors_fn, state_fn, save_state_fn = _deps(run, monitors_fn, state_fn, save_state_fn)
    mode = "off" if mode in {"off", "disable"} else "on"

    if mode == "on":
        dismiss_screensaver(run)
        _restore(state_fn() or {}, run)
        save_state_fn({"disabled": []})
        for m in monitors_fn():
            name = m.get("name")
            if safe_name(name):
                run(dpms_argv("enable", name))
        return ""

    dismiss_screensaver(run)

    # Heads that drop off the bus on DPMS off are disabled instead, so they lose
    # signal once rather than reconnecting lit and being parked afterwards.
    # Lid close skips that: disabling the NVIDIA FlipGo heads leaves that GPU
    # with no outputs and Hyprland 0.56 SIGSEGVs.
    disabled = list((state_fn() or {}).get("disabled") or [])
    if disable:
        direct = plan_direct(monitors_fn(), load_droppers(droppers_path))
        for m in direct:
            run(eval_argv(disable_lua(selector(m))))
            disabled = [s for s in disabled if name_of(s) != m["name"]] + [snapshot(m)]
        save_state_fn({"disabled": disabled})

    for _kind, name, action in plan_off(monitors_fn()):
        run(dpms_argv(action, name))
    if not disable:
        return ""
    after = monitors_fn()
    disabled = list((state_fn() or {}).get("disabled") or [])
    for m in fallback_disable(after):
        run(eval_argv(disable_lua(selector(m))))
        disabled = [s for s in disabled if name_of(s) != m["name"]] + [snapshot(m)]
    save_state_fn({"disabled": disabled})
    return ""


# A head that drops off the bus on DPMS off comes back as a new, lit output.
# DPMS again would repeat the drop, so it is disabled until the others wake.
def park(name, run=None, monitors_fn=None, state_fn=None, save_state_fn=None,
         droppers_path=None):
    run, monitors_fn, state_fn, save_state_fn = _deps(run, monitors_fn, state_fn, save_state_fn)
    if not safe_name(name):
        return False
    mons = monitors_fn()
    target = next((m for m in mons if m.get("name") == name and not m.get("disabled")), None)
    others = [m for m in mons if m is not target and not m.get("disabled")]
    if target is None or not others or not blanked(others):
        return False
    run(eval_argv(disable_lua(selector(target))))
    remember_dropper(target.get("description"), droppers_path)
    parked = [s for s in (state_fn() or {}).get("disabled") or [] if name_of(s) != name]
    save_state_fn({"disabled": parked + [snapshot(target)]})
    return True


def sync(run=None, monitors_fn=None, state_fn=None, save_state_fn=None):
    run, monitors_fn, state_fn, save_state_fn = _deps(run, monitors_fn, state_fn, save_state_fn)
    state = state_fn() or {}
    if not state.get("disabled"):
        return "idle"
    if not awake(monitors_fn()):
        return "waiting"
    _restore(state, run)
    save_state_fn({"disabled": []})
    return "restored"
