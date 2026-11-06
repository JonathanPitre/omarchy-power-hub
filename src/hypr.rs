use crate::paths;
use serde_json::{Value, json};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

fn socket_path() -> Option<PathBuf> {
    let base = paths::runtime_dir().join("hypr");
    let dir = match std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        Ok(sig) if !sig.is_empty() => base.join(sig),
        _ => fs::read_dir(&base)
            .ok()?
            .flatten()
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max()?
            .1,
    };
    Some(dir.join(".socket.sock"))
}

/// One request/reply on the Hyprland command socket; None when it is unreachable.
pub fn request(cmd: &str) -> Option<String> {
    let mut s = UnixStream::connect(socket_path()?).ok()?;
    let t = Some(Duration::from_secs(2));
    s.set_read_timeout(t).ok()?;
    s.set_write_timeout(t).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

pub fn monitors() -> Vec<Value> {
    request("j/monitors")
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
