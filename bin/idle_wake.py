"""Decide whether pointer activity should restore screensaver/DPMS."""
import pathlib
import re

import display_power

ACPI_LID = pathlib.Path("/proc/acpi/button/lid")
SAFE_DEV = re.compile(r"^[A-Za-z0-9:._-]+$")
SKIP_KB = re.compile(r"video-bus|gpio-keys|^power-button$")


def acpi_lid_text(root=None):
    root = ACPI_LID if root is None else root
    for path in sorted(root.glob("*/state")):
        try:
            return path.read_text()
        except OSError:
            continue
    return "state:      open\n"


def lid_is_open(text):
    return "closed" not in str(text or "").lower() and "open" in str(text or "").lower()


def cursor_changed(prev, curr):
    return prev is not None and curr is not None and prev != curr


def restore_from_pointer(*, lid_open, cursor_changed, dpms_on):
    return bool(lid_open and (cursor_changed or dpms_on))


def poll(*, lid_text, cursor, prev_cursor, monitors, blanked):
    if not lid_is_open(lid_text):
        return "closed"
    if restore_from_pointer(
        lid_open=True,
        cursor_changed=cursor_changed(prev_cursor, cursor),
        dpms_on=bool(blanked) and display_power.awake(monitors),
    ):
        return "wake"
    return "blank" if blanked and display_power.partly_lit(monitors) else "hold"


def pointers_enabled(*, lid_open, blanked):
    return bool(lid_open or not blanked)


def mouse_names(devices):
    out = []
    for row in (devices or {}).get("mice") or []:
        name = str(row.get("name") or "")
        if SAFE_DEV.match(name):
            out.append(name)
    return out


def keyboard_names(devices):
    out = []
    for row in (devices or {}).get("keyboards") or []:
        name = str(row.get("name") or "")
        if not SAFE_DEV.match(name) or SKIP_KB.search(name):
            continue
        out.append(name)
    return out


def _set_enabled(names, enabled, run):
    flag = "true" if enabled else "false"
    for name in names:
        run(["hyprctl", "eval", f'hl.device({{ name = "{name}", enabled = {flag} }})'])


def set_mice_enabled(enabled, devices, run):
    _set_enabled(mouse_names(devices), enabled, run)


def set_keyboards_enabled(enabled, devices, run):
    _set_enabled(keyboard_names(devices), enabled, run)
