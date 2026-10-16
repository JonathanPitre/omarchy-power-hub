#!/usr/bin/env python3
"""Logitech peripheral batteries straight from the receiver over HID++.

`read` prints one snapshot; `watch` prints one per change while the panel is
open. `solaar show` walks every feature of every device (about 10 s); this
asks each device only for its battery and then listens for the device's own
battery events, which it sends the moment charging starts or stops.
"""
import fcntl, json, os, select, sys, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ops  # noqa: E402

SLOTS = range(1, 7)
# UNIFIED_BATTERY (0x1004) charging byte, numbered as Solaar's BatteryStatus.
CHARGE = {0: "Discharging", 1: "Charging", 2: "Charging", 3: "Full", 4: "Charging"}
# Level flags stand in for a percent on devices that report none.
APPROX = {8: 90, 4: 50, 2: 20, 1: 5}
# DEVICE_NAME (0x0005) getDeviceType.
DEVICE_TYPE = {0: "keyboard", 3: "pointer", 4: "pointer", 5: "pointer"}
LINK_UP_DELAY = 1.0


class Unsupported(Exception):
    """A paired device has no UNIFIED_BATTERY; only `solaar show` can read it."""


def battery(data):
    """{percent, status} from a get_status reply or battery event, or None."""
    if not data or len(data) < 3 or data[0] > 100 or data[2] > 6:
        return None
    pct = data[0] or APPROX.get(data[1])
    if pct is None:
        return None
    return {"percent": pct, "status": CHARGE.get(data[2], "")}


def report(raw, batt):
    """Classify one hidraw input report from the receiver.

    ("battery", slot, {percent, status}) for a device's own battery event,
    ("link", slot, up) for a receiver connection notice, else None. `batt`
    maps slot to that device's UNIFIED_BATTERY feature index. Replies to
    anyone's requests (ours, the Solaar tray's) carry a nonzero software id.
    """
    if len(raw) < 7:
        return None
    rid, slot, sub, fn = raw[0], raw[1], raw[2], raw[3]
    if rid == 0x11 and fn == 0 and sub and batt.get(slot) == sub:
        b = battery(raw[4:8])
        return ("battery", slot, b) if b else None
    if rid == 0x10 and sub == 0x41 and fn:
        return ("link", slot, not raw[4] & 0x40)
    return None


def _feature(req, slot, feature):
    rep = req(slot, 0x0000, feature >> 8, feature & 0xFF)
    return None if not rep else rep[0]


def identify(req, slot):
    """{battery, name, kind} for a paired, online slot, else None.

    Raises Unsupported when the device answers but has no UNIFIED_BATTERY.
    """
    idx = _feature(req, slot, 0x1004)
    if idx is None:
        return None
    if idx == 0:
        raise Unsupported(slot)
    name, kind = "", ""
    fn = _feature(req, slot, 0x0005)
    if fn:
        fn <<= 8
        count = (req(slot, fn) or b"\0")[0]
        raw = b""
        while len(raw) < count:
            chunk = req(slot, fn | 0x10, len(raw))
            if not chunk:
                break
            raw += chunk[:count - len(raw)]
        name = ops.short_name(raw.decode("utf-8", "replace"))
        t = req(slot, fn | 0x20)
        kind = DEVICE_TYPE.get(t[0], "") if t else ""
    # The Solaar tray shares our HID++ software id, so a reply meant for it
    # can land here; asking twice for the battery slot catches that.
    if not name or _feature(req, slot, 0x1004) != idx:
        return None
    return {"battery": idx, "name": name, "kind": kind or ops.device_kind(name)}


def read_slot(req, slot, ident):
    """(identity, row) for one slot; row is None when empty or offline."""
    for attempt in range(2):
        if not ident:
            ident = identify(req, slot)
            if not ident:
                return None, None
        b = battery(req(slot, ident["battery"] << 8 | 0x10))
        if b:
            return ident, {"name": ident["name"], "kind": ident["kind"], **b}
        ident = None
    return None, None


def _identity_file():
    return ops.STATE / "logi.json"


def load_identities():
    try:
        data = json.loads(_identity_file().read_text())
        return {p: {int(s): i for s, i in slots.items()} for p, slots in data.items()}
    except (OSError, ValueError, AttributeError):
        return {}


def save_identities(known):
    try:
        ops.STATE.mkdir(parents=True, exist_ok=True)
        _identity_file().write_text(json.dumps(known))
    except OSError:
        pass


class _Lock:
    """The lock `ops.solaar_devices` holds, so our reads never overlap it."""

    def __enter__(self):
        ops.RUN.mkdir(parents=True, exist_ok=True)
        self.fh = open(ops.RUN / "solaar.lock", "w")
        fcntl.flock(self.fh, fcntl.LOCK_EX)

    def __exit__(self, *exc):
        self.fh.close()


def _solaar_base():
    # Solaar prints warnings (Wayland rules) on stdout while importing.
    real, sys.stdout = sys.stdout, sys.stderr
    try:
        from logitech_receiver import base
        return base
    finally:
        sys.stdout = real


def scan(base, handles, known, slots=SLOTS):
    """Read `slots` on every receiver; updates `known`, returns {(path, slot): row}."""
    rows = {}
    with _Lock():
        for path, h in handles.items():
            ids = known.setdefault(path, {})

            def req(slot, rid, *params, h=h):
                return base.request(h, slot, rid, *params)

            for slot in slots:
                ident, row = read_slot(req, slot, ids.get(slot))
                if ident:
                    ids[slot] = ident
                if row:
                    rows[(path, slot)] = row
    return rows


def _open(base):
    paths = [info.path for info in base.receivers()]
    return {p: base.open_path(p) for p in paths}


def _close(base, handles):
    for h in handles.values():
        try:
            base.close(h)
        except Exception:
            pass


def emit(rows):
    print(json.dumps({"devices": ops.merge_devices(list(rows))}), flush=True)


def read(identify_all=False):
    """Battery rows for every Logitech device, falling back to `solaar show`."""
    try:
        base = _solaar_base()
        handles = _open(base)
    except Exception:
        return ops.solaar_devices()
    try:
        known = {} if identify_all else load_identities()
        rows = scan(base, handles, known)
    except Exception:
        return ops.solaar_devices()
    finally:
        _close(base, handles)
    save_identities(known)
    return ops.merge_devices(list(rows.values()))


def watch(poll=5.0, identify_all=False):
    """Print a snapshot, then a new one whenever a battery or link changes.

    Devices that have not sent a battery event yet are re-read every `poll`
    seconds, one request each, in case they never send one.
    """
    try:
        base = _solaar_base()
        handles = _open(base)
    except Exception:
        emit(ops.solaar_devices())
        return 0
    known = {} if identify_all else load_identities()
    try:
        rows = scan(base, handles, known)
    except Exception:
        _close(base, handles)
        emit(ops.solaar_devices())
        return 0
    save_identities(known)
    emit(rows.values())
    fds = {os.open(p, os.O_RDONLY | os.O_NONBLOCK): p for p in handles}
    heard, due = set(), {}
    next_poll = time.monotonic() + poll
    try:
        while True:
            now = time.monotonic()
            wake = min([next_poll] + list(due.values()))
            ready, _, _ = select.select(list(fds), [], [], max(0.0, wake - now))
            changed = False
            for fd in ready:
                path = fds[fd]
                try:
                    raw = os.read(fd, 64)
                except BlockingIOError:
                    continue
                batt = {s: i["battery"] for s, i in known.get(path, {}).items()}
                ev = report(raw, batt)
                if not ev:
                    continue
                key = (path, ev[1])
                if ev[0] == "battery" and key in rows:
                    heard.add(key)
                    new = {**rows[key], **ev[2]}
                    changed |= new != rows[key]
                    rows[key] = new
                elif ev[0] == "link" and ev[2]:
                    due[key] = time.monotonic() + LINK_UP_DELAY
                elif ev[0] == "link":
                    changed |= rows.pop(key, None) is not None
                    heard.discard(key)
            now = time.monotonic()
            todo = {k for k, t in due.items() if t <= now}
            if now >= next_poll:
                todo |= {k for k in rows if k not in heard}
                next_poll = now + poll
            for key in todo:
                due.pop(key, None)
                path, slot = key
                got = scan(base, {path: handles[path]}, known, [slot])
                new = got.get(key)
                if new != rows.get(key):
                    changed = True
                    if new:
                        rows[key] = new
                    else:
                        rows.pop(key, None)
            if changed:
                emit(rows.values())
    except Exception:  # receiver unplugged, or the panel closed our stdout
        return 1
    finally:
        for fd in fds:
            os.close(fd)
        _close(base, handles)
        save_identities(known)


def main(argv):
    cmd = argv[1] if len(argv) > 1 else "read"
    identify_all = "--identify" in argv
    if cmd == "read":
        print(json.dumps({"devices": read(identify_all)}))
        return 0
    if cmd == "watch":
        poll = 5.0
        if "--poll" in argv:
            try:
                poll = max(1.0, float(argv[argv.index("--poll") + 1]))
            except (IndexError, ValueError):
                pass
        return watch(poll, identify_all)
    print("usage: logi.py read|watch [--poll SECONDS] [--identify]", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
