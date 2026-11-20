//! The T3 Code step: update the AppImage from its channel's feed, then restart it once it is idle.
use crate::resume_run::{StepResult, log, sh};
use crate::{exec, paths};
use serde_json::Value;
use std::time::{Duration, Instant};

const REPO: &str = "https://github.com/pingdotgg/t3code";
const NAME: &str = "T3 Code";
const READY: &str = "T3 Code update ready, restarting when idle";
const MANUAL: &str = "T3 Code updated, restart it to finish";
/// No thread running, and none touched within this long, counts as idle.
const QUIET_SECS: u64 = 120;

fn env_u64(var: &str, default: u64) -> u64 {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn poll() -> Duration {
    Duration::from_millis(env_u64("POWER_HUB_T3_POLL_MS", 30_000))
}

fn t3_bin() -> String {
    if exec::on_path("t3") { "t3".into() } else { paths::home().join(".t3/bin/t3").to_string_lossy().into_owned() }
}

fn appimage() -> String {
    paths::home().join(".local/share/t3code/T3-Code.AppImage").to_string_lossy().into_owned()
}

/// The installed version, such as `v0.1.11-nightly.20261119.4941`.
fn version() -> Option<String> {
    let (code, out) = exec::run(&[&t3_bin(), "--version"], Duration::from_secs(30));
    out.split_whitespace().nth(1).filter(|_| code == 0).map(String::from)
}

/// `updateChannel` from T3 Code's settings, else the installed version's own channel.
fn channel() -> &'static str {
    let saved = std::fs::read_to_string(paths::home().join(".t3/userdata/desktop-settings.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    match saved.as_ref().and_then(|s| s["updateChannel"].as_str()) {
        Some("latest") => "stable",
        Some("nightly") => "nightly",
        _ if version().is_some_and(|v| v.contains("-nightly")) => "nightly",
        _ => "stable",
    }
}

fn curl(url: &str) -> Result<String, String> {
    let (code, out) = exec::run(&["curl", "-fsSL", "--max-time", "30", url], Duration::from_secs(40));
    if code == 0 { Ok(out) } else { Err("T3 Code feed unreachable".into()) }
}

/// `v1.2.3-nightly.20261119.4941`, the tags `omarchy-install-t3code-app` accepts.
fn nightly_tag(tag: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    tag.strip_prefix('v').and_then(|t| t.split_once("-nightly.")).is_some_and(|(v, n)| v.split('.').count() == 3 && v.split('.').all(digits) && n.split('.').count() == 2 && n.split('.').all(digits))
}

/// The sha512 the channel's feed lists for its newest release, as the installer reads it.
fn feed_sha(channel: &str) -> Result<String, String> {
    let arch = if std::env::consts::ARCH == "aarch64" { "-arm64" } else { "" };
    let url = if channel == "stable" {
        format!("{REPO}/releases/latest/download/latest-linux{arch}.yml")
    } else {
        let releases: Value = serde_json::from_str(&curl("https://api.github.com/repos/pingdotgg/t3code/releases?per_page=100")?).map_err(|_| "T3 Code feed unreachable")?;
        let tag = releases.as_array().into_iter().flatten().filter(|r| r["draft"] != true).filter_map(|r| r["tag_name"].as_str()).find(|t| nightly_tag(t));
        format!("{REPO}/releases/download/{}/nightly-linux{arch}.yml", tag.ok_or("no T3 Code nightly release")?)
    };
    let feed = curl(&url)?;
    feed.lines().find_map(|l| l.strip_prefix("sha512:")).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).ok_or_else(|| "T3 Code feed is not in the expected format".into())
}

fn installed_sha() -> Option<String> {
    let (code, out) = exec::run(&["sh", "-c", "openssl dgst -sha512 -binary \"$1\" | base64 -w0", "_", &appimage()], Duration::from_secs(60));
    Some(out.trim().to_owned()).filter(|s| code == 0 && !s.is_empty())
}

pub fn step() -> StepResult {
    let Some(before) = version() else { return StepResult::Failed("could not read the T3 Code version".into()) };
    let channel = channel();
    let latest = match feed_sha(channel) {
        Ok(s) => s,
        Err(e) => return StepResult::Failed(e),
    };
    match installed_sha() {
        Some(s) if s == latest => return StepResult::Current,
        None => return StepResult::Failed("could not hash the installed T3 Code".into()),
        _ => {}
    }
    if let Err(e) = sh(NAME, &["omarchy-install-t3code-app", channel]) {
        return StepResult::Failed(e);
    }
    StepResult::Updated(vec![(NAME.into(), before, version().unwrap_or_else(|| "unknown".into()))])
}

/// Whether no thread of any live project is running or was updated within `QUIET_SECS`.
fn idle() -> Result<bool, String> {
    let db = paths::home().join(".t3/userdata/statev2.sqlite");
    let (code, ids) = exec::run(&["sqlite3", "-readonly", &db.to_string_lossy(), "select project_id from projection_projects where deleted_at is null and archived_at is null"], Duration::from_secs(30));
    if code != 0 {
        return Err("could not read the T3 Code projects".into());
    }
    let (_, cutoff) = exec::run(&["date", "-u", "-d", &format!("-{QUIET_SECS} sec"), "+%Y-%m-%dT%H:%M:%S.%3NZ"], Duration::from_secs(5));
    let (bin, cutoff) = (t3_bin(), cutoff.trim().to_owned());
    for id in ids.lines().filter(|l| !l.is_empty()) {
        let (code, out) = exec::run(&[&bin, "thread", "list", "--json", "--project", id], Duration::from_secs(30));
        let threads: Value = serde_json::from_str(&out).ok().filter(|_| code == 0).ok_or("could not list the T3 Code threads")?;
        // The timestamps are fixed-width ISO UTC, so text order is time order.
        if threads.as_array().ok_or("unexpected thread list")?.iter().any(|t| t["status"] == "running" || t["updatedAt"].as_str().is_none_or(|u| u > cutoff.as_str())) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn notify(text: &str) {
    exec::run(&["notify-send", "-a", "Power Hub", text], Duration::from_secs(10));
}

fn wait_idle() -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(env_u64("POWER_HUB_T3_IDLE_SECS", 6 * 3600));
    let mut quiet = idle()?;
    notify(READY);
    while !quiet {
        if Instant::now() >= deadline {
            return Err("T3 Code was never idle".into());
        }
        std::thread::sleep(poll());
        quiet = idle()?;
    }
    Ok(())
}

/// Waits for T3 Code to go idle, then quits and relaunches it, restoring its windows when `restore`.
/// A failed check never restarts it: the user is told to.
pub fn wait_idle_then_restart(restore: bool) -> Result<(), String> {
    if let Err(e) = wait_idle() {
        log(&format!("{NAME}: {e}"));
        notify(MANUAL);
        return Err(e);
    }
    let restore = restore && exec::run(&["hypr-persist", "save", "power-hub-t3"], Duration::from_secs(60)).0 == 0;
    exec::run(&["pkill", "-x", "t3code"], Duration::from_secs(10));
    // The old instance holds the single-instance lock until it has quit.
    for _ in 0..50 {
        if exec::run(&["pkill", "-0", "-x", "t3code"], Duration::from_secs(5)).0 != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    crate::resume::detach(&["uwsm-app", "--", &appimage()]);
    if restore {
        // Gives the new window time to appear before it is placed.
        std::thread::sleep(poll());
        exec::run(&["hypr-persist", "restore", "power-hub-t3", "--class", "^t3code$"], Duration::from_secs(120));
    }
    Ok(())
}
