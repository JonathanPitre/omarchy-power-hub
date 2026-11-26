use crate::config::truthy;
use crate::monitor::py_str;
use crate::names::safe_name_v;
use crate::state::py_int;
use crate::{config, exec, hypr, kblight, keyboard, paths, power, settings};
use serde_json::Value;
use std::fs::{self, OpenOptions};

/// Some monitor is enabled and every enabled one is lit.
pub fn awake(monitors: &[Value]) -> bool {
    let mut enabled = monitors.iter().filter(|m| !truthy(&m["disabled"])).peekable();
    enabled.peek().is_some() && enabled.all(|m| truthy(&m["dpmsStatus"]))
}

/// Enabled monitors are a mix of lit and dark.
pub fn partly_lit(monitors: &[Value]) -> bool {
    let lit = |want: bool| monitors.iter().any(|m| !truthy(&m["disabled"]) && truthy(&m["dpmsStatus"]) == want);
    lit(true) && lit(false)
}

/// Some enabled monitor is dark.
pub fn blanked(monitors: &[Value]) -> bool {
    monitors.iter().any(|m| !truthy(&m["disabled"]) && !truthy(&m["dpmsStatus"]))
}

/// Runs `f` holding the display-state lock; None when the lock cannot be taken.
pub fn with_state_lock<T>(f: impl FnOnce() -> T) -> Option<T> {
    let dir = paths::plugin_run_dir();
    fs::create_dir_all(&dir).ok()?;
    let lock = OpenOptions::new().write(true).create(true).truncate(true).open(dir.join("display-off.lock")).ok()?;
    lock.lock().ok()?;
    Some(f())
}

fn dpms(action: &str, name: &Value) {
    hypr::dispatch(&format!(
        r#"hl.dsp.dpms({{ action = "{action}", monitor = "{}" }})"#,
        py_str(name)
    ));
}

pub fn dismiss_screensaver() {
    exec::run(&["pkill", "-x", "ttfx"], hypr::WRITE_WAIT);
    exec::run(
        &["pkill", "-f", "[o]rg.omarchy.screensaver"],
        hypr::WRITE_WAIT,
    );
    hypr::eval_slow("hl.config({ cursor = { invisible = false } })");
}

/// `on` closes the screensaver and lights every monitor, `off` blanks every enabled one. Only DPMS: which heads exist and
/// where they sit is the monitor manager's job (hyprmoncfg here), so a head that drops off the bus while blanked is left
/// to it. The saver stays up behind dark screens: Omarchy reads it closing as the user returning and relights them.
pub fn set_displays(off: bool) {
    if !off {
        dismiss_screensaver();
    }
    for m in hypr::monitors_within(hypr::WRITE_WAIT) {
        if safe_name_v(&m["name"]) && !(off && truthy(&m["disabled"])) {
            dpms(if off { "disable" } else { "enable" }, &m["name"]);
        }
    }
}

/// Put the idle timers where blanking (or waking) needs them. A config that cannot be read must not stop the blank,
/// and a lock or screensaver that is not a number leaves the timers alone rather than turning auto-lock off.
fn idle_for_blank(blank: bool) {
    let cfg = config::load();
    let src = &cfg[power::active_side(&cfg)];
    if let (Some(_), Some(lock)) = (py_int(&src["screensaver"]), py_int(&src["lock"])) {
        let _ = settings::shell_idle(if blank { 0 } else { settings::effective_saver(src) }, lock);
    }
}

/// Keyboard and power-LED lights follow the displays; Auto sets the keys by the room on wake.
pub fn sync_lights(off: bool) {
    kblight::sync(off, keyboard::is_auto(&config::saved_prefs()));
}

/// `displays on|off`: the idle timers and the heads under the state lock, then the lights. None when the lock is unavailable.
pub fn displays(off: bool) -> Option<String> {
    with_state_lock(|| {
        idle_for_blank(off);
        set_displays(off);
    })?;
    sync_lights(off);
    Some("ok".into())
}
