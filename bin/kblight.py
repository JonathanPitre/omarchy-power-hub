"""Keyboard and power-button lights. Laptop 16 keys are USB; 12 and 13 use the EC."""
import json
import os
import pathlib
import re
import shutil
import subprocess

STATE_PATH = (
    pathlib.Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    / "omarchy-power-hub"
    / "kblight.json"
)
LEVEL = re.compile(r"(?:Brightness|Keyboard backlight):\s*(\d{1,3})\s*%")
FP_LEVEL = re.compile(r"Brightness:\s*(\d{1,3})\s*%")
REQUESTED = re.compile(r"Requested:\s*(.+)")
FP_LEVELS = {"auto", "high", "medium", "low", "ultra-low"}
PRIV = pathlib.Path(__file__).resolve().parent / "ops-priv"


def _run(args, timeout=4, input=None):
    try:
        p = subprocess.run(args, text=True, capture_output=True, timeout=timeout, input=input)
        return p.returncode, (p.stdout or "") + (p.stderr or "")
    except (OSError, subprocess.TimeoutExpired) as e:
        return 1, str(e)


def _deps(run, which):
    return run or _run, which or shutil.which


def _percent(text):
    m = LEVEL.search(text or "")
    if not m:
        return None
    n = int(m.group(1))
    return n if 0 <= n <= 100 else None


def _tool_ok(code, out):
    if code != 0:
        return False
    text = out or ""
    if "[ERROR]" in text or "Failed to communicate" in text or "Not a Framework Laptop" in text:
        return False
    return True


def _call(argv, run, priv=None):
    code, out = run(list(argv), timeout=4)
    if _tool_ok(code, out):
        return 0, out
    code, out = run(["sudo", "-n", *argv], timeout=4)
    if _tool_ok(code, out) or not priv:
        return code, out
    return run(["sudo", "-n", *priv], timeout=4)


def _priv_argv(tool_argv, priv_bin):
    if not priv_bin or len(tool_argv) < 2 or not str(tool_argv[1]).startswith("--"):
        return None
    return [str(priv_bin), tool_argv[1][2:], *tool_argv[2:]]


def _load_state(path):
    path = pathlib.Path(path)
    try:
        raw = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError, TypeError):
        return {}
    if not isinstance(raw, dict):
        return {}
    state = {}
    level = raw.get("level")
    if isinstance(level, int) and not isinstance(level, bool) and 0 <= level <= 100:
        state["level"] = level
    fp = raw.get("fp")
    if isinstance(fp, dict):
        cleaned = {}
        if fp.get("sys") is True:
            cleaned["sys"] = True
        requested = fp.get("requested")
        if requested in FP_LEVELS or requested == "custom":
            cleaned["requested"] = requested
        trigger = fp.get("trigger")
        if isinstance(trigger, str) and re.fullmatch(r"[A-Za-z0-9:_-]+", trigger):
            cleaned["trigger"] = trigger
        brightness = fp.get("brightness")
        if isinstance(brightness, int) and not isinstance(brightness, bool) and 0 <= brightness <= 100:
            cleaned["brightness"] = brightness
        if cleaned:
            state["fp"] = cleaned
    return state


def _load(path):
    return _load_state(path).get("level")


def _read_dmi(name):
    try:
        return (pathlib.Path("/sys/class/dmi/id") / name).read_text()
    except OSError:
        return ""


def model(read=None):
    read = read or _read_dmi
    vendor = read("sys_vendor")
    product = read("product_name")
    if "framework" not in vendor.lower():
        return ""
    compact = re.sub(r"[^a-z0-9]+", " ", product.lower())
    if re.search(r"\b16\b", compact):
        return "16"
    if re.search(r"\b1[23]\b", compact):
        return "ec"
    return ""


def _fp_state(text):
    req = REQUESTED.search(text or "")
    bright = FP_LEVEL.search(text or "")
    requested = re.sub(r"\s+", "-", req.group(1).strip().lower()) if req else ""
    brightness = int(bright.group(1)) if bright else None
    if brightness is not None and not 0 <= brightness <= 100:
        brightness = None
    if requested not in FP_LEVELS and requested != "custom":
        requested = ""
    if not requested and brightness is None:
        return None
    state = {}
    if requested:
        state["requested"] = requested
    if brightness is not None:
        state["brightness"] = brightness
    return state


def power_led(leds=None):
    root = pathlib.Path(leds or "/sys/class/leds")
    for p in sorted(root.glob("chromeos*:power")):
        if (p / "brightness").exists():
            return p
    return None


def _put(path, value, run):
    path = pathlib.Path(path)
    text = str(value)
    try:
        path.write_text(text)
        return True
    except OSError:
        pass
    code, _out = run(["sudo", "-n", "tee", str(path)], timeout=4, input=text)
    return code == 0


def _trigger(text):
    m = re.search(r"\[([^\]]+)\]", text or "")
    name = m.group(1) if m else None
    if name and re.fullmatch(r"[A-Za-z0-9:_-]+", name):
        return name
    return None


def _fp_restore(state):
    requested = state.get("requested")
    if requested in FP_LEVELS:
        return ["framework_tool", "--fp-led-level", requested]
    brightness = state.get("brightness")
    if isinstance(brightness, int) and not isinstance(brightness, bool):
        return ["framework_tool", "--fp-brightness", str(brightness)]
    return None


def _blank_power(led, run, state):
    trig = led / "trigger"
    bright = led / "brightness"
    mx = led / "max_brightness"
    fp = {"sys": True}
    try:
        name = _trigger(trig.read_text())
        if name:
            fp["trigger"] = name
    except OSError:
        pass
    try:
        n = int(bright.read_text().strip())
        if 0 <= n <= 100:
            fp["brightness"] = n
    except (OSError, ValueError):
        try:
            n = int(mx.read_text().strip())
            if 0 <= n <= 100:
                fp["brightness"] = n
        except (OSError, ValueError):
            fp["brightness"] = 100
    if _put(bright, "0", run):
        state["fp"] = fp


def _wake_power(led, run, fp):
    trig = led / "trigger"
    bright = led / "brightness"
    name = fp.get("trigger")
    if isinstance(name, str) and re.fullmatch(r"[A-Za-z0-9:_-]+", name):
        _put(trig, name, run)
    n = fp.get("brightness")
    if isinstance(n, int) and not isinstance(n, bool) and 0 <= n <= 100:
        _put(bright, str(n), run)


def _save(path, state):
    path = pathlib.Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(state) + "\n")


def _drop(path):
    try:
        pathlib.Path(path).unlink()
    except OSError:
        pass


def sync(mode, run=None, which=None, path=None, priv_bin=None, model_fn=None, leds=None):
    run, which = _deps(run, which)
    path = path or STATE_PATH
    if priv_bin is None and PRIV.is_file():
        priv_bin = PRIV
    kind = (model_fn or model)()
    if kind not in {"16", "ec"}:
        return ""
    fw = bool(which("framework_tool"))
    qmk = kind == "16" and bool(which("qmk_hid"))
    ec_keys = kind == "ec" and fw
    led = power_led(leds)
    if not qmk and not ec_keys and not fw and not led:
        return ""
    off = mode in {"off", "disable"}
    if qmk:
        keys = ["qmk_hid", "via", "--backlight"]
        key_priv = None
    else:
        keys = ["framework_tool", "--kblight"]
        key_priv = _priv_argv(keys, priv_bin)
    if not off:
        saved = _load_state(path)
        if not saved:
            return ""
        if (qmk or ec_keys) and "level" in saved:
            argv = keys + [str(saved["level"])]
            _call(argv, run, _priv_argv(argv, priv_bin) if ec_keys else None)
        fp = saved.get("fp") or {}
        if led and fp.get("sys"):
            _wake_power(led, run, fp)
        elif fw and fp:
            argv = _fp_restore(fp)
            if argv:
                _call(argv, run, _priv_argv(argv, priv_bin))
        _drop(path)
        return ""
    state = {}
    if qmk or ec_keys:
        code, out = _call(keys, run, key_priv)
        level = _percent(out) if _tool_ok(code, out) else None
        if level is not None:
            state["level"] = level
        set_keys = keys + ["0"]
        _call(set_keys, run, _priv_argv(set_keys, priv_bin) if ec_keys else None)
    if led:
        _blank_power(led, run, state)
    elif fw:
        get_fp = ["framework_tool", "--fp-brightness"]
        code, out = _call(get_fp, run, _priv_argv(get_fp, priv_bin))
        fp = _fp_state(out) if _tool_ok(code, out) else None
        if fp:
            state["fp"] = fp
        set_fp = ["framework_tool", "--fp-brightness", "0"]
        _call(set_fp, run, _priv_argv(set_fp, priv_bin))
    if state:
        _save(path, state)
    else:
        _drop(path)
    return ""
