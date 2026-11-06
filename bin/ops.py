#!/usr/bin/env python3
"""Omarchy Power Hub control plane. Stdlib only."""
import json, os, pathlib, re, shutil, subprocess, sys, time

HOME = pathlib.Path.home()
CFG = HOME / ".config/omarchy/omarchy-power-hub.json"
SHELL = HOME / ".config/omarchy/shell.json"
TWEAKS = HOME / ".config/omarchy/power-hub-tweaks.json"
DRM_ROOT = pathlib.Path("/sys/class/drm")
# Saving tweaks Travel turns on for its duration when they are off.
TRAVEL_TWEAKS = ("abm", "effects")
EFFECT_OPTIONS = {
    "animations:enabled": "animations = {{ enabled = {} }}",
    "decoration:shadow:enabled": "decoration = {{ shadow = {{ enabled = {} }} }}",
}
BINDINGS = HOME / ".config/hypr/bindings.lua"
RUN = pathlib.Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")) / "omarchy-power-hub"
CURSOR_FRESH_SECONDS = 5
PRIV = pathlib.Path(__file__).resolve().parent / "ops-priv"
HELPER_DIR = pathlib.Path("/usr/local/libexec/omarchy-power-hub")
HELPER_FILES = ("ops-priv", "hid_wakeup.py")
HIBERNATE_DROPIN = pathlib.Path("/etc/systemd/sleep.conf.d/90-omarchy-power-hub.conf")
SYS_POWER_DISK = pathlib.Path("/sys/power/disk")
LOCKDOWN = pathlib.Path("/sys/kernel/security/lockdown")
CMDLINE = pathlib.Path("/proc/cmdline")
CPU_ROOT = pathlib.Path("/sys/devices/system/cpu")
PROC = pathlib.Path("/proc")
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import display_power  # noqa: E402
import idle_wake  # noqa: E402
import kblight  # noqa: E402
from display_power import laptop_monitor, set_displays  # noqa: E402
WEEK = 7 * 24 * 3600
BEGIN = "-- BEGIN Omarchy Power Hub lid action override"
END = "-- END Omarchy Power Hub lid action override"
LID_ACTIONS = ("nothing", "display-off", "suspend", "hibernate")
PROFILE_ALIASES = {
    "auto": "balanced",
    "balanced": "balanced",
    "saver": "power-saver",
    "power-saver": "power-saver",
    "full": "performance",
    "performance": "performance",
}

def listed_profiles():
    code, out = run(["omarchy-powerprofiles-list"])
    if code != 0:
        return None
    return {line.split()[0] for line in out.splitlines() if line.split()}

def default_profile(side, available=None):
    if available is None:
        available = listed_profiles()
    if side == "ac" and (available is None or "performance" in available):
        return "performance"
    return "balanced"

def canonical_profile(value, side, available=None):
    if value in PROFILE_ALIASES:
        return PROFILE_ALIASES[value]
    return default_profile(side, available)

def source_defaults(side="ac", available=None, profile=None):
    return {
        "screensaver": 150,
        "displaysOff": 0,
        "lock": 300,
        "sleep": 0,
        "hibernateAfter": 0,
        "lid": "suspend",
        "profile": profile or default_profile(side, available),
    }

# Only asks omarchy-powerprofiles-list (~90 ms) when a side has no valid saved
# profile; every idle click and status poll goes through here.
def load():
    try:
        raw = json.loads(CFG.read_text())
    except (OSError, json.JSONDecodeError):
        raw = {}
    cfg = {"chargeLimit": 80, "boost": False, "boostSavedLimit": 80}
    for side in ("ac", "battery"):
        src = raw.get(side) or {}
        base = source_defaults(side, profile=PROFILE_ALIASES.get(str(src.get("profile"))))
        base.update({k: src[k] for k in base if k in src and k != "profile"})
        # Older configs saved "system-default", which logind handles as suspend.
        if base["lid"] not in LID_ACTIONS:
            base["lid"] = "suspend"
        cfg[side] = base
    for k in ("chargeLimit", "boost", "boostSavedLimit"):
        if k in raw:
            cfg[k] = raw[k]
    return cfg

def save(cfg):
    CFG.parent.mkdir(parents=True, exist_ok=True)
    CFG.write_text(json.dumps(cfg, indent=2) + "\n")

def run(args, timeout=4):
    try:
        p = subprocess.run(args, text=True, capture_output=True, timeout=timeout)
        return p.returncode, (p.stdout or "") + (p.stderr or "")
    except (OSError, subprocess.TimeoutExpired) as e:
        return 1, str(e)

POWER_SUPPLY = pathlib.Path("/sys/class/power_supply")
MIN_ADAPTER_WATTS = 15

def ucsi_source_watts(root=None):
    """Best online USB-C source contract in watts, or None without UCSI."""
    root = POWER_SUPPLY if root is None else root
    ports = sorted(root.glob("ucsi-source-psy-*"))
    if not ports:
        return None
    best = 0.0
    for port in ports:
        try:
            if (port / "online").read_text().strip() != "1":
                continue
            volts = int((port / "voltage_now").read_text()) / 1e6
            amps = int((port / "current_now").read_text()) / 1e6
        except (OSError, ValueError):
            continue
        best = max(best, volts * amps)
    return best

# Monitors and docks can feed a few watts back over USB-C, and ACPI and UPower
# count that as AC. Only a source that can run the laptop counts on UCSI.
def on_battery():
    watts = ucsi_source_watts()
    if watts is not None:
        return watts < MIN_ADAPTER_WATTS
    code, out = run(["busctl", "get-property", "org.freedesktop.UPower",
                     "/org/freedesktop/UPower", "org.freedesktop.UPower", "OnBattery"])
    return "true" in out.lower()

def pack_discharging(root=None):
    root = POWER_SUPPLY if root is None else root
    for bat in sorted(root.glob("BAT*")):
        try:
            if (bat / "status").read_text().strip() == "Discharging":
                return True
        except OSError:
            continue
    return False

def pack_percent(root=None):
    root = POWER_SUPPLY if root is None else root
    levels = [int(p.read_text()) for p in root.glob("BAT*/capacity")]
    return max(levels, default=0)

# Above the limit the Framework EC discharges the pack with the adapter connected.
def draining_to_limit(cfg):
    return framework() and not cfg.get("boost") and pack_percent() > int(cfg.get("chargeLimit") or 80)

def drain_to(cfg=None):
    cfg = cfg or load()
    draining = pack_discharging() and draining_to_limit(cfg) and not on_battery()
    return int(cfg.get("chargeLimit") or 80) if draining else 0

def active_side(cfg=None):
    if pack_discharging() and not draining_to_limit(cfg or load()):
        return "battery"
    return "battery" if on_battery() else "ac"

# The shell watches shell.json and reloads its whole config on every write.
def shell_idle(screensaver, lock):
    try:
        data = json.loads(SHELL.read_text())
    except (OSError, json.JSONDecodeError):
        return "shell.json unreadable"
    idle = data.setdefault("idle", {})
    want = {
        "screensaver": WEEK if int(screensaver) <= 0 else int(screensaver),
        "lock": WEEK if int(lock) <= 0 else int(lock),
    }
    if all(idle.get(k) == v for k, v in want.items()):
        return ""
    idle.update(want)
    SHELL.write_text(json.dumps(data, indent=2) + "\n")
    return ""

# Displays going off first would close the saver, so it never starts. Mirrors
# effectiveSaver in Model.js; the saved value returns if Displays off moves past it.
def effective_saver(src):
    saver, displays = int(src["screensaver"]), int(src.get("displaysOff") or 0)
    return 0 if 0 < displays <= saver else saver

def idle_for_blank(blank, cfg=None):
    cfg = cfg or load()
    src = cfg[active_side(cfg)]
    return shell_idle(0 if blank else effective_saver(src), src["lock"])

def profile_state_dir():
    base = os.environ.get("XDG_STATE_HOME")
    root = pathlib.Path(base) if base else HOME / ".local" / "state"
    return root / "omarchy" / "powerprofiles"

def remember_profile(side, name):
    which = "battery" if side == "battery" else "ac"
    path = profile_state_dir() / which
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(name + "\n")

# Omarchy's battery service runs `omarchy-powerprofiles-set <side>` on every
# power-source change and at shell start, applying the profile remembered here.
# Travel holds both files at power-saver so that handler cannot undo it, and
# restore writes the normal profiles back.
def remember_travel_profiles(cfg, held=None):
    for side in ("ac", "battery"):
        if held or side in cfg:
            remember_profile(side, held or canonical_profile(cfg[side]["profile"], side))

def apply_profile(side, mode, live=True):
    which = "battery" if side == "battery" else "ac"
    name = canonical_profile(mode, which)
    if not live or which != active_side():
        remember_profile(which, name)
        return ""
    code, out = run(["omarchy-powerprofiles-set", which, name])
    return out.strip() if code else ""

# The passwordless sudo rule covers only this root-owned copy: pointing it at
# the plugin folder would let anything running as the user edit ops-priv and
# become root. A copy that differs from the plugin's is treated as absent.
def helper_bin():
    try:
        same = all((HELPER_DIR / n).read_bytes() == (PRIV.parent / n).read_bytes()
                   for n in HELPER_FILES)
    except OSError:
        return None
    return HELPER_DIR / "ops-priv" if same else None

def helper_needed():
    if not helper_bin():
        return True
    code, out = run(["sudo", "-n", "-l"], timeout=4)
    return code != 0 or f"NOPASSWD: {HELPER_DIR}/ops-priv" not in out

def priv(args):
    helper = helper_bin()
    if helper:
        code, out = run(["sudo", "-n", str(helper), *args], timeout=8)
        if code == 0:
            return ""
    code, out = run(["pkexec", str(PRIV), *args], timeout=30)
    return "" if code == 0 else out.strip() or "privilege required"

def framework():
    try:
        vendor = pathlib.Path("/sys/class/dmi/id/sys_vendor").read_text()
    except OSError:
        vendor = ""
    return "framework" in vendor.lower()

def parse_busctl_value(raw):
    text = (raw or "").strip().split("\n", 1)[0]
    if not text:
        return None
    parts = text.split(None, 1)
    if len(parts) < 2:
        return text
    typ, val = parts[0], parts[1]
    if typ == "b":
        return val.lower() in ("true", "1")
    if typ in ("u", "t", "i", "n", "q", "x"):
        try:
            return int(val)
        except ValueError:
            return None
    if typ == "s":
        return val.strip().strip('"')
    return val

def parse_upower_history(raw):
    parts = (raw or "").strip().split("\n", 1)[0].split()
    if len(parts) < 2 or parts[0] != "a(udu)":
        return []
    try:
        n = int(parts[1])
    except ValueError:
        return []
    vals = parts[2:]
    if n < 0 or len(vals) < n * 3:
        return []
    rows = []
    for i in range(n):
        try:
            t = int(vals[i * 3])
            pct = float(vals[i * 3 + 1])
            state = int(vals[i * 3 + 2])
        except (ValueError, IndexError):
            return []
        rows.append({"t": t, "pct": pct, "charging": state == 1})
    return rows

def upower_prop(run_fn, device, prop):
    code, out = run_fn(["busctl", "get-property", "org.freedesktop.UPower", device,
                        "org.freedesktop.UPower.Device", prop])
    if code != 0:
        return None
    return parse_busctl_value(out)

def upower_batteries(run_fn):
    code, out = run_fn(["upower", "-e"])
    if code != 0:
        return []
    rows = []
    for line in out.splitlines():
        device = line.strip()
        if not device or device.endswith("/DisplayDevice"):
            continue
        if upower_prop(run_fn, device, "ChargeThresholdSupported") is True:
            rows.append(device)
    return rows

def battery_history(run_fn=None):
    run_fn = run_fn or run
    code, out = run_fn(["upower", "-e"])
    device = ""
    if code == 0:
        for line in out.splitlines():
            path = line.strip()
            if "battery_" in path and not path.endswith("/DisplayDevice"):
                device = path
                break
    if not device:
        return []
    code, out = run_fn(["busctl", "call", "org.freedesktop.UPower", device,
                        "org.freedesktop.UPower.Device", "GetHistory",
                        "suu", "charge", "172800", "400"])
    if code != 0:
        return []
    return parse_upower_history(out)

def upower_health_status(run_fn=None):
    run_fn = run_fn or run
    empty = {"kind": "none", "available": False, "reason": "no charge threshold",
             "limit": None, "tool": False, "health": None, "policy": None}
    batteries = upower_batteries(run_fn)
    if not batteries:
        return empty
    enabled = True
    policy = None
    for device in batteries:
        if upower_prop(run_fn, device, "ChargeThresholdEnabled") is not True:
            enabled = False
        if policy is not None:
            continue
        settings = upower_prop(run_fn, device, "ChargeThresholdSettingsSupported") or 0
        start = upower_prop(run_fn, device, "ChargeStartThreshold")
        end = upower_prop(run_fn, device, "ChargeEndThreshold")
        if isinstance(settings, int) and (settings & 2) and isinstance(end, int):
            if (settings & 1) and isinstance(start, int) and start < end:
                policy = f"{start}-{end}"
            else:
                policy = str(end)
        elif isinstance(settings, int) and (settings & 4):
            policy = "firmware"
    return {"kind": "upower", "available": True, "reason": "", "limit": None,
            "tool": False, "health": enabled, "policy": policy or "firmware"}

def charge_status():
    none = {"kind": "none", "available": False, "reason": "", "limit": None,
            "tool": False, "health": None, "policy": None}
    if framework():
        tool = shutil.which("framework_tool")
        if not tool:
            none["reason"] = "framework_tool is not installed"
            return none
        helper = helper_bin()
        if not helper:
            return {**none, "kind": "framework", "tool": True, "reason": "permission"}
        code, out = run(["sudo", "-n", str(helper), "charge-limit"], timeout=6)
        m = re.search(r"Maximum\D*(\d{1,3})", out) if code == 0 else None
        reason = "" if m else "permission" if "password" in out else out.strip()
        return {"kind": "framework", "available": bool(m), "reason": reason,
                "limit": int(m.group(1)) if m else None,
                "tool": True, "health": None, "policy": None}
    return upower_health_status()

def set_charge(limit):
    limit = max(40, min(100, int(limit)))
    if not framework():
        return "not a Framework machine"
    if not shutil.which("framework_tool"):
        return "framework_tool is not installed"
    return priv(["charge-limit", str(limit)])

def set_health(on, run_fn=None):
    run_fn = run_fn or run
    desired = "true" if on else "false"
    batteries = upower_batteries(run_fn)
    if not batteries:
        return "Battery charge limits are not supported"
    failed = False
    for device in batteries:
        code, _ = run_fn(["busctl", "call", "org.freedesktop.UPower", device,
                          "org.freedesktop.UPower.Device", "EnableChargeThreshold",
                          "b", desired])
        if code != 0:
            failed = True
    return "" if not failed else "Failed to update charge limits"

def maybe_reapply_framework_limit(cfg, ch, run_fn=None, helper_fn=None):
    run_fn = run_fn or run
    ch = ch or {}
    if ch.get("kind") != "framework" or ch.get("limit") is None or cfg.get("boost"):
        return ch
    wanted = max(40, min(100, int(cfg.get("chargeLimit") or 80)))
    if ch.get("limit") == wanted:
        return ch
    helper = (helper_fn or helper_bin)()
    if not helper:
        return ch
    code, _ = run_fn(["sudo", "-n", str(helper), "charge-limit", str(wanted)])
    if code != 0:
        return ch
    next_ch = dict(ch)
    next_ch["limit"] = wanted
    next_ch["available"] = True
    next_ch["reason"] = ""
    return next_ch

# A timed-out PPM init at boot leaves no USB-C data until the driver is rebound.
def recover_ucsi(run_fn=None):
    run_fn = run_fn or run
    mark = RUN / "ucsi-rebound"
    if ucsi_source_watts() is not None or mark.exists() or not framework():
        return
    helper = helper_bin()
    if not helper:
        return
    RUN.mkdir(parents=True, exist_ok=True)
    mark.touch()
    run_fn(["sudo", "-n", str(helper), "ucsi-rebind"])

def _text(path):
    try:
        return path.read_text()
    except OSError:
        return ""

def _lockdown_on():
    m = re.search(r"\[([^\]]+)\]", _text(LOCKDOWN))
    return bool(m) and m.group(1) != "none"

def _bitwarden_running():
    for cmd in PROC.glob("[0-9]*/cmdline"):
        try:
            raw = cmd.read_bytes().replace(b"\0", b" ").decode("utf-8", "replace")
        except OSError:
            continue
        if "/bitwarden/" in raw:
            return True
    return False

def hibernate_available():
    if _text(SYS_POWER_DISK).strip() == "[disabled]":
        if _lockdown_on():
            return {"available": False,
                    "reason": "kernel lockdown is blocking hibernation"}
        if "nohibernate" in _text(CMDLINE).split():
            return {"available": False,
                    "reason": "the kernel was booted with nohibernate"}
        if _bitwarden_running():
            return {"available": False,
                    "reason": "Bitwarden holds secret memory that blocks hibernation. Set SECURE_KEY_CONTAINER_BACKEND=mlock and restart Bitwarden."}
        return {"available": False,
                "reason": "an application holds secret memory, which blocks hibernation"}
    _, out = run(["busctl", "call", "org.freedesktop.login1",
                  "/org/freedesktop/login1", "org.freedesktop.login1.Manager",
                  "CanHibernate"], timeout=4)
    text = out.lower()
    ok = "yes" in text and "challenge" not in text and '"no"' not in text
    reason = "" if ok else (
        "hibernate is not available: check swap and a resume= kernel parameter")
    return {"available": ok, "reason": reason}

def hibernate_delay_applied():
    try:
        m = re.search(r"^HibernateDelaySec=(\d+)$", HIBERNATE_DROPIN.read_text(), re.M)
    except OSError:
        return 0
    return int(m.group(1)) if m else 0

def apply_hibernate_delay(seconds):
    seconds = int(seconds)
    if seconds == hibernate_delay_applied():
        return ""
    info = hibernate_available()
    if seconds > 0 and not info["available"]:
        return info["reason"] or "hibernate is not available"
    return priv(["hibernate-delay", str(seconds)])

def gpu_status(root=pathlib.Path("/sys")):
    for dev in sorted((root / "bus" / "pci" / "devices").glob("*")):
        # boot_vga exists only on VGA-class devices; Optimus cards are 3D controllers without it.
        boot_vga = _text(dev / "boot_vga").strip()
        discrete = boot_vga == "0" or (not boot_vga and (dev / "driver").resolve().name in {"nvidia", "nouveau"})
        if _text(dev / "class").startswith("0x03") and discrete:
            return {"present": True, "asleep": _text(dev / "power" / "runtime_status").strip() == "suspended"}
    return {"present": False, "asleep": False}

def travel_path():
    return RUN / "travel.json"

def travel_state():
    state = read_energy(travel_path(), {"on": False})
    if state.get("invalid"):
        return state
    if type(state.get("on")) is not bool:
        return {"on": True, "invalid": True,
                "error": "Travel restore point is malformed; restore ownership manually"}
    if state.get("on"):
        legacy = (isinstance(state.get("monitors"), list)
                  and (state.get("brightness") is None
                       or type(state.get("brightness")) is int))
        current = _valid_travel_snapshot(state)
        if not legacy and not current:
            return {"on": True, "invalid": True,
                    "error": "Travel restore point is malformed; restore ownership manually"}
    return state

def quick_dim_state():
    state = read_energy(RUN / "quick-dim.json", {"on": False})
    if state.get("invalid"):
        return state
    if type(state.get("on")) is not bool:
        return {"on": True, "invalid": True,
                "error": "Quick dim restore point is malformed; restore ownership manually"}
    if state.get("on") and (not display_power.safe_name(state.get("monitor"))
                            or type(state.get("brightness")) is not int
                            or not 0 <= state["brightness"] <= 100):
        return {"on": True, "invalid": True,
                "error": "Quick dim restore point is malformed; restore ownership manually"}
    return state

def brightness_get(monitor=None):
    screens = monitors()
    internal = monitor or laptop_monitor(screens)
    if not internal or not display_power.safe_name(internal) or not any(
            isinstance(item, dict) and item.get("name") == internal for item in screens):
        return None
    code, out = run(["omarchy", "brightness", "display", "--monitor", internal], timeout=4)
    if code:
        return None
    text = out.strip()
    if not re.fullmatch(r"\d{1,3}", text):
        return None
    value = int(text)
    return value if 0 <= value <= 100 else None

def brightness_set(monitor, value):
    if type(value) is not int or not 0 <= value <= 100 or not display_power.safe_name(monitor):
        return "Invalid display brightness target"
    if not any(isinstance(item, dict) and item.get("name") == monitor for item in monitors()):
        return "Display is no longer connected"
    code, out = run(["omarchy", "brightness", "display", "--no-osd", "--monitor",
                     monitor, f"{value}%"], timeout=4)
    return "" if code == 0 else out.strip() or "Brightness command failed"

def energy_lock():
    return display_power.state_lock(RUN / "energy-controls.json")

def atomic_state(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(tmp, path)
        os.chmod(path, 0o600)
    except Exception:
        try:
            tmp.unlink()
        except OSError:
            pass
        raise

def read_energy(path, default):
    try:
        value = json.loads(path.read_text())
        return value if isinstance(value, dict) else {"on": True, "invalid": True, "error": "Saved settings are malformed"}
    except FileNotFoundError:
        return default
    except (OSError, json.JSONDecodeError):
        return {"on": True, "invalid": True, "error": "Saved settings are unreadable; restore ownership manually"}

def travel_owned():
    return bool(travel_state().get("on"))

def quick_dim(on):
    with energy_lock():
        if travel_owned():
            return "Turn off Travel mode before changing Quick dim"
        path = RUN / "quick-dim.json"
        state = quick_dim_state()
        if state.get("invalid"):
            return state.get("error", "Quick dim restore point is malformed")
        if state.get("on") == on:
            return ""
        if on:
            monitor = laptop_monitor(monitors())
            if not monitor:
                return "Laptop display is unavailable"
            before = brightness_get(monitor)
            if before is None:
                return "Laptop brightness cannot be read"
            saved = {"on": True, "monitor": monitor, "brightness": before, "error": ""}
            try:
                atomic_state(path, saved)
            except OSError as error:
                return f"Could not save brightness restore point: {error}"
            err = brightness_set(monitor, min(before, 40))
            actual = brightness_get(monitor)
            if err or actual is None or abs(actual - min(before, 40)) > 1:
                saved["error"] = err or "Brightness did not reach the requested level"
                atomic_state(path, saved)
                return saved["error"]
            return ""
        monitor, before = state.get("monitor"), state.get("brightness")
        if not display_power.safe_name(monitor) or type(before) is not int or not 0 <= before <= 100:
            return "Saved brightness restore point is invalid"
        err = brightness_set(monitor, before)
        actual = brightness_get(monitor)
        if err or actual is None or abs(actual - before) > 1:
            state["error"] = err or "Brightness restoration could not be verified"
            atomic_state(path, state)
            return state["error"]
        atomic_state(path, {"on": False})
        return ""
def monitors():
    code, out = run(["hyprctl", "monitors", "-j"], timeout=4)
    try:
        return json.loads(out) if code == 0 else []
    except json.JSONDecodeError:
        return []

def _monitor_snapshot(mon):
    snapshot = display_power.snapshot(mon)
    transform = mon.get("transform", 0)
    if type(transform) is not int or not 0 <= transform <= 7:
        raise ValueError("Invalid laptop display transform")
    if transform in {1, 3, 5, 7}:
        snapshot["width"], snapshot["height"] = snapshot["height"], snapshot["width"]
    snapshot["transform"] = transform
    return snapshot

def _target_refresh(mon):
    try:
        current = float(mon["refreshRate"])
        width, height = int(mon["width"]), int(mon["height"])
        transform = mon.get("transform", 0)
    except (KeyError, TypeError, ValueError):
        return None
    if current <= 60 or type(transform) is not int or not 0 <= transform <= 7:
        return None
    if transform in {1, 3, 5, 7}:
        width, height = height, width
    modes = []
    for mode in mon.get("availableModes", []):
        match = re.fullmatch(r"(\d+)x(\d+)@([\d.]+)(?:Hz)?", str(mode).strip())
        if match and int(match.group(1)) == width and int(match.group(2)) == height:
            rate = float(match.group(3))
            if 0 < rate <= 60:
                modes.append(rate)
    return max(modes) if modes else None

def _valid_monitor_snapshot(saved):
    if not isinstance(saved, dict) or not display_power.safe_output(saved.get("output")):
        return False
    if not display_power.safe_name(saved.get("name")):
        return False
    for key in ("width", "height", "x", "y"):
        if type(saved.get(key)) is not int:
            return False
    if saved["width"] <= 0 or saved["height"] <= 0:
        return False
    transform = saved.get("transform", 0)
    if type(transform) is not int or not 0 <= transform <= 7:
        return False
    for key, low, high in (("refreshRate", 0, 1000), ("scale", 0, 10)):
        value = saved.get(key)
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not low < value <= high:
            return False
    return True
def _valid_travel_snapshot(state):
    if (type(state.get("complete")) is not bool
            or type(state.get("reduceCpu")) is not bool
            or state.get("source") not in ("ac", "battery")
            or not isinstance(state.get("pending"), list)
            or not isinstance(state.get("cpu"), dict)
            or not isinstance(state.get("controls"), dict)
            or not isinstance(state.get("error"), str)):
        return False
    profile = state.get("profile")
    if profile is not None and not isinstance(profile, str):
        return False
    brightness = state.get("brightness")
    if brightness is not None and (
            not isinstance(brightness, dict)
            or not display_power.safe_name(brightness.get("monitor"))
            or type(brightness.get("before")) is not int
            or not 0 <= brightness["before"] <= 100):
        return False
    monitor = state.get("monitor")
    if monitor is not None and not _valid_monitor_snapshot(monitor):
        return False
    cpu = state["cpu"]
    turbo = cpu.get("turbo")
    if turbo is not None and type(turbo) is not bool:
        return False
    preferences = cpu.get("epp")
    if not isinstance(preferences, list):
        return False
    indexes = set()
    names = ("power", "balance_power", "balance_performance", "performance")
    for entry in preferences:
        if (not isinstance(entry, dict)
                or type(entry.get("cpu")) is not int
                or entry["cpu"] < 0
                or entry["cpu"] in indexes
                or entry.get("before") not in names
                or (entry.get("target") is not None
                    and entry.get("target") not in ("power", "balance_power"))):
            return False
        indexes.add(entry["cpu"])
    pending = state["pending"]
    if (any(not isinstance(token, str) for token in pending)
            or len(pending) != len(set(pending))):
        return False
    tweaks = state.get("tweaks", [])
    if not isinstance(tweaks, list) or not set(tweaks) <= set(TRAVEL_TWEAKS):
        return False
    valid_tokens = {"profile", "turbo", "brightness", "refresh"}
    valid_tokens.update(f"epp:{index}" for index in indexes)
    valid_tokens.update(f"tweak:{key}" for key in tweaks)
    if not set(pending) <= valid_tokens:
        return False
    epp_pending = {token for token in pending if token.startswith("epp:")}
    if ("profile" in pending and not profile
            or "turbo" in pending and turbo is None
            or "brightness" in pending and brightness is None
            or "refresh" in pending and monitor is None
            or not state["reduceCpu"] and (
                profile is not None or turbo is not None or preferences or epp_pending
            )):
        return False
    controls = state["controls"]
    for name in ("profile", "turbo", "epp", "brightness", "refresh"):
        item = controls.get(name)
        if (not isinstance(item, dict) or type(item.get("available")) is not bool
                or not isinstance(item.get("detail"), str)):
            return False
    if state["complete"]:
        expected = set()
        if profile is not None:
            expected.add("profile")
        if turbo is not None:
            expected.add("turbo")
        expected.update(f"epp:{index}" for index in indexes)
        expected.update(f"tweak:{key}" for key in tweaks)
        if brightness is not None:
            expected.add("brightness")
        refresh = controls.get("refresh")
        detail = refresh.get("detail", "") if isinstance(refresh, dict) else ""
        if monitor is not None and detail.startswith("Laptop refresh capped at "):
            expected.add("refresh")
        if set(pending) != expected:
            return False
    return True


def _monitor_change(saved, refresh):
    if not _valid_monitor_snapshot(saved):
        return "Saved monitor restore point is invalid"
    spec = dict(saved)
    spec["refreshRate"] = refresh
    code, out = run(display_power.eval_argv(display_power.restore_lua(spec)), timeout=8)
    if code:
        return out.strip() or "Monitor mode change failed"
    actual = next((m for m in monitors() if m.get("name") == saved["name"]), None)
    if not actual:
        return "Saved laptop display is unavailable"
    width, height = saved["width"], saved["height"]
    if saved.get("transform", 0) in {1, 3, 5, 7}:
        width, height = height, width
    try:
        matches = (int(actual["width"]) == width and int(actual["height"]) == height and
                   abs(float(actual["refreshRate"]) - float(refresh)) <= 0.1)
    except (KeyError, TypeError, ValueError):
        matches = False
    return "" if matches else "Laptop display mode could not be verified"
def _turbo_path():
    intel = CPU_ROOT / "intel_pstate/no_turbo"
    boost = CPU_ROOT / "cpufreq/boost"
    return intel if intel.exists() else boost if boost.exists() else None

def _turbo_value():
    path = _turbo_path()
    try:
        raw = path.read_text().strip()
        if raw not in {"0", "1"}:
            return None
        return raw == "0" if path.name == "no_turbo" else raw == "1"
    except (OSError, AttributeError):
        return None

def _cpu_preferences():
    found = []
    for path in sorted(CPU_ROOT.glob("cpu[0-9]*/cpufreq/energy_performance_preference")):
        try:
            cpu = int(path.parts[-3][3:])
            current = path.read_text().strip()
            if current not in {"power", "balance_power", "balance_performance", "performance"}:
                continue
            found.append({"cpu": cpu, "before": current, "target": None})
        except (OSError, ValueError):
            continue
    return found

def _epp_saving_target(cpu):
    path = CPU_ROOT / f"cpu{cpu}/cpufreq/energy_performance_available_preferences"
    try:
        available = path.read_text().split()
    except OSError:
        return None
    return "power" if "power" in available else "balance_power" if "balance_power" in available else None

def _epp_saving_supported():
    return any(_epp_saving_target(int(path.parts[-3][3:]))
               for path in CPU_ROOT.glob("cpu[0-9]*/cpufreq/energy_performance_preference"))

def _cpu_available():
    profiles = listed_profiles() or set()
    code, current = run(["powerprofilesctl", "get"])
    return {
        "profile": "power-saver" in profiles and code == 0 and bool(current.strip()),
        "turbo": _turbo_value() is not None,
        "epp": _epp_saving_supported(),
    }

def _apply_cpu_overlay(state):
    if state.get("profile") and state["controls"]["profile"]["available"]:
        # A re-apply finds boost already off; power-profiles-daemon then fails every
        # profile on amd_pstate, so lift it first. The turbo step below turns it off again.
        if state.get("cpu", {}).get("turbo") is not None and _turbo_value() is False:
            err = priv(["turbo", "on"])
            if err:
                return err
        remember_travel_profiles(None, held="power-saver")
        code, out = run(["powerprofilesctl", "set", "power-saver"])
        if code:
            return out.strip() or "Power saver could not be applied"
        atomic_state(travel_path(), state)
        code, out = run(["powerprofilesctl", "get"])
        if code or out.strip() != "power-saver":
            return f"Power saver was replaced by {out.strip()}" if out.strip() else "Power saver could not be verified"
    turbo = state.get("cpu", {}).get("turbo")
    if turbo is not None:
        atomic_state(travel_path(), state)
        err = priv(["turbo", "off"])
        if err or _turbo_value() is not False:
            return err or "CPU turbo could not be disabled"
    preferences = state.get("cpu", {}).get("epp", [])
    saving_count = 0
    for entry in preferences:
        target = _epp_saving_target(entry["cpu"])
        entry["target"] = target
        saving_count += target is not None
    epp_control = state["controls"]["epp"]
    epp_control["available"] = saving_count > 0
    if saving_count == 0:
        epp_control["detail"] = "No saving CPU preference supported"
    elif saving_count < len(preferences):
        epp_control["detail"] = (
            f"Energy-saving CPU preference on {saving_count} of {len(preferences)} CPUs"
        )
    else:
        epp_control["detail"] = "Energy-saving CPU preference"
    atomic_state(travel_path(), state)
    for entry in preferences:
        target = entry["target"]
        if not target:
            continue
        path = CPU_ROOT / f"cpu{entry['cpu']}/cpufreq/energy_performance_preference"
        atomic_state(travel_path(), state)
        err = priv(["epp-cpu", str(entry["cpu"]), target])
        if err:
            return err
        try:
            if path.read_text().strip() != target:
                return f"CPU {entry['cpu']} preference could not be verified"
        except OSError:
            return f"CPU {entry['cpu']} preference is unavailable"
    return ""
def _restore_cpu(state, path, rollback=False, cfg=None):
    cpu = state.get("cpu", {})
    pending = state.get("pending", [])
    restore_profile = "profile" in pending
    profiles = (listed_profiles() or set()) if state.get("reduceCpu") else set()
    if restore_profile:
        if rollback:
            desired = state.get("profile")
        else:
            side = active_side(cfg)
            desired = cfg[side]["profile"]
        if not isinstance(desired, str) or desired not in profiles:
            return "Saved normal power profile is unavailable"
    elif not rollback and state.get("reduceCpu") and profiles:
        side = active_side(cfg)
        desired = cfg[side]["profile"]
        restore_profile = isinstance(desired, str) and desired in profiles
    if restore_profile:
        if cpu.get("turbo") is not None and "turbo" not in pending:
            pending.append("turbo")
        for entry in cpu.get("epp", []):
            token = f"epp:{entry.get('cpu')}"
            if token not in pending:
                pending.append(token)
        atomic_state(path, state)
        # amd_pstate rejects policyN/boost=1 while global boost is off, which makes
        # power-profiles-daemon fail balanced/performance. Re-enable boost first;
        # the turbo step below then restores the saved value.
        if "turbo" in pending:
            err = priv(["turbo", "on"])
            if err:
                return err
        code, out = run(["powerprofilesctl", "set", desired])
        if code:
            return out.strip() or "Power profile restoration failed"
        remember_travel_profiles(cfg or load())
        code, out = run(["powerprofilesctl", "get"])
        if code or out.strip() != desired:
            return "Power profile restoration could not be verified"
        atomic_state(path, state)
    if "turbo" in pending:
        old = cpu.get("turbo")
        if type(old) is not bool:
            return "Saved CPU turbo state is invalid"
        err = priv(["turbo", "on" if old else "off"])
        if err or _turbo_value() is not old:
            return err or "CPU turbo restoration could not be verified"
        pending.remove("turbo")
        atomic_state(path, state)
    for entry in cpu.get("epp", []):
        token = f"epp:{entry.get('cpu')}"
        if token not in pending:
            continue
        index, old = entry.get("cpu"), entry.get("before")
        if type(index) is not int or index < 0 or old not in {"power", "balance_power", "balance_performance", "performance"}:
            return "Saved CPU preference restore point is invalid"
        pref = CPU_ROOT / f"cpu{index}/cpufreq/energy_performance_preference"
        try:
            available = (pref.parent / "energy_performance_available_preferences").read_text().split()
            if not pref.exists():
                return f"CPU {index} is unavailable"
            if old not in available:
                notice = f"CPU {index} original preference is incompatible with the current power-source profile; keeping its supported value"
                state["error"] = (state.get("error", "") + "; " if state.get("error") else "") + notice
                pending.remove(token)
                atomic_state(path, state)
                continue
        except OSError:
            return f"CPU {index} is unavailable"
        err = priv(["epp-cpu", str(index), old])
        try:
            matches = pref.read_text().strip() == old
        except OSError:
            matches = False
        if err or not matches:
            return err or f"CPU {index} preference restoration failed"
        pending.remove(token)
        atomic_state(path, state)
    return ""
def travel(on, cfg=None):
    with energy_lock():
        cfg = cfg or load()
        state = travel_state()
        if state.get("invalid"):
            return state.get("error", "Travel restore point is malformed")
        if on and state.get("on"):
            return ""
        if not on and not state.get("on"):
            return ""
        path = travel_path()
        if on:
            current = monitors()
            internal = laptop_monitor(current)
            before = brightness_get(internal) if internal else None
            mon = next((m for m in current if isinstance(m, dict) and m.get("name") == internal), None)
            saved_mon = None
            if mon:
                try:
                    saved_mon = _monitor_snapshot(mon)
                except (TypeError, ValueError):
                    pass
            target = _target_refresh(mon) if saved_mon else None
            # Travel always switches to Power saver. On amd_pstate that already turns
            # per-CPU boost off and sets EPP to power, so turbo and EPP are left alone
            # (turning global boost off also breaks power-profiles-daemon).
            profiles = listed_profiles() or set()
            profile = None
            if _cpu_available().get("profile") and "power-saver" in profiles:
                code, out = run(["powerprofilesctl", "get"])
                profile = out.strip() if code == 0 and out.strip() in profiles else None
            reduce_cpu = profile is not None
            available = {"profile": reduce_cpu, "turbo": False, "epp": False}
            turbo, preferences = None, []
            if before is None and target is None and not reduce_cpu:
                return "No supported battery-saving controls"
            try:
                current_refresh = float(mon.get("refreshRate")) if mon else None
            except (TypeError, ValueError):
                current_refresh = None
            refresh_available = bool(target is not None or
                                     current_refresh is not None and current_refresh <= 60)
            refresh_detail = (f"Laptop refresh capped at {target:g} Hz" if target is not None
                              else f"Laptop refresh is {current_refresh:g} Hz (already at or below 60 Hz)"
                              if refresh_available else "No supported mode at or below 60 Hz")
            controls = {
                "profile": {"available": bool(available["profile"]), "detail": "Power saver"},
                "turbo": {"available": bool(available["turbo"]), "detail": "CPU turbo off"},
                "epp": {"available": bool(available["epp"]), "detail": "Energy-saving CPU preference"},
                "brightness": {"available": before is not None,
                               "detail": "Laptop brightness capped at 40%" if before is not None else "Brightness cannot be read"},
                "refresh": {"available": refresh_available, "detail": refresh_detail},
            }
            saved = {
                "on": True, "complete": False, "reduceCpu": bool(reduce_cpu),
                "source": active_side(cfg), "profile": profile,
                "brightness": {"monitor": internal, "before": before} if before is not None else None,
                "monitor": saved_mon,
                "cpu": {"turbo": turbo, "epp": preferences},
                "pending": (["brightness"] if before is not None else [])
                           + (["refresh"] if target else []),
                "controls": controls, "error": "",
            }
            if reduce_cpu:
                if profile:
                    saved["pending"].append("profile")
                if saved["cpu"]["turbo"] is not None:
                    saved["pending"].append("turbo")
                saved["pending"].extend(f"epp:{item['cpu']}" for item in saved["cpu"]["epp"])
            try:
                atomic_state(path, saved)
            except OSError as error:
                return f"Could not save Travel restore point: {error}"
            if reduce_cpu:
                try:
                    err = _apply_cpu_overlay(saved)
                except OSError as error:
                    err = f"Could not save CPU restore progress: {error}"
                if err:
                    saved["error"] = err
                    try:
                        restore_error = _restore_cpu(saved, path, rollback=True, cfg=cfg)
                    except OSError as error:
                        restore_error = f"Could not save CPU restoration progress: {error}"
                    try:
                        display_error = _restore_travel_display(saved, path)
                    except OSError as error:
                        display_error = f"Could not save display restoration progress: {error}"
                    failure = restore_error or display_error
                    if failure:
                        saved["on"], saved["complete"] = True, False
                        saved["error"] += f"; restoration incomplete: {failure}"
                        atomic_state(path, saved)
                    else:
                        atomic_state(path, {
                            "on": False, "complete": True, "reduceCpu": False,
                            "pending": [], "error": err,
                        })
                    return saved["error"]
            err = ""
            if "brightness" in saved["pending"]:
                cap = min(before, 40)
                err = brightness_set(internal, cap)
                actual = brightness_get(internal)
                if not err and (actual is None or abs(actual - cap) > 1):
                    err = "Brightness cap could not be verified"
            if not err and target:
                err = _monitor_change(saved_mon, target)
            if err:
                saved["error"] = err
            if saved["error"]:
                try:
                    restore_error = _restore_travel_display(saved, path)
                except OSError as error:
                    restore_error = f"Could not save display restoration progress: {error}"
                try:
                    cpu_error = _restore_cpu(saved, path, rollback=True, cfg=cfg) if reduce_cpu else ""
                except OSError as error:
                    cpu_error = f"Could not save CPU restoration progress: {error}"
                failure = restore_error or cpu_error
                if failure:
                    saved["on"], saved["complete"] = True, False
                    saved["error"] += f"; restoration incomplete: {failure}"
                    atomic_state(path, saved)
                else:
                    atomic_state(path, {
                        "on": False, "complete": True, "reduceCpu": False,
                        "pending": [], "error": saved["error"],
                    })
                return saved["error"]
            saved["on"] = True
            saved["complete"] = True
            atomic_state(path, saved)
            return _apply_travel_tweaks(saved, path)
        if "complete" not in state or "monitors" in state:
            return _restore_legacy_travel(state, cfg, path)
        state["error"] = ""
        state["complete"] = False
        atomic_state(path, state)
        err = _restore_cpu(state, path, cfg=cfg) if state.get("reduceCpu") else ""
        err = err or _restore_travel_tweaks(state, path)
        if err:
            state["error"], state["complete"] = err, False
            atomic_state(path, state)
            return err
        if "refresh" in state.get("pending", []):
            err = _monitor_change(state.get("monitor"), state["monitor"]["refreshRate"])
            if err:
                state["error"], state["complete"] = err, False
                atomic_state(path, state)
                return err
            state["pending"].remove("refresh")
            atomic_state(path, state)
        if "brightness" in state.get("pending", []):
            saved_brightness = state.get("brightness") or {}
            monitor, before = saved_brightness.get("monitor"), saved_brightness.get("before")
            if not display_power.safe_name(monitor) or type(before) is not int or not 0 <= before <= 100:
                state["error"], state["complete"] = "Saved brightness restore point is invalid", False
                atomic_state(path, state)
                return state["error"]
            err = brightness_set(monitor, before)
            actual = brightness_get(monitor)
            if err or actual is None or abs(actual - before) > 1:
                state["error"], state["complete"] = err or "Brightness restoration could not be verified", False
                atomic_state(path, state)
                return state["error"]
            state["pending"].remove("brightness")
            atomic_state(path, state)
        atomic_state(path, {"on": False, "complete": True, "reduceCpu": False,
                            "pending": [], "error": state.get("error", "")})
        return ""

def _restore_legacy_travel(state, cfg, path):
    old_brightness = state.get("brightness")
    old_monitors = state.get("monitors") or []
    if old_brightness is not None:
        internal = laptop_monitor(old_monitors)
        if not internal or type(old_brightness) is not int or not 0 <= old_brightness <= 100:
            return "Legacy Travel brightness restore point is invalid"
        err = brightness_set(internal, old_brightness)
        actual = brightness_get(internal)
        if err or actual is None or abs(actual - old_brightness) > 1:
            state["error"] = err or "Brightness restoration could not be verified"
            atomic_state(path, state)
            return state["error"]
        state["brightness"] = None
        atomic_state(path, state)
    for saved in list(old_monitors):
        if not isinstance(saved, dict) or not display_power.safe_name(saved.get("name")):
            return "Legacy Travel monitor restore point is invalid"
        spec = {
            "name": saved["name"], "output": saved["name"],
            "width": saved.get("width"), "height": saved.get("height"),
            "refreshRate": saved.get("refresh"), "x": saved.get("x"),
            "y": saved.get("y"), "scale": saved.get("scale"),
        }
        err = _monitor_change(spec, spec["refreshRate"])
        if err:
            state["error"] = err
            atomic_state(path, state)
            return err
        state["monitors"].remove(saved)
        atomic_state(path, state)
    side = active_side(cfg)
    err = apply_profile(side, cfg[side]["profile"])
    if err:
        state["error"] = err
        atomic_state(path, state)
        return err
    atomic_state(path, {"on": False, "complete": True, "reduceCpu": False, "pending": []})
    return ""

def _restore_travel_display(state, path):
    error = ""
    if "refresh" in state.get("pending", []):
        error = _monitor_change(state.get("monitor"), state["monitor"]["refreshRate"])
        if not error:
            state["pending"].remove("refresh")
            atomic_state(path, state)
    brightness = state.get("brightness") or {}
    if "brightness" in state.get("pending", []):
        monitor, before = brightness.get("monitor"), brightness.get("before")
        if not display_power.safe_name(monitor) or type(before) is not int or not 0 <= before <= 100:
            error = error or "Saved brightness restore point is invalid"
        else:
            err = brightness_set(monitor, before)
            actual = brightness_get(monitor)
            if err or actual is None or abs(actual - before) > 1:
                error = error or err or "Brightness restoration could not be verified"
            else:
                state["pending"].remove("brightness")
                atomic_state(path, state)
    if error:
        state["on"], state["complete"], state["error"] = True, False, error
        atomic_state(path, state)
    elif not state.get("pending"):
        atomic_state(path, {"on": False, "complete": True, "reduceCpu": False, "pending": []})
    return error
def _control_report(saved, fallback, detail):
    if isinstance(saved, dict):
        available, label = saved.get("available"), saved.get("detail")
        if type(available) is bool:
            return {"available": available,
                    "detail": label if isinstance(label, str) else detail}
    return {"available": bool(fallback), "detail": detail}

def travel_status():
    state = travel_state()
    active = bool(state.get("on"))
    current = monitors()
    internal = next((m for m in current if m.get("name") == laptop_monitor(current)), None)
    cpu = _cpu_available()
    try:
        current_refresh = float(internal.get("refreshRate")) if internal else None
    except (TypeError, ValueError):
        current_refresh = None
    target_refresh = _target_refresh(internal) if internal else None
    refresh_available = bool(internal and (target_refresh is not None or
                                           current_refresh is not None and current_refresh <= 60))
    refresh_detail = (f"Laptop refresh capped at {target_refresh:g} Hz" if target_refresh is not None
                      else f"Laptop refresh is {current_refresh:g} Hz (already at or below 60 Hz)"
                      if refresh_available else "No supported mode at or below 60 Hz")
    frozen = (state.get("controls") or {}) if active else {}
    brightness = bool(internal and brightness_get(internal["name"]) is not None)
    reduce_cpu = bool(state.get("reduceCpu", "complete" not in state)) if active else False
    phase = ("on" if state.get("complete") else "incomplete") if active else "off"
    return {
        "phase": phase,
        "reduceCpu": reduce_cpu,
        "error": state.get("error", "") if isinstance(state.get("error", ""), str)
            else "Travel restore state is malformed",
        "controls": {
            "profile": _control_report(frozen.get("profile"), cpu["profile"], "Power saver"),
            "turbo": _control_report(frozen.get("turbo"), cpu["turbo"], "CPU turbo off"),
            "epp": _control_report(frozen.get("epp"), cpu["epp"], "Energy-saving CPU preference"),
            "brightness": _control_report(frozen.get("brightness"), brightness,
                "Laptop brightness capped at 40%" if brightness else "Brightness cannot be read"),
            "refresh": _control_report(frozen.get("refresh"), refresh_available, refresh_detail),
        },
    }

def quick_dim_status():
    state = quick_dim_state()
    monitor = state.get("monitor") if state.get("on") else laptop_monitor(monitors())
    value = brightness_get(monitor) if monitor else None
    return {"on": bool(state.get("on")), "available": state.get("on", False) or value is not None,
            "brightness": value, "error": state.get("error", "")}

def write_lid_block(action):
    original = BINDINGS.read_text() if BINDINGS.exists() else ""
    text = original
    pattern = re.compile(re.escape(BEGIN) + r".*?" + re.escape(END) + r"\n?", re.S)
    text = pattern.sub("", text)
    if action != "system-default":
        exe = pathlib.Path(__file__).resolve()
        block = (
            f"{BEGIN}\n"
            'hl.unbind("switch:on:Lid Switch")\n'
            'hl.unbind("switch:off:Lid Switch")\n'
            'o.bind("switch:on:Lid Switch", nil, '
            f'"python3 {exe} lid-closed", {{ locked = true }})\n'
            'o.bind("switch:off:Lid Switch", nil, '
            f'"python3 {exe} lid-opened", {{ locked = true }})\n'
            f"{END}\n"
        )
        if not text.endswith("\n"):
            text += "\n"
        text += block
    if text != original:
        BINDINGS.write_text(text)
        run(["hyprctl", "reload"])

def lid_closed():
    priv(["hid-wakeup", "off"])
    try:
        devices = json.loads(run(["hyprctl", "devices", "-j"], timeout=2)[1] or "{}")
    except json.JSONDecodeError:
        devices = {}
    idle_wake.set_mice_enabled(False, devices, run)
    idle_wake.set_keyboards_enabled(False, devices, run)
    cfg = load()
    action = cfg[active_side()]["lid"]
    if action == "nothing":
        return 0
    if action == "display-off":
        with display_power.state_lock():
            set_displays("off", disable=False)
        kblight.sync("off", priv_bin=helper_bin())
        return 0
    if action == "suspend":
        run(["systemctl", "suspend"], timeout=8)
        return 0
    if action == "hibernate":
        code, _ = run(["systemctl", "hibernate"], timeout=8)
        if code != 0:
            run(["systemctl", "suspend"], timeout=8)
        return 0
    return 0

def lid_opened():
    priv(["hid-wakeup", "on"])
    try:
        devices = json.loads(run(["hyprctl", "devices", "-j"], timeout=2)[1] or "{}")
    except json.JSONDecodeError:
        devices = {}
    idle_wake.set_mice_enabled(True, devices, run)
    idle_wake.set_keyboards_enabled(True, devices, run)
    # Key presses re-enable DPMS but not heads that Displays off disabled.
    with display_power.state_lock():
        if (display_power.load_state().get("disabled")
                or display_power.blanked(display_power.default_monitors())):
            set_displays("on")
            kblight.sync("on", priv_bin=helper_bin())
    return 0

def tweak_state():
    try:
        return json.loads(TWEAKS.read_text())
    except (OSError, json.JSONDecodeError):
        return {}

def _hypr_option(name):
    code, out = run(["hyprctl", "getoption", name])
    first = out.split("\n", 1)[0].split(":")[-1].strip() if code == 0 else ""
    return {"true": True, "1": True, "false": False, "0": False}.get(first)

def tweak_on(key):
    """Live state of a saving tweak, or None when this machine cannot do it."""
    if key == "abm":
        paths = sorted(DRM_ROOT.glob("*/amdgpu/panel_power_savings"))
        return int(paths[0].read_text()) > 0 if paths else None
    values = [_hypr_option(name) for name in EFFECT_OPTIONS]
    return None if None in values else not any(values)

def _set_effects(on):
    state = tweak_state()
    if on:
        state.setdefault("effectsBefore", {name: _hypr_option(name) for name in EFFECT_OPTIONS})
    before = {} if on else state.pop("effectsBefore", {})
    errors = []
    for name, lua in EFFECT_OPTIONS.items():
        enabled = False if on else before.get(name, True) is not False
        code, out = run(display_power.eval_argv(
            "hl.config({ " + lua.format("true" if enabled else "false") + " })"))
        if code:
            errors.append(out.strip() or f"{name} could not be changed")
    TWEAKS.parent.mkdir(parents=True, exist_ok=True)
    TWEAKS.write_text(json.dumps(state, indent=2) + "\n")
    return " ".join(errors)

def _set_tweak(key, on):
    return _set_effects(on) if key == "effects" else priv([key, "on" if on else "off"])

def travel_tweak_targets():
    return [key for key in TRAVEL_TWEAKS if tweak_on(key) is False]

def _apply_travel_tweaks(state, path):
    # A missing extra saving never fails Travel; only what was turned on is restored.
    for key in travel_tweak_targets():
        state.setdefault("tweaks", []).append(key)
        state["pending"].append(f"tweak:{key}")
        atomic_state(path, state)
        if _set_tweak(key, True):
            state["tweaks"].remove(key)
            state["pending"].remove(f"tweak:{key}")
            atomic_state(path, state)
    return ""

def _restore_travel_tweaks(state, path):
    for key in state.get("tweaks", []):
        token = f"tweak:{key}"
        if token not in state["pending"]:
            continue
        err = _set_tweak(key, False)
        if err:
            return err
        state["pending"].remove(token)
        atomic_state(path, state)
    return ""

def apply_tweak(key, value, persist=False):
    if key == "turbo":
        with energy_lock():
            state = travel_state()
            if state.get("on") and state.get("reduceCpu", True):
                return "Turn off Travel mode before changing CPU settings"
            return _apply_tweak(key, value, persist)
    return _apply_tweak(key, value, persist)

def _apply_tweak(key, value, persist=False):
    err = _set_tweak(key, value)
    state = tweak_state()
    state[key] = bool(value)
    if persist:
        state[key + "Pinned"] = True
    TWEAKS.parent.mkdir(parents=True, exist_ok=True)
    TWEAKS.write_text(json.dumps(state, indent=2) + "\n")
    if persist:
        unit_dir = HOME / ".config/systemd/user"
        unit_dir.mkdir(parents=True, exist_ok=True)
        unit = unit_dir / "omarchy-power-hub-tweaks.service"
        unit.write_text(
            "[Unit]\nDescription=Omarchy Power Hub device tweaks\n\n"
            "[Service]\nType=oneshot\n"
            f"ExecStart={sys.executable} {pathlib.Path(__file__).resolve()} apply-tweaks\n\n"
            "[Install]\nWantedBy=default.target\n"
        )
        run(["systemctl", "--user", "enable", "--now", "omarchy-power-hub-tweaks.service"], timeout=8)
    return err

def apply_saved_tweaks():
    state = tweak_state()
    travel = travel_state()
    cpu_owned = travel.get("on") and travel.get("reduceCpu", True)
    err = ""
    for key in ("wifi", "pcie", "usb", "audio", "abm", "effects", "turbo"):
        if key == "turbo" and cpu_owned:
            continue
        if state.get(key + "Pinned") or key in state and state.get(key + "Pinned", False):
            err = apply_tweak(key, bool(state.get(key)), persist=False) or err
    if not cpu_owned and state.get("eppPinned") and state.get("epp"):
        with energy_lock():
            travel = travel_state()
            if not (travel.get("on") and travel.get("reduceCpu", True)):
                err = priv(["epp", state["epp"]]) or err
    return err

def apply_active(cfg=None):
    with energy_lock():
        cfg = cfg or load()
        state = travel_state()
        cpu_owned = state.get("on") and state.get("reduceCpu", True)
        side = active_side(cfg)
        src = cfg[side]
        err = ""
        if cpu_owned:
            if not state.get("complete"):
                err = state.get("error") or "Travel mode restoration is incomplete"
            else:
                if not state.get("controls", {}).get("profile", {}).get("available"):
                    profiles = listed_profiles() or set()
                    normal_profile = canonical_profile(src["profile"], side, profiles)
                    if normal_profile in profiles:
                        err = apply_profile(side, normal_profile)
                if not err:
                    try:
                        err = _apply_cpu_overlay(state)
                    except OSError as error:
                        err = f"Could not save CPU overlay progress: {error}"
                if err:
                    state["complete"], state["error"] = False, err
                    atomic_state(travel_path(), state)
        profile_error = apply_profile(side, src["profile"]) if not cpu_owned else ""
        source_error = shell_idle(effective_saver(src), src["lock"])
        if int(src.get("hibernateAfter") or 0) > 0:
            source_error = (source_error + " "
                            + apply_hibernate_delay(int(src["hibernateAfter"]))).strip()
        write_lid_block(src["lid"])
        return (err + " " + profile_error + " " + source_error).strip()

def status():
    cfg = load()
    side = active_side()
    ch = maybe_reapply_framework_limit(cfg, charge_status())
    hib = hibernate_available()
    return {
        "side": side,
        "config": cfg,
        "travel": travel_status(),
        "quickDim": quick_dim_status(),
        "charge": ch,
        "boost": bool(cfg.get("boost")),
        "hibernate": hib,
        "tweaks": tweak_state(),
        "framework": framework(),
    }

def set_key(side, key, value):
    if side not in ("ac", "battery"):
        return "bad source"
    cfg = load()
    src = cfg[side]
    if key in ("screensaver", "displaysOff", "lock", "sleep", "hibernateAfter"):
        src[key] = max(0, int(value))
    elif key == "lid":
        if value not in LID_ACTIONS:
            return "bad lid action"
        src[key] = value
    elif key == "profile":
        if value not in PROFILE_ALIASES:
            return "bad profile"
        src[key] = canonical_profile(value, side)
    else:
        return "bad key"
    if key == "profile":
        with energy_lock():
            travel = travel_state()
            cpu_owned = travel.get("on") and travel.get("reduceCpu", True)
            save(cfg)
            # Travel holds Omarchy's remembered profiles; restore writes this one back.
            return "" if cpu_owned else apply_profile(side, src["profile"])
    save(cfg)
    travel_on = travel_state().get("on")
    if travel_on or side != active_side():
        return ""
    if key in ("screensaver", "displaysOff", "lock"):
        return shell_idle(effective_saver(src), src["lock"])
    if key == "hibernateAfter" and src[key] > 0:
        return apply_hibernate_delay(src[key])
    if key == "lid":
        write_lid_block(src["lid"])
    return ""

# Polls only run while blanked or parked, so an older cursor is from the last
# blank and would read as motion, waking the displays a second after they go off.
def idle_cursor_prev(cursor):
    state_path = RUN / "idle-wake.json"
    try:
        fresh = time.time() - state_path.stat().st_mtime < CURSOR_FRESH_SECONDS
        prev = json.loads(state_path.read_text()).get("cursor") if fresh else None
    except (OSError, json.JSONDecodeError):
        prev = None
    RUN.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps({"cursor": cursor}) + "\n")
    return prev

# Closing the saver for Displays off reads to Omarchy as the user returning,
# which cancels its lock countdown, so the plugin keeps that deadline itself.
def idle_lock():
    if (HOME / ".local/state/omarchy/indicators/stay-awake").exists():
        return 0
    run(["omarchy-system-lock"], timeout=8)
    return 0

# Omarchy's idle timer honors Wayland idle inhibitors (a playing video in a
# browser holds one), so it never starts the screensaver. Only step in then;
# otherwise Omarchy launches it and a second launch would race the first.
def saver_past_inhibitor():
    try:
        clients = json.loads(run(["hyprctl", "clients", "-j"], timeout=2)[1] or "[]")
    except json.JSONDecodeError:
        clients = []
    if not any(c.get("inhibitingIdle") for c in clients):
        return 0
    if (HOME / ".local/state/omarchy/indicators/stay-awake").exists():
        return 0
    if run(["omarchy-shell", "lock", "isLocked"])[1].strip() == "true":
        return 0
    run(["omarchy-launch-screensaver"], timeout=20)
    return 0

def main(argv):
    cmd = argv[1] if len(argv) > 1 else "status"
    if cmd == "status":
        print(json.dumps(status()))
        return 0
    if cmd == "config":
        print(json.dumps({"side": active_side(), "config": load()}))
        return 0
    if cmd == "drain-to":
        print(drain_to())
        return 0
    if cmd == "recover-ucsi":
        recover_ucsi()
        return 0
    if cmd == "battery-history":
        print(json.dumps(battery_history()))
        return 0
    if cmd == "gpu":
        print(json.dumps(gpu_status()))
        return 0
    if cmd == "helper-install":
        code, out = run(["pkexec", str(PRIV), "helper-install"], timeout=120)
        print("ok" if code == 0 else out.strip() or "helper install failed")
        return code
    if cmd == "helper-needed":
        print("needed" if helper_needed() else "ok")
        return 0
    if cmd == "ac":
        print(0 if on_battery() else 1)
        return 0
    if cmd == "apply":
        err = (apply_active() + " " + apply_saved_tweaks()).strip()
        if idle_wake.lid_is_open(idle_wake.acpi_lid_text()):
            err = (err + " " + priv(["hid-wakeup", "on"])).strip()
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "apply-profile":
        err = apply_active()
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "led-sudo-needed":
        if not framework() or not kblight.power_led():
            print("skip")
            return 0
        code, out = run(["sudo", "-n", "-l"], timeout=4)
        if code == 0 and "chromeos" in out and "/usr/bin/tee" in out:
            print("ok")
            return 0
        print("needed")
        return 0
    if cmd == "apply-tweaks":
        err = apply_saved_tweaks()
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "profile" and len(argv) >= 3:
        cfg = load()
        side = active_side()
        err = set_key(side, "profile", argv[2])
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "unpin" and len(argv) >= 3:
        state = tweak_state()
        state.pop(argv[2] + "Pinned", None)
        TWEAKS.write_text(json.dumps(state, indent=2) + "\n")
        print("ok")
        return 0
    if cmd == "set" and len(argv) >= 5:
        err = set_key(argv[2], argv[3], argv[4])
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "health" and len(argv) >= 3:
        err = set_health(argv[2] in {"1", "on", "true"})
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "charge" and len(argv) >= 3:
        cfg = load()
        err = set_charge(argv[2])
        if not err:
            cfg["chargeLimit"] = int(argv[2])
            if not cfg.get("boost"):
                cfg["boostSavedLimit"] = int(argv[2])
            save(cfg)
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "boost" and len(argv) >= 3:
        cfg = load()
        on = argv[2] in {"1", "on", "true"}
        if on:
            current = charge_status().get("limit") or cfg.get("chargeLimit") or 80
            cfg["boostSavedLimit"] = current
            cfg["boost"] = True
            save(cfg)
            err = set_charge(100)
        else:
            cfg["boost"] = False
            save(cfg)
            err = set_charge(cfg.get("boostSavedLimit") or cfg.get("chargeLimit") or 80)
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "boost-check":
        cfg = load()
        if not cfg.get("boost"):
            print("ok")
            return 0
        code, out = run(["upower", "-i", "/org/freedesktop/UPower/devices/DisplayDevice"], timeout=4)
        full = "fully-charged" in out or "fully charged" in out.lower()
        pct = re.search(r"percentage:\s+(\d+)", out)
        ac = "online" in out and "yes" in out.lower().split("online", 1)[-1][:12]
        if full or (pct and int(pct.group(1)) >= 100 and not on_battery()):
            cfg["boost"] = False
            save(cfg)
            err = set_charge(cfg.get("boostSavedLimit") or 80)
            print(err or "restored")
            return 0 if not err else 1
        print("holding")
        return 0
    if cmd == "travel":
        if len(argv) != 3 or argv[2] not in {"on", "off"}:
            print("usage: travel on|off")
            return 2
        try:
            err = travel(argv[2] == "on")
        except OSError as error:
            err = f"Travel operation could not save its recovery state: {error}"
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "quick-dim":
        if len(argv) != 3 or argv[2] not in {"on", "off"}:
            print("usage: quick-dim on|off")
            return 2
        try:
            err = quick_dim(argv[2] == "on")
        except OSError as error:
            err = f"Quick dim could not save its recovery state: {error}"
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "tweak" and len(argv) >= 4:
        persist = len(argv) > 4 and argv[4] in {"1", "persist", "pin"}
        err = apply_tweak(argv[2], argv[3] in {"1", "on", "true"}, persist)
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "cpu" and len(argv) >= 4:
        with energy_lock():
            state = travel_state()
            if state.get("on") and state.get("reduceCpu", True):
                err = "Turn off Travel mode before changing CPU settings"
            else:
                err = priv([argv[2], argv[3]])
                if not err and argv[2] == "epp":
                    prefs = tweak_state()
                    prefs["epp"] = argv[3]
                    prefs["eppPinned"] = True
                    TWEAKS.write_text(json.dumps(prefs, indent=2) + "\n")
        print(err or "ok")
        return 0 if not err else 1
    if cmd in {"displays", "dpms"} and len(argv) >= 3:
        with display_power.state_lock():
            if argv[2] == "park" and len(argv) >= 4:
                if not idle_wake.lid_is_open(idle_wake.acpi_lid_text()):
                    print("skip")
                    return 0
                print("parked" if display_power.park(argv[3]) else "skip")
                return 0
            if argv[2] == "sync":
                print(display_power.sync())
                return 0
            if argv[2] in {"off", "disable", "on"}:
                idle_for_blank(argv[2] in {"off", "disable"})
            err = set_displays(argv[2])
        kblight.sync(argv[2], priv_bin=helper_bin())
        print(err or "ok")
        return 0 if not err else 1
    if cmd == "lid-closed":
        return lid_closed()
    if cmd == "lid-opened":
        return lid_opened()
    if cmd == "idle-lock":
        return idle_lock()
    if cmd == "saver-past-inhibitor":
        return saver_past_inhibitor()
    if cmd == "idle-wake-poll":
        blanked = len(argv) > 2 and argv[2] in {"blanked", "off"}
        cursor = run(["hyprctl", "cursorpos"], timeout=2)[1].strip()
        mons = display_power.default_monitors()
        try:
            hid = json.loads(run(["hyprctl", "devices", "-j"], timeout=2)[1] or "{}")
        except json.JSONDecodeError:
            hid = {}
        lid_text = idle_wake.acpi_lid_text()
        lid_open = idle_wake.lid_is_open(lid_text)
        result = idle_wake.poll(
            lid_text=lid_text,
            cursor=cursor,
            prev_cursor=idle_cursor_prev(cursor),
            monitors=mons,
            blanked=blanked,
        )
        enabled = result == "wake" or idle_wake.pointers_enabled(lid_open=lid_open, blanked=blanked)
        idle_wake.set_mice_enabled(enabled, hid, run)
        idle_wake.set_keyboards_enabled(enabled, hid, run)
        print(result)
        return 0
    print("usage", file=sys.stderr)
    return 2

if __name__ == "__main__":
    sys.exit(main(sys.argv))
