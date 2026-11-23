use crate::paths;
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn socket_path() -> Option<PathBuf> {
    let base = paths::runtime_dir().join("hypr");
    let dir = match std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        Ok(sig) if !sig.is_empty() => base.join(sig),
        _ => fs::read_dir(&base)
            .ok()?
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max()?
            .1,
    };
    Some(dir.join(".socket.sock"))
}

/// One request/reply on the Hyprland command socket; None when it is unreachable.
pub fn request(cmd: &str) -> Option<String> {
    request_within(cmd, Duration::from_secs(2))
}

/// `request` that waits up to `timeout` for the reply, for writes whose effect Hyprland applies slowly.
fn request_within(cmd: &str, timeout: Duration) -> Option<String> {
    request_at(&socket_path()?, cmd, timeout)
}

fn request_at(path: &Path, cmd: &str, timeout: Duration) -> Option<String> {
    let mut s = UnixStream::connect(path).ok()?;
    let t = Some(timeout);
    s.set_read_timeout(t).ok()?;
    s.set_write_timeout(t).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

pub fn monitors() -> Vec<Value> {
    monitors_within(Duration::from_secs(2))
}

/// `monitors` for the read that verifies a mode change, which gets 4 s.
pub fn monitors_within(timeout: Duration) -> Vec<Value> {
    socket_path().map_or_else(Vec::new, |p| monitors_at(&p, timeout))
}

/// While a head reconnects Hyprland answers "Displays are still connecting" instead of JSON; ask again until `timeout`.
fn monitors_at(path: &Path, timeout: Duration) -> Vec<Value> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let reply = request_at(path, "j/monitors", left.max(Duration::from_millis(1)));
        if !reply.as_deref().is_some_and(|r| r.starts_with("Displays are still connecting")) || left < Duration::from_millis(250) {
            return reply.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or_default();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

pub fn devices() -> Value {
    request("j/devices")
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_else(|| json!({}))
}

pub fn cursor() -> String {
    request("cursorpos").unwrap_or_default().trim().to_string()
}

/// How long a display write may take to reply: Hyprland applies mode and power changes slowly.
pub const WRITE_WAIT: Duration = Duration::from_secs(4);

fn write_at(path: &Path, verb: &str, lua: &str) -> Option<String> {
    request_at(path, &format!("{verb} {lua}"), WRITE_WAIT)
}

/// `eval` that waits `WRITE_WAIT` for the reply.
pub fn eval_slow(lua: &str) {
    if let Some(p) = socket_path() {
        write_at(&p, "eval", lua);
    }
}

/// `hyprctl dispatch <lua>` over the socket, waiting `WRITE_WAIT`.
pub fn dispatch(lua: &str) {
    if let Some(p) = socket_path() {
        write_at(&p, "dispatch", lua);
    }
}

pub fn eval(lua: &str) {
    request(&format!("eval {lua}"));
}

fn eval_reply(reply: Option<String>) -> Result<(), String> {
    match reply {
        None => Err("Hyprland is not reachable".into()),
        Some(r) if r.starts_with("error") => Err(r),
        Some(_) => Ok(()),
    }
}

/// Run Lua in Hyprland, waiting up to 8 s; Err when the socket is unreachable or the reply starts with `error`.
pub fn eval_checked(lua: &str) -> Result<(), String> {
    eval_reply(request_within(&format!("eval {lua}"), Duration::from_secs(8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_held_for_three_seconds_still_arrives() {
        let path = std::env::temp_dir().join(format!("ph-slow-{}.sock", std::process::id()));
        let _ = fs::remove_file(&path);
        let server = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut conn, _) = server.accept().unwrap();
            let mut buf = [0; 64];
            let _ = conn.read(&mut buf);
            std::thread::sleep(Duration::from_secs(3));
            conn.write_all(b"ok").unwrap();
        });
        assert_eq!(request_at(&path, "eval x", Duration::from_secs(8)).as_deref(), Some("ok"));
        handle.join().ok();
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_reply_after_3_s_reaches_a_write() {
        for verb in ["eval", "dispatch"] {
            let path = std::env::temp_dir().join(format!("ph-write-{verb}-{}.sock", std::process::id()));
            let _ = fs::remove_file(&path);
            let server = std::os::unix::net::UnixListener::bind(&path).unwrap();
            let handle = std::thread::spawn(move || {
                let (mut conn, _) = server.accept().unwrap();
                let mut buf = [0; 64];
                let n = conn.read(&mut buf).unwrap();
                std::thread::sleep(Duration::from_secs(3));
                conn.write_all(b"ok").unwrap();
                String::from_utf8_lossy(&buf[..n]).into_owned()
            });
            assert_eq!(write_at(&path, verb, "x").as_deref(), Some("ok"));
            assert_eq!(handle.join().unwrap(), format!("{verb} x"));
            let _ = fs::remove_file(&path);
        }
    }

    /// Serves `replies` on `path`, one connection each.
    fn serve(path: &Path, replies: &'static [&'static str]) -> std::thread::JoinHandle<()> {
        let _ = fs::remove_file(path);
        let server = std::os::unix::net::UnixListener::bind(path).unwrap();
        std::thread::spawn(move || {
            for reply in replies {
                let (mut conn, _) = server.accept().unwrap();
                let mut buf = [0; 64];
                let _ = conn.read(&mut buf);
                conn.write_all(reply.as_bytes()).unwrap();
            }
        })
    }

    #[test]
    fn monitors_wait_out_displays_still_connecting() {
        let path = std::env::temp_dir().join(format!("ph-mons-{}.sock", std::process::id()));
        let busy = "Displays are still connecting; try again shortly. (monitors)";
        let handle = serve(&path, Box::leak(Box::new([busy, busy, r#"[{"name": "DP-11"}]"#])));
        assert_eq!(monitors_at(&path, Duration::from_secs(4)), vec![json!({"name": "DP-11"})]);
        handle.join().unwrap();
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn monitors_still_connecting_past_the_timeout_are_none() {
        let path = std::env::temp_dir().join(format!("ph-mons-late-{}.sock", std::process::id()));
        let busy = "Displays are still connecting; try again shortly. (monitors)";
        let handle = serve(&path, Box::leak(Box::new([busy; 3])));
        let started = std::time::Instant::now();
        assert!(monitors_at(&path, Duration::from_millis(600)).is_empty());
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(handle);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn eval_error_reply_is_an_err() {
        assert_eq!(eval_reply(Some("error: x".into())), Err("error: x".into()));
        assert!(eval_reply(Some("ok".into())).is_ok());
        assert!(eval_reply(None).is_err());
    }
}
