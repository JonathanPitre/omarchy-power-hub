use std::env;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// Absolute system path; the test sandbox prefixes it with `$POWER_HUB_ROOT`.
pub fn root(abs: &str) -> PathBuf {
    #[cfg(feature = "fixture-root")]
    if let Some(r) = env::var_os("POWER_HUB_ROOT") {
        return PathBuf::from(r).join(abs.trim_start_matches('/'));
    }
    PathBuf::from(abs)
}

pub fn bin_dir() -> PathBuf {
    #[cfg(feature = "fixture-root")]
    if let Some(b) = env::var_os("POWER_HUB_BIN") {
        return PathBuf::from(b);
    }
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_default()
}

pub fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn runtime_dir() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => PathBuf::from(d),
        None => {
            let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
            PathBuf::from(format!("/run/user/{uid}"))
        }
    }
}

pub fn plugin_run_dir() -> PathBuf {
    runtime_dir().join("omarchy-power-hub")
}

pub fn cfg_file() -> PathBuf {
    home().join(".config/omarchy/omarchy-power-hub.json")
}

pub fn tweaks_file() -> PathBuf {
    home().join(".config/omarchy/power-hub-tweaks.json")
}

pub fn ops_priv() -> PathBuf {
    bin_dir().join("ops-priv")
}

pub fn helper_dir() -> PathBuf {
    root("/usr/local/libexec/omarchy-power-hub")
}

pub fn shell_file() -> PathBuf {
    home().join(".config/omarchy/shell.json")
}

pub fn bindings_file() -> PathBuf {
    home().join(".config/hypr/bindings.lua")
}

pub fn profile_state_dir() -> PathBuf {
    match env::var_os("XDG_STATE_HOME") {
        Some(base) if !base.is_empty() => PathBuf::from(base),
        _ => home().join(".local/state"),
    }
    .join("omarchy/powerprofiles")
}
