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

/// `monitors` for the read that verifies a mode change, which Python allows 4 s.
pub fn monitors_within(timeout: Duration) -> Vec<Value> {
    request_within("j/monitors", timeout)
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default()
}

pub fn devices() -> Value {
    request("j/devices")
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_else(|| json!({}))
}

pub fn cursor() -> String {
    request("cursorpos").unwrap_or_default().trim().to_string()
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

/// Run Lua in Hyprland, waiting up to 8 s as Python does; Err when the socket is unreachable or the reply starts with `error`.
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
    fn eval_error_reply_is_an_err() {
        assert_eq!(eval_reply(Some("error: x".into())), Err("error: x".into()));
        assert!(eval_reply(Some("ok".into())).is_ok());
        assert!(eval_reply(None).is_err());
    }
}
