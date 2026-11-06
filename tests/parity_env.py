"""Sandbox for comparing bin/ops.py (reference) against the Rust binary on the same fake system."""
import json
import os
import pathlib
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from dataclasses import dataclass, field

REPO = pathlib.Path(__file__).resolve().parent.parent
RUST_BIN = REPO / "target" / "debug" / "power-hub"
SYSTEM_PATHS = r"(?<![\w/.])/(?:sys|proc|run|var|etc|usr/(?!bin\b|local/bin\b))"
STUBBED = ("hyprctl", "busctl", "iw", "systemctl", "sudo", "pkexec", "upower", "omarchy", "powerprofilesctl",
           "omarchy-powerprofiles-list", "omarchy-powerprofiles-set", "omarchy-system-lock",
           "omarchy-shell", "omarchy-launch-screensaver", "pkill")

_case = unittest.TestCase()


def build_rust():
    if not shutil.which("cargo"):
        raise RuntimeError("cargo is required to run the parity tests")
    p = subprocess.run(["cargo", "build", "--features", "fixture-root"], cwd=REPO, text=True, capture_output=True)
    if p.returncode:
        raise RuntimeError(f"cargo build failed:\n{p.stderr}")


@dataclass
class Result:
    code: int
    stdout: str
    calls: list = field(default_factory=list)
    calls_text: str = ""
    evals: list = field(default_factory=list)
    requests: list = field(default_factory=list)


def assert_same(a, b):
    try:
        left, right = json.loads(a.stdout), json.loads(b.stdout)
    except ValueError:
        left, right = a.stdout, b.stdout
    _case.assertEqual(left, right, "stdout differs")
    _case.assertEqual(a.code, b.code, "exit code differs")
    _case.assertEqual(a.evals, b.evals, "hyprland evals differ")


HYPRCTL = """#!{python}
import socket, sys
args = sys.argv[1:]
with open({calls!r}, "a") as f:
    f.write("hyprctl " + " ".join(args) + "\\n")
if args[:1] == ["eval"]:
    req = "eval " + " ".join(args[1:])
elif args[-1:] == ["-j"]:
    req = "j/" + " ".join(args[:-1])
else:
    req = " ".join(args)
s = socket.socket(socket.AF_UNIX)
s.connect({sock!r})
s.sendall(req.encode())
s.shutdown(socket.SHUT_WR)
out = b""
while chunk := s.recv(65536):
    out += chunk
sys.stdout.write(out.decode())
"""


class Sandbox:
    def __init__(self):
        self.base = pathlib.Path(tempfile.mkdtemp(prefix="ph-", dir="/tmp"))
        self.root, self.home, self.run, self.stubs, self.bin = (self.base / d for d in ("root", "home", "run", "stubs", "bin"))
        for d in (self.root, self.home, self.run, self.stubs, self.bin):
            d.mkdir()
        self.calls_log = self.base / "calls.log"
        self.hypr = {}
        self._evals = []
        self._requests = []
        self._rules = {}
        self._install_scripts()
        for tool in STUBBED:
            self._write_stub(tool)
        self._serve()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self._server.shutdown(socket.SHUT_RDWR)
        self._server.close()
        self._thread.join(timeout=2)
        shutil.rmtree(self.base, ignore_errors=True)

    def _install_scripts(self):
        src = REPO / "bin"
        for name in ("ops.py", "display_power.py", "idle_wake.py", "kblight.py"):
            (self.bin / name).write_text(self._rewrite(name, (src / name).read_text()))
        (self.bin / "battery-plus-data").write_text(self._rewrite("battery-plus-data", (src / "battery-plus-data").read_text()))
        (self.bin / "battery-plus-data").chmod(0o755)
        for name in ("ops-priv", "hid_wakeup.py"):
            shutil.copy2(src / name, self.bin / name)

    def _rewrite(self, name, text):
        prefix = str(self.root)
        lines = text.split("\n")
        for i, line in enumerate(lines):
            if i == 0 and line.startswith("#!"):
                continue
            if line.startswith("PATH="):
                lines[i] = f"PATH={self.stubs}:/usr/bin:/bin"
            elif name.endswith(".py"):
                lines[i] = re.sub(r"(?<=[\"'])/(?=(?:sys|proc|run|var|etc|usr/(?!bin/))\b)", prefix + "/", line)
            else:
                lines[i] = re.sub(SYSTEM_PATHS, lambda m: prefix + m.group(0), line)
        return "\n".join(lines)

    def _serve(self):
        sock_dir = self.run / "hypr" / "test"
        sock_dir.mkdir(parents=True)
        self.sock_path = str(sock_dir / ".socket.sock")
        self._server = socket.socket(socket.AF_UNIX)
        self._server.bind(self.sock_path)
        self._server.listen(8)
        self._thread = threading.Thread(target=self._loop, daemon=True)
        self._thread.start()
        hyprctl = self.stubs / "hyprctl"
        hyprctl.write_text(HYPRCTL.format(python=sys.executable, calls=str(self.calls_log), sock=self.sock_path))
        hyprctl.chmod(0o755)

    def _loop(self):
        while True:
            try:
                conn, _ = self._server.accept()
            except OSError:
                return
            with conn:
                try:
                    req = conn.recv(65536).decode()
                    self._requests.append(req)
                    if req.startswith("eval "):
                        self._evals.append(req[5:])
                        reply = "ok"
                    else:
                        reply = self.hypr.get(req, "")
                    conn.sendall(reply.encode())
                except OSError:
                    pass

    def _target(self, path):
        path = str(path)
        if path.startswith("~/"):
            return self.home / path[2:]
        for prefix, base in (("home/", self.home), ("run/", self.run)):
            if path.startswith(prefix):
                return base / path[len(prefix):]
        return self.root / path.lstrip("/")

    def write(self, path, text):
        p = self._target(path)
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)
        return p

    def _write_stub(self, tool):
        cases = "".join(
            f"  *{shlex.quote(m)}*) printf %s {shlex.quote(out)}; exit {code};;\n" for m, code, out in self._rules.get(tool, [])
        )
        # The real list tool ends in a pipeline (tac), so it exits 0 with no output when powerprofilesctl fails.
        fallback = 0 if tool == "omarchy-powerprofiles-list" else 1
        script = f'#!/bin/bash\necho "{tool} $*" >> {shlex.quote(str(self.calls_log))}\ncase "$*" in\n{cases}esac\nexit {fallback}\n'
        p = self.stubs / tool
        p.write_text(script)
        p.chmod(0o755)

    def stub(self, tool, match, code=0, stdout=""):
        # The case statement takes the first match, so a repeated match replaces its earlier rule.
        rules = [r for r in self._rules.get(tool, []) if r[0] != match]
        self._rules[tool] = [*rules, (match, code, stdout)]
        self._write_stub(tool)

    def pci(self, addr, cls="0x030000", boot_vga=None, runtime="active", driver=None):
        sysfs = self.root / "sys" / "bus" / "pci"
        dev = sysfs / "devices" / addr
        (dev / "power").mkdir(parents=True)
        if driver:
            (sysfs / "drivers" / driver).mkdir(parents=True, exist_ok=True)
            (dev / "driver").symlink_to(sysfs / "drivers" / driver)
        (dev / "class").write_text(cls + "\n")
        (dev / "power" / "runtime_status").write_text(runtime + "\n")
        if boot_vga is not None:
            (dev / "boot_vga").write_text(boot_vga + "\n")

    def discharging(self, percent=70, status="Discharging", name="BAT0"):
        bat = self.root / "sys/class/power_supply" / name
        bat.mkdir(parents=True, exist_ok=True)
        (bat / "status").write_text(status + "\n")
        (bat / "capacity").write_text(f"{percent}\n")

    def battery(self, name="BAT0", charge_units=False, **fields):
        """A pack with sysfs files in energy units (or charge units); a None field removes its file."""
        units = ({"charge_now": 3500000, "charge_full": 4200000, "charge_full_design": 4500000, "current_now": 1000000}
                 if charge_units else
                 {"energy_now": 40000000, "energy_full": 50000000, "energy_full_design": 55000000, "power_now": 10000000})
        files = {"status": "Discharging", "capacity": 70, "voltage_now": 12000000, "voltage_min_design": 11400000,
                 "cycle_count": 123, **units, **fields}
        bat = self.root / "sys/class/power_supply" / name
        bat.mkdir(parents=True, exist_ok=True)
        for key, value in files.items():
            if value is None:
                (bat / key).unlink(missing_ok=True)
            else:
                (bat / key).write_text(f"{value}\n")

    def ucsi(self, name, online, volts, amps):
        port = self.root / "sys/class/power_supply" / f"ucsi-source-psy-{name}"
        port.mkdir(parents=True, exist_ok=True)
        (port / "online").write_text(f"{int(online)}\n")
        (port / "voltage_now").write_text(f"{int(volts * 1e6)}\n")
        (port / "current_now").write_text(f"{int(amps * 1e6)}\n")

    def framework(self, tool=True):
        self.write("/sys/class/dmi/id/sys_vendor", "Framework\n")
        if tool:
            self._write_stub("framework_tool")

    @property
    def helper_dir(self):
        return self.root / "usr/local/libexec/omarchy-power-hub"

    def install_helper(self, tamper=False):
        """Root-owned helper copies at the fake-root path; `tamper` makes ops-priv differ from the plugin's."""
        self.helper_dir.mkdir(parents=True, exist_ok=True)
        for name in ("ops-priv", "hid_wakeup.py"):
            shutil.copy2(self.bin / name, self.helper_dir / name)
        if tamper:
            with open(self.helper_dir / "ops-priv", "a") as f:
                f.write("# tampered\n")

    def charge_limit(self, percent):
        """The EC's current limit as the helper reports it, for the read and for any write."""
        self.stub("sudo", "charge-limit", 0, f"Minimum 0%, Maximum {percent}%\n")

    def monitors(self, *rows):
        self.hypr["j/monitors"] = json.dumps(list(rows))

    CONFIG_PATH = "home/.config/omarchy/omarchy-power-hub.json"

    def config(self, **keys):
        """Saved config with a valid profile on both sides, so loading never asks power-profiles-daemon."""
        saved = {"ac": {"profile": "balanced"}, "battery": {"profile": "balanced"}, **keys}
        self.write(self.CONFIG_PATH, json.dumps(saved))

    def read_config(self):
        return json.loads(self._target(self.CONFIG_PATH).read_text())

    def profiles(self, names):
        entries = " ".join(f'1 "Profile" s "{n}" "Driver" s "platform_profile"' for n in names)
        self.stub("busctl", "Profiles", 0, f"aa{{sv}} {len(names)} {entries}\n")
        listing = "".join(f"{n}\n" for n in names)
        self.stub("omarchy-powerprofiles-list", "", 0, listing)
        self.stub("powerprofilesctl", "", 0, listing)

    def _exec(self, argv):
        env = {
            "PATH": f"{self.stubs}:/usr/bin:/bin",
            "HOME": str(self.home),
            "XDG_RUNTIME_DIR": str(self.run),
            "HYPRLAND_INSTANCE_SIGNATURE": "test",
            "POWER_HUB_ROOT": str(self.root),
            "POWER_HUB_BIN": str(self.bin),
            "PYTHONDONTWRITEBYTECODE": "1",
        }
        self.calls_log.write_text("")
        self._evals.clear()
        self._requests.clear()
        p = subprocess.run(argv, env=env, cwd=self.base, text=True, capture_output=True, timeout=30)
        calls = [c for c in self.calls_log.read_text().split("\n") if c]
        return Result(p.returncode, p.stdout, calls, "\n".join(calls), list(self._evals), list(self._requests))

    def python(self, *argv):
        return self._exec([sys.executable, str(self.bin / "ops.py"), *argv])

    def bash_data(self):
        return self._exec(["bash", str(self.bin / "battery-plus-data")])

    def rust(self, *argv):
        return self._exec([str(RUST_BIN), *argv])
