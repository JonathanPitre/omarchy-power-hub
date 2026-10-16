"""USB/i2c HID wakeup nodes for suspend (pointer vs keyboard)."""
import pathlib
import sys

SYS = pathlib.Path("/sys")
LOGITECH = "046d"


def _text(path):
    try:
        return path.read_text().strip()
    except OSError:
        return ""


def _usb_devs(root):
    return [p for p in (root / "bus" / "usb" / "devices").glob("*") if p.is_dir()]


def _interfaces(dev):
    rows = []
    for iface in dev.glob("*"):
        cls = iface / "bInterfaceClass"
        if not cls.exists():
            continue
        proto = _text(iface / "bInterfaceProtocol")
        rows.append((_text(cls), proto))
    return rows


def _is_logitech(dev):
    return _text(dev / "idVendor").lower() == LOGITECH


def _is_hid(dev):
    return any(cls == "03" for cls, _proto in _interfaces(dev))


def _i2c_touchpad_wakeup(root):
    out = []
    for node in (root / "bus" / "i2c" / "devices").glob("*/power/wakeup"):
        name = _text(node.parent.parent / "name").lower()
        ident = node.parent.parent.name.lower()
        blob = name + ident
        if "pixa" in blob or "touchpad" in blob or "mouse" in blob or "keyboard" in blob:
            out.append(node)
    return out


def wakeup_nodes(root=None):
    root = SYS if root is None else root
    nodes = []
    for dev in _usb_devs(root):
        w = dev / "power" / "wakeup"
        if not w.exists():
            continue
        if _is_logitech(dev) or _is_hid(dev):
            nodes.append(w)
    nodes.extend(_i2c_touchpad_wakeup(root))
    return nodes


def apply(mode, root=None):
    root = SYS if root is None else root
    want = "enabled" if mode == "on" else "disabled"
    for node in wakeup_nodes(root):
        try:
            node.write_text(want + "\n")
        except OSError:
            pass


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else ""
    if mode not in ("on", "off"):
        sys.exit(2)
    apply(mode)
    print("ok")
