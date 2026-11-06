#!/usr/bin/env python3
"""Time the hot-path verbs of bin/power-hub. Usage: measure-hot-path.py [--runs N]

Runs the real verbs: status and boost-check may re-apply a charge limit, and
idle-wake-poll sends real eval requests (only with lid open and displays on).
Peak memory is not measured: ru_maxrss carries across exec, so it reports Python.
"""
import argparse, glob, json, os, statistics, subprocess, time

BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bin", "power-hub")
VERBS = [["data"], ["status"], ["gpu"], ["boost-check"], ["idle-wake-poll"], ["displays", "sync"]]


def run_once(args):
    t0 = time.perf_counter()
    p = subprocess.Popen([BIN, *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, status, ru = os.wait4(p.pid, 0)
    p.returncode = os.waitstatus_to_exitcode(status)
    if p.returncode:
        raise SystemExit(f"power-hub {' '.join(args)} exited {p.returncode}; aborting")
    return time.perf_counter() - t0, ru.ru_utime + ru.ru_stime


def safe_for_idle_poll():
    """idle-wake-poll sends real eval requests: only with the lid open and every display on."""
    paths = glob.glob("/proc/acpi/button/lid/*/state")
    if not paths:
        return False, "lid state unknown"
    for path in paths:
        try:
            with open(path) as f:
                text = f.read()
        except OSError:
            return False, "lid state unreadable"
        if not ("open" in text and "closed" not in text):
            return False, "lid not open"
    try:
        out = subprocess.run(["hyprctl", "-j", "monitors"], capture_output=True, text=True, timeout=5).stdout
        monitors = json.loads(out)
    except (OSError, ValueError, subprocess.SubprocessError):
        return False, "cannot read displays"
    if not monitors or not all(m.get("dpmsStatus") for m in monitors):
        return False, "displays off"
    return True, ""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=9)
    n = ap.parse_args().runs
    rows = {}
    for verb in VERBS:
        name = " ".join(verb)
        if verb[0] == "idle-wake-poll":
            ok, why = safe_for_idle_poll()
            if not ok:
                print(f"skipping idle-wake-poll: {why}")
                continue
        samples = [run_once(verb) for _ in range(n)]
        rows[name] = tuple(statistics.median(s[i] for s in samples) for i in range(2))
    print(f"{'verb':<16}{'wall ms':>10}{'cpu ms':>10}")
    for name, (wall, cpu) in rows.items():
        print(f"{name:<16}{wall * 1000:>10.1f}{cpu * 1000:>10.1f}")
    if "data" in rows and "gpu" in rows:
        print(f"5 s panel poll (data + gpu): {(rows['data'][1] + rows['gpu'][1]) / 5 * 100:.2f}% of one core")
    if "idle-wake-poll" in rows:
        wall, cpu = rows["idle-wake-poll"]
        print(f"1 Hz idle poll: {cpu * 100:.2f}% client, up to {wall * 100:.2f}% wall (compositor not counted)")
        print("note: measured with displays on; a blanked session may differ")
    print("peak memory is not measured here")


if __name__ == "__main__":
    main()
