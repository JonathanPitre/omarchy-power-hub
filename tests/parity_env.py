"""Sandbox for comparing bin/ops.py (reference) against the Rust binary on the same fake system."""
import contextlib
import fcntl
import json
import os
import pathlib
import re
import shlex
import shutil
import stat
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from dataclasses import dataclass, field, replace

REPO = pathlib.Path(__file__).resolve().parent.parent
RUST_BIN = REPO / "target" / "debug" / "power-hub"
SYSTEM_PATHS = r"(?<![\w/.])/(?:sys|proc|run|var|etc|usr/(?!bin\b|local/bin\b))"
STUBBED = ("hyprctl", "busctl", "iw", "systemctl", "sudo", "pkexec", "upower", "omarchy", "powerprofilesctl",
           "omarchy-powerprofiles-list", "omarchy-powerprofiles-set", "omarchy-system-lock",
           "omarchy-shell", "omarchy-launch-screensaver", "omarchy-launch-floating-terminal-with-presentation", "pkill")

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


def assert_same(a, b, stdout=True):
    try:
        left, right = json.loads(a.stdout), json.loads(b.stdout)
    except ValueError:
        left, right = a.stdout, b.stdout
    if stdout:
        _case.assertEqual(left, right, "stdout differs")
    _case.assertEqual(a.code, b.code, "exit code differs")
    _case.assertEqual(a.evals, b.evals, "hyprland evals differ")


def assert_same_files(a, b, normalise=None):
    """The snapshot comparison both() runs after every step; `normalise(path, text)` fixes a file's content first."""
    _case.maxDiff = None
    _case.assertEqual(a.snapshot(normalise), b.snapshot(normalise), "files differ")


DROPPED = ("hyprctl", "busctl get-property net.hadess", "omarchy-powerprofiles-list")


def both(setup, *steps, drop=DROPPED, normalise=None, stdout=True, inspect=None):
    """Run each step through python in one sandbox and rust in another; after every step the exit codes, evals,
    files and recorded calls must match, then `inspect(py, rs)` sees both sandboxes.
    A step is an argv, or a (python_argv, rust_argv) pair."""
    with Sandbox() as py, Sandbox() as rs:
        for s in (py, rs):
            setup(s)
        for step in steps:
            py_argv, rs_argv = step if isinstance(step[0], (list, tuple)) else (step, step)
            a, b = (s.settled(r) for s, r in ((py, py.python(*py_argv)), (rs, rs.rust(*rs_argv))))
            assert_same(a, b, stdout)
            assert_same_files(py, rs, normalise)
            _case.assertEqual(*(
                [c for c in r.calls if not c.startswith(drop)] for r in (a, b)), "calls differ")
            if inspect:
                inspect(py, rs)
    return py, rs


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
if args[:1] == ["eval"] and out.startswith(b"error"):
    sys.exit(7)
"""


class Sandbox:
    def __init__(self, hypr_up=True):
        self.hypr_up = hypr_up
        self.base = pathlib.Path(tempfile.mkdtemp(prefix="ph-", dir="/tmp"))
        self.root, self.home, self.run, self.stubs, self.bin = (self.base / d for d in ("root", "home", "run", "stubs", "bin"))
        for d in (self.root, self.home, self.run, self.stubs, self.bin):
            d.mkdir()
        self.calls_log = self.base / "calls.log"
        self.path = f"{self.stubs}:/usr/bin:/bin"
        self.hypr = {}
        self.hypr_on_eval = None
        self.eval_reply = "ok"
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
        if self.hypr_up:
            self._server.shutdown(socket.SHUT_RDWR)
            self._server.close()
            self._thread.join(timeout=2)
        shutil.rmtree(self.base, ignore_errors=True)

    def _install_scripts(self):
        src = REPO / "bin"
        for name in ("ops.py", "display_power.py", "idle_wake.py", "kblight.py"):
            (self.bin / name).write_text(self._rewrite(name, (src / name).read_text()))
        (self.bin / "battery-plus-data").write_text(self._rewrite("battery-plus-data", (REPO / "tests/fixtures/battery-plus-data").read_text()))
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
        self.sock_path = str(sock_dir / ".socket.sock")
        if self.hypr_up:
            sock_dir.mkdir(parents=True)
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
                        if self.hypr_on_eval:
                            self.hypr_on_eval(req[5:])
                        reply = self.eval_reply
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
        else:  # the host's own framework_tool must not be found
            self.path = str(self.stubs)

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

    def stateful_profile(self, active):
        """powerprofilesctl whose `get` returns the last `set`."""
        state = self.base / "profile.state"
        state.write_text(active + "\n")
        self._stateful("powerprofilesctl", f'''case "$1" in
  get) cat {state};;
  set) echo "$2" > {state};;
  *) exit 1;;
esac''')

    def stateful_brightness(self, value):
        """`omarchy brightness display --monitor X` returns the last `--no-osd` set."""
        state = self.base / "brightness.state"
        state.write_text(f"{value}\n")
        self._stateful("omarchy", f'''case "$*" in
  *--no-osd*) echo "${{@: -1}}" | tr -d % > {state};;
  *"brightness display --monitor"*) cat {state};;
  *) exit 1;;
esac''')

    def _stateful(self, tool, body):
        p = self.stubs / tool
        p.write_text(f'#!/bin/bash\necho "{tool} $*" >> {shlex.quote(str(self.calls_log))}\n{body}\n')
        p.chmod(0o755)

    def snapshot(self, normalise=None):
        """Every file under home/ and run/ (not locks or temp files) as (mode, content), with the base path masked."""
        out = {}
        for top in (self.home, self.run):
            for p in sorted(top.rglob("*")):
                if not (p.is_symlink() or p.is_file()) or p.suffix in (".lock", ".tmp"):
                    continue
                rel = str(p.relative_to(self.base))
                if p.is_symlink():
                    out[rel] = ("symlink", os.readlink(p).replace(str(self.base), "<base>"))
                    continue
                text = p.read_text(errors="replace").replace(str(self.base), "<base>")
                if normalise:
                    text = normalise(rel, text)
                if p.suffix == ".json":
                    try:  # whitespace and escaping may differ; key order and value types may not
                        text = json.dumps(json.loads(text))
                    except ValueError:
                        pass
                out[rel] = (stat.S_IMODE(p.stat().st_mode), text)
        return out

    def settled(self, r):
        """A result with this sandbox's base path masked, so two sandboxes compare equal."""
        mask = lambda t: t.replace(str(self.base), "<base>")  # noqa: E731
        return replace(r, stdout=mask(r.stdout), calls=[mask(c) for c in r.calls], calls_text=mask(r.calls_text),
                       evals=[mask(e) for e in r.evals], requests=[mask(q) for q in r.requests])

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

    def lid(self, state):
        self.write("/proc/acpi/button/lid/LID0/state", f"state:      {state}\n")

    def devices(self, mice=(), keyboards=()):
        row = lambda d: d if isinstance(d, dict) else {"name": d}  # noqa: E731
        self.hypr["j/devices"] = json.dumps({"mice": [row(d) for d in mice], "keyboards": [row(d) for d in keyboards]})

    @contextlib.contextmanager
    def hold_flock(self, path):
        p = self._target(path)
        p.parent.mkdir(parents=True, exist_ok=True)
        with open(p, "w") as fh:
            fcntl.flock(fh, fcntl.LOCK_EX)
            yield

    def read_state(self):
        return json.loads(self._target("run/omarchy-power-hub/display-off.json").read_text())

    def _env(self):
        return {
            "PATH": self.path,
            "HOME": str(self.home),
            "XDG_RUNTIME_DIR": str(self.run),
            "HYPRLAND_INSTANCE_SIGNATURE": "test",
            "POWER_HUB_ROOT": str(self.root),
            "POWER_HUB_BIN": str(self.bin),
            "PYTHONDONTWRITEBYTECODE": "1",
        }

    def rust_start(self, *argv):
        """Start the Rust binary without waiting for it."""
        return subprocess.Popen([str(RUST_BIN), *argv], env=self._env(), cwd=self.base, text=True,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def _exec(self, argv):
        env = self._env()
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
