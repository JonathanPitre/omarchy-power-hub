//! A fake system for the binary, and replay of the Python results recorded in `tests/golden`.
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixListener;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Once, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const RUST_BIN: &str = env!("CARGO_BIN_EXE_power-hub");
const REPO: &str = env!("CARGO_MANIFEST_DIR");
const LED: &str = "sys/class/leds/chromeos:white:power";
const STUBBED: &[&str] = &["busctl", "iw", "systemctl", "sudo", "pkexec", "upower", "omarchy", "powerprofilesctl",
    "omarchy-powerprofiles-list", "omarchy-powerprofiles-set", "omarchy-system-lock", "omarchy-shell",
    "omarchy-launch-screensaver", "omarchy-launch-floating-terminal-with-presentation", "pkill"];
pub const DROPPED: &[&str] = &["hyprctl", "busctl get-property net.hadess", "omarchy-powerprofiles-list"];

unsafe extern "C" {
    fn atexit(handler: extern "C" fn()) -> i32;
    fn _exit(code: i32) -> !;
    fn shutdown(fd: i32, how: i32) -> i32;
    fn umask(mask: u32) -> u32;
}

pub type Normalise = fn(&str, &str) -> String;
pub type OnEval = Box<dyn FnMut(&mut Hypr, &str) + Send>;
type Rules = HashMap<String, Vec<(String, i32, String)>>;
pub type Cursors = HashMap<String, (Arc<Vec<Value>>, usize)>;

/// How a step is compared. The default is what `both()` compares; `direct()` is what `assert_same` compares, plus files.
#[derive(Clone, Copy)]
pub struct Replay {
    pub drop: &'static [&'static str],
    pub normalise: Option<Normalise>,
    pub stdout: bool,
    pub calls: bool,
    pub requests: bool,
}

impl Replay {
    pub const fn direct() -> Self {
        Self { drop: DROPPED, normalise: None, stdout: true, calls: false, requests: false }
    }
}

impl Default for Replay {
    fn default() -> Self {
        Self { calls: true, ..Self::direct() }
    }
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub calls: Vec<String>,
    pub evals: Vec<String>,
    pub requests: Vec<String>,
}

impl Outcome {
    pub fn calls_text(&self) -> String {
        self.calls.join("\n")
    }
}

/// What the fake Hyprland replies, and what it has seen.
pub struct Hypr {
    pub replies: HashMap<String, String>,
    pub on_eval: Option<OnEval>,
    pub eval_reply: String,
    pub reply_delay: Duration,
    pub evals: Vec<String>,
    pub requests: Vec<String>,
}

impl Hypr {
    pub fn monitors(&mut self, rows: &[Value]) {
        self.replies.insert("j/monitors".into(), json!(rows).to_string());
    }
}

fn lock(hypr: &Mutex<Hypr>) -> MutexGuard<'_, Hypr> {
    hypr.lock().unwrap_or_else(|e| e.into_inner())
}

fn answer(conn: &mut (impl Read + Write), hypr: &Mutex<Hypr>) -> io::Result<()> {
    let mut buf = vec![0; 65536];
    let n = conn.read(&mut buf)?;
    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
    let delay = {
        let mut h = lock(hypr);
        h.requests.push(req.clone());
        h.reply_delay
    };
    thread::sleep(delay);
    let mut h = lock(hypr);
    let reply = if let Some(lua) = req.strip_prefix("eval ") {
        h.evals.push(lua.to_string());
        if let Some(mut on_eval) = h.on_eval.take() {
            on_eval(&mut h, lua);
            h.on_eval.get_or_insert(on_eval);
        }
        h.eval_reply.clone()
    } else {
        h.replies.get(&req).cloned().unwrap_or_default()
    };
    drop(h);
    conn.write_all(reply.as_bytes())
}

/// Options of `Sandbox::pci`.
pub struct Pci {
    pub cls: &'static str,
    pub boot_vga: Option<&'static str>,
    pub runtime: &'static str,
    pub driver: Option<&'static str>,
}

impl Default for Pci {
    fn default() -> Self {
        Self { cls: "0x030000", boot_vga: None, runtime: "active", driver: None }
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Held shared while spawning and exclusively while writing a script: a child forked by another test keeps every open
/// file until it execs, and a script still open for writing in any process fails to exec with ETXTBSY.
static FORK: RwLock<()> = RwLock::new(());

pub fn executable(path: &Path, text: &str) {
    {
        let _writing = FORK.write().unwrap_or_else(|e| e.into_inner());
        fs::write(path, text).unwrap();
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Every process a test starts goes through here, for `FORK`.
pub fn spawn(command: &mut Command) -> Child {
    let _spawning = FORK.read().unwrap_or_else(|e| e.into_inner());
    command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
}

/// `subprocess.run` with the output captured.
pub fn run(command: &mut Command) -> Output {
    spawn(command).wait_with_output().unwrap()
}

/// `json.dumps(json.loads(text))` as a comparison key: key order and number types stay, spacing and escapes go.
fn canonical(text: &str) -> String {
    serde_json::from_str::<Value>(text).map_or_else(|_| text.to_string(), |v| v.to_string())
}

static SANDBOXES: AtomicUsize = AtomicUsize::new(0);

/// Every file a run can see. The two listeners answer alike: the binary uses the socket, the Bash `hyprctl` stub TCP.
pub struct Sandbox {
    pub base: PathBuf,
    pub root: PathBuf,
    pub home: PathBuf,
    pub run: PathBuf,
    pub stubs: PathBuf,
    pub bin: PathBuf,
    pub calls_log: PathBuf,
    pub sock_path: PathBuf,
    path: RefCell<String>,
    rules: RefCell<Rules>,
    traced: Cell<usize>,
    hypr: Arc<Mutex<Hypr>>,
    servers: Vec<(i32, JoinHandle<()>)>,
    listeners: Option<(UnixListener, TcpListener)>,
}

impl Sandbox {
    pub const CONFIG_PATH: &str = "home/.config/omarchy/omarchy-power-hub.json";

    pub fn new() -> Self {
        Self::build(true)
    }

    pub fn without_hypr() -> Self {
        Self::build(false)
    }

    fn build(hypr_up: bool) -> Self {
        unsafe { umask(0o022) }; // goldens hold file modes, recorded under this mask
        let n = SANDBOXES.fetch_add(1, Ordering::Relaxed);
        let base = PathBuf::from(format!("/tmp/ph-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let [root, home, run, stubs, bin] = ["root", "home", "run", "stubs", "bin"].map(|d| base.join(d));
        for d in [&root, &home, &run, &stubs, &bin] {
            fs::create_dir_all(d).unwrap();
        }
        fs::copy(Path::new(REPO).join("bin/ops-priv"), bin.join("ops-priv")).unwrap();
        let sock_path = run.join("hypr/test/.socket.sock");
        let mut s = Self {
            calls_log: base.join("calls.log"),
            path: RefCell::new(format!("{}:/usr/bin:/bin", stubs.display())),
            rules: RefCell::default(),
            traced: Cell::new(0),
            hypr: Arc::new(Mutex::new(Hypr {
                replies: HashMap::new(),
                on_eval: None,
                eval_reply: "ok".into(),
                reply_delay: Duration::ZERO,
                evals: vec![],
                requests: vec![],
            })),
            servers: vec![],
            listeners: None,
            base, root, home, run, stubs, bin, sock_path,
        };
        for tool in STUBBED {
            s.write_stub(tool);
        }
        s.serve(hypr_up);
        s
    }

    fn serve(&mut self, hypr_up: bool) {
        let mut port = 1;
        if hypr_up {
            fs::create_dir_all(self.sock_path.parent().unwrap()).unwrap();
            let unix = UnixListener::bind(&self.sock_path).unwrap();
            let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
            port = tcp.local_addr().unwrap().port();
            let (unix2, tcp2) = (unix.try_clone().unwrap(), tcp.try_clone().unwrap());
            let (hypr, hypr2) = (self.hypr.clone(), self.hypr.clone());
            self.servers = vec![
                (unix.as_raw_fd(), thread::spawn(move || {
                    for mut conn in unix2.incoming().map_while(Result::ok) {
                        let _ = answer(&mut conn, &hypr);
                    }
                })),
                (tcp.as_raw_fd(), thread::spawn(move || {
                    for mut conn in tcp2.incoming().map_while(Result::ok) {
                        let _ = answer(&mut conn, &hypr2);
                    }
                })),
            ];
            self.listeners = Some((unix, tcp));
        }
        let script = format!(r#"#!/bin/bash
echo "hyprctl $*" >> {calls}
if [ "$1" = eval ]; then req="eval ${{*:2}}"
elif [ "${{!#}}" = -j ]; then req="j/${{*:1:$#-1}}"
else req="$*"; fi
[ -S {sock} ] || exit 1
exec 3<>/dev/tcp/127.0.0.1/{port}
printf %s "$req" >&3
out=$(cat <&3; printf .); out=${{out%.}}
printf %s "$out"
if [ "$1" = eval ] && [[ "$out" == error* ]]; then exit 7; fi
"#, calls = quote(&self.calls_log.display().to_string()), sock = quote(&self.sock_path.display().to_string()));
        executable(&self.stubs.join("hyprctl"), &script);
    }

    pub fn hypr(&self) -> MutexGuard<'_, Hypr> {
        lock(&self.hypr)
    }

    /// `s.hypr[request] = reply`.
    pub fn reply(&self, request: &str, reply: &str) {
        self.hypr().replies.insert(request.into(), reply.into());
    }

    pub fn target(&self, path: &str) -> PathBuf {
        if let Some(rest) = path.strip_prefix("~/").or(path.strip_prefix("home/")) {
            return self.home.join(rest);
        }
        if let Some(rest) = path.strip_prefix("run/") {
            return self.run.join(rest);
        }
        self.root.join(path.trim_start_matches('/'))
    }

    pub fn write(&self, path: &str, text: &str) -> PathBuf {
        let p = self.target(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, text).unwrap();
        p
    }

    pub fn write_stub(&self, tool: &str) {
        let rules = self.rules.borrow();
        let cases: String = rules.get(tool).into_iter().flatten()
            .map(|(m, code, out)| format!("  *{}*) printf %s {}; exit {code};;\n", quote(m), quote(out)))
            .collect();
        // The real list tool ends in a pipeline (tac), so it exits 0 with no output when powerprofilesctl fails.
        let fallback = i32::from(tool != "omarchy-powerprofiles-list");
        let log = quote(&self.calls_log.display().to_string());
        executable(&self.stubs.join(tool), &format!("#!/bin/bash\necho \"{tool} $*\" >> {log}\ncase \"$*\" in\n{cases}esac\nexit {fallback}\n"));
    }

    pub fn stub(&self, tool: &str, pattern: &str, code: i32, stdout: &str) {
        {
            // The case statement takes the first match, so a repeated match replaces its earlier rule.
            let mut rules = self.rules.borrow_mut();
            let rules = rules.entry(tool.to_string()).or_default();
            rules.retain(|r| r.0 != pattern);
            rules.push((pattern.to_string(), code, stdout.to_string()));
        }
        self.write_stub(tool);
    }

    pub fn pci(&self, addr: &str, opts: Pci) {
        let sysfs = self.root.join("sys/bus/pci");
        let dev = sysfs.join("devices").join(addr);
        fs::create_dir_all(dev.join("power")).unwrap();
        if let Some(driver) = opts.driver {
            let target = sysfs.join("drivers").join(driver);
            fs::create_dir_all(&target).unwrap();
            symlink(target, dev.join("driver")).unwrap();
        }
        fs::write(dev.join("class"), format!("{}\n", opts.cls)).unwrap();
        fs::write(dev.join("power/runtime_status"), format!("{}\n", opts.runtime)).unwrap();
        if let Some(boot_vga) = opts.boot_vga {
            fs::write(dev.join("boot_vga"), format!("{boot_vga}\n")).unwrap();
        }
    }

    fn supply(&self, name: &str) -> PathBuf {
        let dir = self.root.join("sys/class/power_supply").join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub fn discharging(&self, percent: u32) {
        let bat = self.supply("BAT0");
        fs::write(bat.join("status"), "Discharging\n").unwrap();
        fs::write(bat.join("capacity"), format!("{percent}\n")).unwrap();
    }

    /// A pack with sysfs files in energy units (or charge units); a None field removes its file.
    pub fn battery(&self, charge_units: bool, fields: &[(&str, Option<&str>)]) {
        let units = if charge_units {
            [("charge_now", "3500000"), ("charge_full", "4200000"), ("charge_full_design", "4500000"), ("current_now", "1000000")]
        } else {
            [("energy_now", "40000000"), ("energy_full", "50000000"), ("energy_full_design", "55000000"), ("power_now", "10000000")]
        };
        let defaults = [("status", "Discharging"), ("capacity", "70"), ("voltage_now", "12000000"), ("voltage_min_design", "11400000"), ("cycle_count", "123")];
        let bat = self.supply("BAT0");
        for (key, value) in defaults.iter().chain(&units).filter(|(k, _)| !fields.iter().any(|(f, _)| f == k)).map(|(k, v)| (*k, Some(*v))).chain(fields.iter().copied()) {
            match value {
                Some(value) => fs::write(bat.join(key), format!("{value}\n")).unwrap(),
                None => { let _ = fs::remove_file(bat.join(key)); }
            }
        }
    }

    pub fn ucsi(&self, name: &str, online: bool, volts: f64, amps: f64) {
        let port = self.supply(&format!("ucsi-source-psy-{name}"));
        fs::write(port.join("online"), format!("{}\n", i32::from(online))).unwrap();
        fs::write(port.join("voltage_now"), format!("{}\n", (volts * 1e6) as i64)).unwrap();
        fs::write(port.join("current_now"), format!("{}\n", (amps * 1e6) as i64)).unwrap();
    }

    pub fn framework(&self, tool: bool) {
        self.write("/sys/class/dmi/id/sys_vendor", "Framework\n");
        if tool {
            self.write_stub("framework_tool");
        } else { // the host's own framework_tool must not be found
            self.set_path(&self.stubs.display().to_string());
        }
    }

    pub fn set_path(&self, path: &str) {
        *self.path.borrow_mut() = path.to_string();
    }

    pub fn helper_dir(&self) -> PathBuf {
        self.root.join("usr/local/libexec/omarchy-power-hub")
    }

    /// Root-owned helper copies at the fake-root path; `tamper` makes ops-priv differ from the plugin's.
    pub fn install_helper(&self, tamper: bool) {
        let dir = self.helper_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::copy(self.bin.join("ops-priv"), dir.join("ops-priv")).unwrap();
        if tamper {
            File::options().append(true).open(dir.join("ops-priv")).unwrap().write_all(b"# tampered\n").unwrap();
        }
    }

    /// The EC's current limit as the helper reports it, for the read and for any write.
    pub fn charge_limit(&self, percent: u32) {
        self.stub("sudo", "charge-limit", 0, &format!("Minimum 0%, Maximum {percent}%\n"));
    }

    /// powerprofilesctl whose `get` returns the last `set`.
    pub fn stateful_profile(&self, active: &str) {
        let state = self.base.join("profile.state");
        fs::write(&state, format!("{active}\n")).unwrap();
        let state = state.display();
        self.stateful("powerprofilesctl", &format!("case \"$1\" in\n  get) cat {state};;\n  set) echo \"$2\" > {state};;\n  *) exit 1;;\nesac"));
    }

    /// `omarchy brightness display --monitor X` returns the last `--no-osd` set.
    pub fn stateful_brightness(&self, value: u32) {
        let state = self.base.join("brightness.state");
        fs::write(&state, format!("{value}\n")).unwrap();
        let state = state.display();
        self.stateful("omarchy", &format!("case \"$*\" in\n  *--no-osd*) echo \"${{@: -1}}\" | tr -d % > {state};;\n  *\"brightness display --monitor\"*) cat {state};;\n  *) exit 1;;\nesac"));
    }

    pub fn stateful(&self, tool: &str, body: &str) {
        let log = quote(&self.calls_log.display().to_string());
        executable(&self.stubs.join(tool), &format!("#!/bin/bash\necho \"{tool} $*\" >> {log}\n{body}\n"));
    }

    /// Every file under home/ and run/ (not locks or temp files) as (mode, content), with the base path masked.
    pub fn snapshot(&self, normalise: Option<Normalise>) -> BTreeMap<String, (String, String)> {
        let mut out = BTreeMap::new();
        for top in [&self.home, &self.run] {
            self.walk(top, normalise, &mut out);
        }
        out
    }

    fn walk(&self, dir: &Path, normalise: Option<Normalise>, out: &mut BTreeMap<String, (String, String)>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                self.walk(&p, normalise, out);
                continue;
            }
            let suffix = p.extension().and_then(|e| e.to_str());
            if !(kind.is_symlink() || kind.is_file()) || matches!(suffix, Some("lock" | "tmp")) {
                continue;
            }
            let rel = p.strip_prefix(&self.base).unwrap().display().to_string();
            if kind.is_symlink() {
                out.insert(rel, ("symlink".into(), self.mask(&fs::read_link(&p).unwrap().display().to_string())));
                continue;
            }
            let text = self.mask(&String::from_utf8_lossy(&fs::read(&p).unwrap()));
            let mode = p.metadata().unwrap().mode() & 0o7777;
            out.insert(rel.clone(), (mode.to_string(), settle(&rel, &text, normalise)));
        }
    }

    /// State `both()` leaves out that other tests compare: stateful stub files, the power LED, new Travel trace lines.
    pub fn side(&self) -> Map<String, Value> {
        let mut out = Map::new();
        let leds = fs::read_dir(self.root.join(LED)).into_iter().flatten().flatten().map(|e| e.path());
        let states = fs::read_dir(&self.base).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "state"));
        for p in states.chain(leds).filter(|p| p.is_file()) {
            out.insert(p.file_name().unwrap().to_string_lossy().into_owned(), json!(String::from_utf8_lossy(&fs::read(&p).unwrap())));
        }
        if let Ok(trace) = fs::read_to_string(self.base.join("trace")) {
            let lines: Vec<String> = self.mask(&trace).lines().map(trace_line).collect();
            out.insert("trace".into(), json!(lines[self.traced.get()..]));
            self.traced.set(lines.len());
        }
        out
    }

    fn mask(&self, text: &str) -> String {
        text.replace(&self.base.display().to_string(), "<base>")
    }

    pub fn monitors(&self, rows: &[Value]) {
        self.hypr().monitors(rows);
    }

    /// Saved config with a valid profile on both sides, so loading never asks power-profiles-daemon.
    pub fn config(&self, keys: Value) {
        let Value::Object(keys) = keys else { panic!("config takes an object") };
        let mut saved = json!({"ac": {"profile": "balanced"}, "battery": {"profile": "balanced"}}).as_object().unwrap().clone();
        saved.extend(keys);
        self.write(Self::CONFIG_PATH, &Value::Object(saved).to_string());
    }

    pub fn read_config(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.target(Self::CONFIG_PATH)).unwrap()).unwrap()
    }

    pub fn profiles(&self, names: &[&str]) {
        let entries: Vec<String> = names.iter().map(|n| format!("1 \"Profile\" s \"{n}\" \"Driver\" s \"platform_profile\"")).collect();
        self.stub("busctl", "Profiles", 0, &format!("aa{{sv}} {} {}\n", names.len(), entries.join(" ")));
        let listing: String = names.iter().map(|n| format!("{n}\n")).collect();
        self.stub("omarchy-powerprofiles-list", "", 0, &listing);
        self.stub("powerprofilesctl", "", 0, &listing);
    }

    pub fn lid(&self, state: &str) {
        self.write("/proc/acpi/button/lid/LID0/state", &format!("state:      {state}\n"));
    }

    pub fn devices(&self, mice: &[Value], keyboards: &[Value]) {
        let row = |d: &Value| if d.is_object() { d.clone() } else { json!({"name": d}) };
        let list = |rows: &[Value]| rows.iter().map(row).collect::<Vec<_>>();
        self.reply("j/devices", &json!({"mice": list(mice), "keyboards": list(keyboards)}).to_string());
    }

    pub fn dmi(&self, vendor: &str, product: &str) {
        self.write("/sys/class/dmi/id/sys_vendor", &format!("{vendor}\n"));
        self.write("/sys/class/dmi/id/product_name", &format!("{product}\n"));
    }

    pub fn power_led(&self, trigger: &str, brightness: impl std::fmt::Display, max_brightness: impl std::fmt::Display) {
        self.write(&format!("/{LED}/trigger"), &format!("none [{trigger}] timer\n"));
        self.write(&format!("/{LED}/brightness"), &format!("{brightness}\n"));
        self.write(&format!("/{LED}/max_brightness"), &format!("{max_brightness}\n"));
    }

    pub fn light_sensor(&self, raw: impl std::fmt::Display, scale: Option<&str>) {
        let dev = "/sys/bus/iio/devices/iio:device0";
        self.write(&format!("{dev}/in_illuminance_raw"), &format!("{raw}\n"));
        if let Some(scale) = scale {
            self.write(&format!("{dev}/in_illuminance_scale"), &format!("{scale}\n"));
        }
    }

    /// Holds the lock until the returned file is dropped.
    pub fn hold_flock(&self, path: &str) -> File {
        let p = self.target(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        let file = File::create(p).unwrap();
        file.lock().unwrap();
        file
    }

    pub fn read_state(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.target("run/omarchy-power-hub/display-off.json")).unwrap()).unwrap()
    }

    pub fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("PATH", self.path.borrow().clone()),
            ("HOME", self.home.display().to_string()),
            ("XDG_RUNTIME_DIR", self.run.display().to_string()),
            ("HYPRLAND_INSTANCE_SIGNATURE", "test".to_string()),
            ("POWER_HUB_ROOT", self.root.display().to_string()),
            ("POWER_HUB_BIN", self.bin.display().to_string()),
        ]
    }

    fn command(&self, argv: &[&str]) -> Command {
        let mut c = Command::new(RUST_BIN);
        c.args(argv).env_clear().envs(self.env()).current_dir(&self.base).stdin(Stdio::null());
        c
    }

    /// Start the Rust binary without waiting for it.
    pub fn rust_start(&self, argv: &[&str]) -> Child {
        spawn(&mut self.command(argv))
    }

    pub fn rust(&self, argv: &[&str]) -> Outcome {
        fs::write(&self.calls_log, "").unwrap();
        {
            let mut h = self.hypr();
            h.evals.clear();
            h.requests.clear();
        }
        let p = run(&mut self.command(argv));
        let log = fs::read_to_string(&self.calls_log).unwrap();
        let h = self.hypr();
        Outcome {
            code: p.status.code().or(p.status.signal().map(|n| -n)).unwrap(),
            stdout: String::from_utf8_lossy(&p.stdout).into_owned(),
            calls: log.split('\n').filter(|c| !c.is_empty()).map(String::from).collect(),
            evals: h.evals.clone(),
            requests: h.requests.clone(),
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        for (fd, server) in self.servers.drain(..) {
            unsafe { shutdown(fd, 2) };
            let _ = server.join();
        }
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn settle(rel: &str, text: &str, normalise: Option<Normalise>) -> String {
    let text = normalise.map_or_else(|| text.to_string(), |f| f(rel, text));
    if rel.ends_with(".json") { canonical(&text) } else { text }
}

fn trace_line(line: &str) -> String {
    if line.starts_with('{') { canonical(line) } else { line.to_string() }
}

/// The snapshot comparison `both()` runs after every step.
pub fn assert_same_files(a: &Sandbox, b: &Sandbox, normalise: Option<Normalise>) {
    assert_eq!(a.snapshot(normalise), b.snapshot(normalise), "files differ");
}

/// Python's `==` on parsed JSON, which `assert_same` relies on: key order is free and 1 equals 1.0.
pub fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x == y || x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q)),
        (Value::Object(x), Value::Object(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w))),
        _ => a == b,
    }
}

/// Like `assert_same`: parsed when both sides are JSON, the text otherwise.
fn same_stdout(a: &str, b: &str) -> bool {
    match (serde_json::from_str(a), serde_json::from_str(b)) {
        (Ok(x), Ok(y)) => same_json(&x, &y),
        _ => a == b,
    }
}

fn strings(step: &Value, key: &str) -> Vec<String> {
    step[key].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

fn golden_files(step: &Value, normalise: Option<Normalise>) -> BTreeMap<String, (String, String)> {
    step["files"].as_object().unwrap().iter().map(|(rel, entry)| {
        let (mode, text) = (&entry[0], entry[1].as_str().unwrap());
        match mode.as_str() {
            Some(link) => (rel.clone(), (link.to_string(), text.to_string())),
            None => (rel.clone(), (mode.to_string(), settle(rel, text, normalise))),
        }
    }).collect()
}

/// Whether `got`, just run in `s`, matches a recorded step.
pub fn verify(s: &Sandbox, step: &Value, got: &Outcome, opts: Replay) -> Result<(), String> {
    fn differ<T: PartialEq + std::fmt::Debug>(what: &str, golden: T, rust: T) -> Result<(), String> {
        if golden == rust { Ok(()) } else { Err(format!("{what}\n golden: {golden:#?}\n   rust: {rust:#?}")) }
    }
    let mask = |rows: &[String]| rows.iter().map(|r| s.mask(r)).collect::<Vec<_>>();
    let kept = |rows: Vec<String>| rows.into_iter().filter(|c| !opts.drop.iter().any(|d| c.starts_with(d))).collect::<Vec<_>>();
    let stdout = s.mask(&got.stdout);
    let recorded = step["stdout"].as_str().unwrap();
    if opts.stdout && !same_stdout(recorded, &stdout) {
        return Err(format!("stdout differs\n golden: {recorded}\n   rust: {stdout}"));
    }
    differ("exit code differs", step["code"].as_i64().unwrap(), i64::from(got.code))?;
    differ("hyprland evals differ", strings(step, "evals"), mask(&got.evals))?;
    differ("files differ", golden_files(step, opts.normalise), s.snapshot(opts.normalise))?;
    if opts.calls {
        differ("calls differ", kept(strings(step, "calls")), kept(mask(&got.calls)))?;
    }
    if opts.requests {
        differ("requests differ", strings(step, "requests"), mask(&got.requests))?;
    }
    let mut side = step.get("side").and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(Value::Array(lines)) = side.get_mut("trace") {
        lines.iter_mut().for_each(|l| *l = json!(trace_line(l.as_str().unwrap())));
    }
    differ("side files differ", side, s.side())
}

fn golden(name: &str) -> Vec<Value> {
    let text = fs::read_to_string(format!("{REPO}/tests/golden/{name}.json")).unwrap_or_else(|e| panic!("no golden for {name}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("bad golden for {name}: {e}"))
}

/// Step `index` of a golden, for tests of the comparison itself.
pub fn golden_step(name: &str, index: usize) -> Value {
    golden(name).swap_remove(index)
}

static CURSORS: LazyLock<Mutex<Cursors>> = LazyLock::new(Mutex::default);
static AUDIT: Once = Once::new();

/// Every golden a test started and did not finish, by name.
pub fn stopped_short(cursors: &Cursors) -> Vec<String> {
    let mut short: Vec<String> = cursors.iter().filter(|(_, (steps, next))| next < &steps.len())
        .map(|(name, (steps, next))| format!("{name} stopped at step {next} of {}", steps.len())).collect();
    short.sort();
    short
}

/// Runs at exit: libtest has no hook after the last test, and a test that drops a replay would otherwise stay green.
extern "C" fn audit_cursors() {
    let short = stopped_short(&CURSORS.lock().unwrap_or_else(|e| e.into_inner()));
    if !short.is_empty() {
        eprintln!("goldens not fully replayed:\n  {}", short.join("\n  "));
        unsafe { _exit(1) };
    }
}

/// The next recorded step of `<Class>.<test>`, in call order across every sandbox of the test.
fn next_step(name: &str) -> (Arc<Vec<Value>>, usize) {
    AUDIT.call_once(|| assert_eq!(unsafe { atexit(audit_cursors) }, 0));
    let mut cursors = CURSORS.lock().unwrap_or_else(|e| e.into_inner());
    let entry = cursors.entry(name.to_string()).or_insert_with(|| (Arc::new(golden(name)), 0));
    entry.1 += 1;
    (entry.0.clone(), entry.1 - 1)
}

/// Runs `argv` in `s` and compares it with the next recorded step of `<Class>.<test>`. A missing golden or step fails.
pub fn replay(s: &Sandbox, name: &str, argv: &[&str], opts: Replay) -> Outcome {
    let (steps, index) = next_step(name);
    let step = steps.get(index).unwrap_or_else(|| panic!("{name} has {} recorded steps, no step {index}", steps.len()));
    let got = s.rust(argv);
    if let Err(diff) = verify(s, step, &got, opts) {
        panic!("{name} step {index} {argv:?}: {diff}");
    }
    got
}
