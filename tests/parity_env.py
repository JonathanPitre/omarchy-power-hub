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
        script = f'#!/bin/bash\necho "{tool} $*" >> {shlex.quote(str(self.calls_log))}\ncase "$*" in\n{cases}esac\nexit 1\n'
        p = self.stubs / tool
        p.write_text(script)
        p.chmod(0o755)

    def stub(self, tool, match, code=0, stdout=""):
        self._rules.setdefault(tool, []).append((match, code, stdout))
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
        p = subprocess.run(argv, env=env, cwd=self.base, text=True, capture_output=True, timeout=30)
        calls = [c for c in self.calls_log.read_text().split("\n") if c]
        return Result(p.returncode, p.stdout, calls, "\n".join(calls), list(self._evals))

    def python(self, *argv):
        return self._exec([sys.executable, str(self.bin / "ops.py"), *argv])

    def bash_data(self):
        return self._exec(["bash", str(self.bin / "battery-plus-data")])

    def rust(self, *argv):
        return self._exec([str(RUST_BIN), *argv])
