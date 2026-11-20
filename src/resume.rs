use crate::{config, exec, paths, resume_run, t3};
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::fs::{self, File, OpenOptions};
use std::io;

const TRIGGERS: [&str; 3] = ["wake", "displays", "unlock"];
const T3CODE: [&str; 3] = ["off", "update", "restart"];
const FLAGS: [&str; 7] = ["omarchy", "plugins", "leanCtx", "rulesync", "skillsManager", "restartShell", "sessionRestore"];
const RULESYNC_TTL: u64 = 3600;

pub fn dir() -> PathBuf {
    paths::state_dir().join("omarchy-power-hub")
}

pub fn state_file() -> PathBuf {
    dir().join("resume.json")
}

pub fn log_file() -> PathBuf {
    dir().join("resume.log")
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn read_state() -> Map<String, Value> {
    fs::read_to_string(state_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// `name` in the state dir, opened for an flock. Rust opens it close-on-exec, so no child inherits the lock.
fn lock_file(name: &str) -> io::Result<File> {
    fs::create_dir_all(dir())?;
    OpenOptions::new().write(true).create(true).truncate(false).open(dir().join(name))
}

/// An exclusive flock on `name`, held until dropped, or `None` while another process holds it.
pub fn try_lock(name: &str) -> Option<File> {
    lock_file(name).ok().filter(|f| f.try_lock().is_ok())
}

/// Read-modify-write of `resume.json` under its lock, so no writer drops another's keys.
pub fn update_state<T>(change: impl FnOnce(&mut Map<String, Value>) -> T) -> io::Result<T> {
    let lock = lock_file("resume.json.lock")?;
    lock.lock()?;
    let mut state = read_state();
    let out = change(&mut state);
    let (p, tmp) = (state_file(), dir().join("resume.json.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(&state).map_err(io::Error::other)? + "\n")?;
    fs::rename(tmp, p)?;
    Ok(out)
}

fn bounded(n: Option<i64>, max: i64) -> Option<i64> {
    n.filter(|n| (0..=max).contains(n))
}

/// The `onResume` block, with every key that is missing or has the wrong type or range at its default.
pub fn settings() -> Value {
    let raw = &config::raw()["onResume"];
    let triggers = raw["triggers"].as_array().filter(|a| a.iter().all(|t| t.as_str().is_some_and(|t| TRIGGERS.contains(&t)))).map_or(json!(["wake"]), |a| json!(a));
    let mut out = Map::new();
    out.insert("triggers".into(), triggers);
    out.insert("awayMinutes".into(), json!(bounded(raw["awayMinutes"].as_i64(), 1440).unwrap_or(30)));
    out.insert("everyHours".into(), json!(bounded(raw["everyHours"].as_i64(), 168).unwrap_or(4)));
    for k in FLAGS {
        out.insert(k.into(), json!(raw[k].as_bool().unwrap_or(true)));
    }
    out.insert("t3code".into(), json!(raw["t3code"].as_str().filter(|v| T3CODE.contains(v)).unwrap_or("restart")));
    out.insert("script".into(), json!(raw["script"].as_str().unwrap_or("")));
    Value::Object(out)
}

/// Saves one `onResume` key; empty on success.
pub fn set(key: &str, value: &str) -> String {
    let parsed = match key {
        "triggers" => {
            let picked: Vec<&str> = value.split(',').map(str::trim).filter(|t| !t.is_empty()).collect();
            picked.iter().all(|t| TRIGGERS.contains(t)).then(|| json!(TRIGGERS.iter().filter(|t| picked.contains(t)).collect::<Vec<_>>()))
        }
        "awayMinutes" => bounded(value.trim().parse().ok(), 1440).map(|n| json!(n)),
        "everyHours" => bounded(value.trim().parse().ok(), 168).map(|n| json!(n)),
        "t3code" => T3CODE.contains(&value).then(|| json!(value)),
        "script" => Some(json!(value)),
        k if FLAGS.contains(&k) => match value {
            "on" | "1" | "true" => Some(json!(true)),
            "off" | "0" | "false" => Some(json!(false)),
            _ => None,
        },
        _ => return "bad key".into(),
    };
    let Some(v) = parsed else { return "bad value".into() };
    let mut cfg = config::load();
    if !cfg["onResume"].is_object() {
        cfg["onResume"] = json!({});
    }
    cfg["onResume"][key] = v;
    config::save(&cfg).err().map(|e| e.to_string()).unwrap_or_default()
}

/// A probe answer cached for an hour, since every panel poll comes here. A failed or hung probe is cached too, so polls stay cheap.
fn cached_probe(key: &str, probe: impl FnOnce() -> bool) -> bool {
    let cached = read_state().get(key).cloned().unwrap_or_default();
    if let (Some(at), Some(found)) = (cached["at"].as_u64(), cached["installed"].as_bool()) {
        if now().saturating_sub(at) < RULESYNC_TTL {
            return found;
        }
    }
    let found = probe();
    let _ = update_state(|s| {
        s.insert(key.into(), json!({"at": now(), "installed": found}));
    });
    found
}

/// Whether `mise` has Rulesync. Stdout only: a warning on stderr must not read as an install.
fn rulesync() -> bool {
    exec::on_path("mise")
        && cached_probe("rulesync", || {
            let (code, out) = exec::run(&["/bin/sh", "-c", "exec mise ls --json npm:rulesync 2>/dev/null"], Duration::from_secs(3));
            code == 0 && serde_json::from_str::<Value>(&out).is_ok_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        })
}

/// Whether `hypr-persist restore` has the `--class` filter that Session Restore needs.
fn hypr_persist() -> bool {
    exec::on_path("hypr-persist")
        && cached_probe("hyprPersist", || {
            let (_, out) = exec::run(&["/bin/sh", "-c", "exec hypr-persist restore --help 2>&1"], Duration::from_secs(3));
            out.contains("--class")
        })
}

pub fn status() -> Value {
    let settings = settings();
    let state = read_state();
    let skills = exec::on_path("skills-manager-refresh");
    json!({
        "settings": settings,
        "installed": {
            "leanCtx": exec::executable(&paths::home().join(".local/bin/lean-ctx")),
            "rulesync": rulesync(),
            "skillsManager": skills,
            "t3code": paths::home().join(".local/share/t3code/T3-Code.AppImage").is_file(),
            "hyprPersist": hypr_persist(),
        },
        "lastRun": state.get("lastRun").cloned().unwrap_or_default(),
        "lastError": state.get("lastError").cloned().unwrap_or_default(),
        "needsSudo": settings["script"] == "" && (settings["omarchy"] == true || (settings["skillsManager"] == true && skills)),
    })
}

/// Why a resume must not start a run, or `Ok` when it should. `--now` skips every limit but a live run.
fn gate(state: &Map<String, Value>, source: &str, away_secs: u64, now: bool) -> Result<(), &'static str> {
    let settings = settings();
    if !now && !settings["triggers"].as_array().is_some_and(|t| t.iter().any(|t| t == source)) {
        return Err("no trigger");
    }
    if try_lock("resume.lock").is_none() {
        return Err("running");
    }
    if now {
        return Ok(());
    }
    if source != "wake" && away_secs < settings["awayMinutes"].as_u64().unwrap_or(30) * 60 {
        return Err("not away long enough");
    }
    let last = state.get("lastRun").and_then(Value::as_u64).unwrap_or(0);
    if self::now().saturating_sub(last) < settings["everyHours"].as_u64().unwrap_or(4) * 3600 {
        return Err("too soon");
    }
    Ok(())
}

/// Starts the run detached from the caller, in a terminal when it needs sudo.
pub fn dispatch() {
    let bin = paths::bin_dir().join("power-hub").to_string_lossy().into_owned();
    let terminal = ["xdg-terminal-exec", "--app-id=org.omarchy.terminal", "--title=Stack Refresh", "-e"];
    let mut argv = vec!["uwsm-app", "--"];
    if status()["needsSudo"] == true {
        argv.extend(terminal);
    }
    argv.extend([bin.as_str(), "resume-run", "--exec"]);
    detach(&argv);
}

/// Starts `argv` in its own session with no stdio, without waiting.
pub fn detach(argv: &[&str]) {
    let _ = Command::new("setsid").args(argv).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// `resume-run --source S --away N`, `--now` or `--exec`; the gate's verdict is silent. Returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let flag = |f: &str| args.iter().position(|a| a == f).and_then(|i| args.get(i + 1));
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--exec") {
        return resume_run::exec();
    }
    if has("--t3-restart") {
        // One waiter at a time: a second one exits.
        let Some(_held) = try_lock("t3-restart.lock") else { return 0 };
        return i32::from(t3::wait_idle_then_restart(has("--restore")).is_err());
    }
    let source = flag("--source").map_or("", String::as_str);
    if source.is_empty() && !has("--now") {
        return 2;
    }
    let away = flag("--away").and_then(|a| a.parse().ok()).unwrap_or(0);
    // Gate and claim under one lock, so a second trigger right behind this one hits "too soon".
    let go = update_state(|s| gate(s, source, away, has("--now")).inspect(|()| _ = s.insert("lastRun".into(), json!(now()))).is_ok());
    if go.unwrap_or(false) {
        dispatch();
    }
    0
}
