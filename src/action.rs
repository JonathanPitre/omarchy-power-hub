use crate::{config, data, paths, power, settings, tweaks};
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

fn on(arg: &str) -> bool {
    matches!(arg, "1" | "on" | "true")
}

/// The panel's one round trip: make the change, discard its result as the script's `>/dev/null` did, then return fresh data.
/// `install-sleep-tracking` opens a terminal instead and returns nothing.
pub fn run(args: &[String]) -> Option<Value> {
    let arg = |i: usize| args.get(i).map_or("", String::as_str);
    let _ = match arg(0) {
        "mode" => settings::set_key(power::active_side(&config::load()), "profile", arg(1)),
        verb @ ("turbo" | "epp") => tweaks::cpu(verb, arg(1)),
        verb @ ("wifi" | "usb" | "pcie" | "audio" | "abm" | "effects") => tweaks::apply_tweak(verb, on(arg(1)), false),
        "daylight" => tweaks::daylight(arg(1) == "on"),
        "persist" => tweaks::apply_tweak(arg(1), on(arg(2)), true),
        "unpersist" => tweaks::unpin(arg(1)),
        "install-sleep-tracking" => {
            install_sleep_tracking();
            return None;
        }
        _ => String::new(),
    };
    Some(data::collect())
}

/// Single-quoted for a shell, so any path survives as one word.
fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

fn install_sleep_tracking() {
    let repo = paths::bin_dir().parent().unwrap_or(Path::new("/")).join("extras/sleep-tracking/apply.sh");
    let user = paths::home().join(".config/omarchy/sleep-tracking/apply.sh");
    let script = if repo.exists() || !user.exists() { repo } else { user };
    // The launcher runs its one argument as a shell command line, so the path needs its own quoting.
    let _ = Command::new("setsid")
        .args(["omarchy-launch-floating-terminal-with-presentation", &format!("bash {}", quote(&script.to_string_lossy()))])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quote_survives_spaces_and_apostrophes() {
        assert_eq!(quote("/a b/it's"), r"'/a b/it'\''s'");
    }
}
