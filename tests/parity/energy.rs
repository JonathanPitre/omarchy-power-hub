//! Quick dim and Travel, ported from `tests/test_parity.py` class by class under the same test names.
use crate::read::{DIM_FILE, TRAVEL_FILE, TWEAKS_FILE, laptop_row, merged, parsed, travel_snapshot};
use crate::support::{Hypr, Replay, Sandbox, executable, replay};
use crate::write::{BOTH, OLDER, Restore, Setup, Tweaks, checked, checked_each, chmod, display_only, no_leftovers, read, root_user};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

const DIM_ON: &str = r#"{"on": true, "monitor": "eDP-1", "brightness": 70, "error": ""}"#;
const FAIL: &str = "echo boom; false";
const SAVER_DENIED: &str = r#"if [ "$v" = power-saver ]; then echo "authorization denied"; false; else echo $v; fi"#;
const CAP_REFUSED: &str = r#"if [ "${v%\%}" = 40 ]; then echo boom; false; else echo ${v%\%}; fi"#;
const DEFAULT_SET: &str = r"echo ${v%\%}";
const SAVE_FAILED: &str = "Travel operation could not save its recovery state: ";

/// Brightness `start`; a set runs `set_body` with `v` the requested value and stores what it prints.
pub(crate) struct Dim {
    pub start: u32,
    pub state: Option<String>,
    pub travel: Option<String>,
    pub monitors: Vec<Value>,
    pub set_body: String,
}

impl Default for Dim {
    fn default() -> Self {
        Self { start: 80, state: None, travel: None, monitors: vec![laptop_row()], set_body: DEFAULT_SET.into() }
    }
}

impl Setup for Dim {
    fn setup(&self, s: &Sandbox) {
        s.monitors(&self.monitors);
        let state_file = s.base.join("brightness.state");
        fs::write(&state_file, format!("{}\n", self.start)).unwrap();
        let (state_file, body) = (state_file.display(), &self.set_body);
        s.stateful("omarchy", &format!("case \"$*\" in\n  *--no-osd*) v=\"${{@: -1}}\"; out=$({body}) || {{ echo \"$out\"; exit 1; }}; echo \"$out\" > {state_file};;\n  *\"brightness display --monitor\"*) cat {state_file};;\n  *) exit 1;;\nesac"));
        for (path, value) in [(DIM_FILE, &self.state), (TRAVEL_FILE, &self.travel)] {
            if let Some(value) = value {
                s.write(path, value);
            }
        }
    }
}

fn dim_state(text: &str) -> Dim {
    Dim { state: Some(text.into()), ..Dim::default() }
}

fn read_stub(code: i32, out: &'static str) -> impl Fn(&Sandbox) {
    move |s| s.stub("omarchy", "brightness display --monitor", code, out)
}

mod quick_dim_parity {
    use super::*;

    const ON: &[&str] = &["quick-dim", "on"];
    const OFF: &[&str] = &["quick-dim", "off"];

    #[test]
    fn test_on_caps_at_40_and_off_restores() {
        for start in [80, 30, 40, 0] {
            checked("QuickDimParity.test_on_caps_at_40_and_off_restores", &Dim { start, ..Dim::default() }, &[ON, OFF], BOTH);
        }
    }

    #[test]
    fn test_already_on_or_off_changes_nothing() {
        const T: &str = "QuickDimParity.test_already_on_or_off_changes_nothing";
        checked(T, &Dim { start: 40, state: Some(DIM_ON.into()), ..Dim::default() }, &[ON], BOTH);
        checked(T, &Dim::default(), &[OFF], BOTH);
    }

    #[test]
    fn test_refused_while_travel_is_on() {
        let setup = Dim { travel: Some(travel_snapshot(json!({})).to_string()), ..Dim::default() };
        checked("QuickDimParity.test_refused_while_travel_is_on", &setup, &[ON, OFF], BOTH);
    }

    #[test]
    fn test_no_laptop_display() {
        for monitors in [vec![json!({"name": "DP-1"})], vec![]] {
            checked("QuickDimParity.test_no_laptop_display", &Dim { monitors, ..Dim::default() }, &[ON], BOTH);
        }
    }

    #[test]
    fn test_unreadable_brightness() {
        for (code, out) in [(1, ""), (0, "abc\n"), (0, "101\n"), (0, "")] {
            let setup = move |s: &Sandbox| {
                Dim::default().setup(s);
                read_stub(code, out)(s);
            };
            checked("QuickDimParity.test_unreadable_brightness", &setup, &[ON], BOTH);
        }
    }

    #[test]
    fn test_failing_set_keeps_the_restore_point_with_the_error() {
        const T: &str = "QuickDimParity.test_failing_set_keeps_the_restore_point_with_the_error";
        checked(T, &Dim { set_body: FAIL.into(), ..Dim::default() }, &[ON, OFF], BOTH);
        checked(T, &Dim { start: 70, state: Some(DIM_ON.into()), set_body: FAIL.into(), ..Dim::default() }, &[OFF], BOTH);
    }

    #[test]
    fn test_set_that_misses_by_two_is_an_error_and_by_one_is_fine() {
        const T: &str = "QuickDimParity.test_set_that_misses_by_two_is_an_error_and_by_one_is_fine";
        checked(T, &Dim { set_body: r"echo $((${v%\%}+2))".into(), ..Dim::default() }, &[ON], BOTH);
        checked(T, &Dim { set_body: r"echo $((${v%\%}+1))".into(), ..Dim::default() }, &[ON], BOTH);
        checked(T, &Dim { start: 40, state: Some(DIM_ON.into()), set_body: "echo 10".into(), ..Dim::default() }, &[OFF], BOTH);
    }

    #[test]
    fn test_invalid_saved_state_is_shown_and_kept() {
        for state in [r#"{"on": "yes"}"#, r#"{"on": true}"#, r#"{"on": true, "monitor": "e DP", "brightness": 5}"#,
                      r#"{"on": true, "monitor": "eDP-1", "brightness": 101}"#, "[1]", "{"] {
            checked("QuickDimParity.test_invalid_saved_state_is_shown_and_kept", &dim_state(state), &[ON, OFF], BOTH);
        }
    }

    #[test]
    fn test_usage() {
        for argv in [&["quick-dim"][..], &["quick-dim", "maybe"], &["quick-dim", "on", "x"]] {
            checked("QuickDimParity.test_usage", &Dim::default(), &[argv], BOTH);
        }
        let s = Sandbox::new();
        let r = s.rust(&["quick-dim", "maybe"]);
        assert_eq!((r.stdout.as_str(), r.code), ("usage: quick-dim on|off\n", 2));
    }

    #[test]
    fn test_unwritable_run_dir_never_dims() {
        fn attempt(prepare: fn(&Path), prefix: &str) -> (bool, i32, Vec<String>) {
            let s = Sandbox::new();
            Dim::default().setup(&s);
            let run = s.target("run/omarchy-power-hub");
            prepare(&run);
            let _restore = Restore(run);
            let r = s.rust(&["quick-dim", "on"]);
            (r.stdout.starts_with(prefix), r.code, r.calls.iter().filter(|c| c.contains("--no-osd")).cloned().collect())
        }
        assert_eq!(attempt(as_file, "Quick dim could not save its recovery state: "), (true, 1, vec![]));
        if !root_user() { // root ignores directory modes; the recorded run words this case on its own
            assert_eq!(attempt(read_only, "Could not save brightness restore point: "), (true, 1, vec![]));
        }
    }
}

fn as_file(run: &Path) {
    fs::create_dir_all(run.parent().unwrap()).unwrap();
    fs::write(run, "").unwrap();
}

fn read_only(run: &Path) {
    fs::create_dir_all(run).unwrap();
    fs::write(run.join("energy-controls.lock"), "").unwrap();
    chmod(run, 0o500);
}

fn panel() -> Value {
    merged(laptop_row(), json!({"x": 0, "y": 0, "scale": 1.25, "description": "BOE NE160"}))
}

fn modes(changes: Value) -> Value {
    merged(panel(), changes)
}

/// The refresh a `mode = "WxH@R"` eval asks for.
fn asked_refresh(lua: &str) -> Option<f64> {
    lua.match_indices("mode = \"").find_map(|(at, found)| {
        let (size, rate) = lua[at + found.len()..].split_once('"')?.0.split_once('@')?;
        let (w, h) = size.split_once('x')?;
        let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
        (digits(w) && digits(h) && !rate.is_empty() && rate.bytes().all(|b| b.is_ascii_digit() || b == b'.')).then(|| rate.parse().ok()).flatten()
    })
}

/// Hyprland applies a mode change: the laptop row reports the refresh the eval asked for.
fn follow_modes(hypr: &mut Hypr, lua: &str) {
    let Some(rate) = asked_refresh(lua) else { return };
    let mut rows: Vec<Value> = parsed(&hypr.replies["j/monitors"]).as_array().unwrap().clone();
    for row in &mut rows {
        if row["name"].as_str().is_some_and(|n| n.starts_with("eDP")) {
            row["refreshRate"] = json!(rate);
        }
    }
    hypr.monitors(&rows);
}

/// The Hyprland reply to each eval in turn, the last one repeating.
fn eval_replies(s: &Sandbox, replies: &[&str]) {
    let replies: Vec<String> = replies.iter().map(ToString::to_string).collect();
    let mut next = 0;
    s.hypr().on_eval = Some(Box::new(move |hypr, lua| {
        hypr.eval_reply = replies[next.min(replies.len() - 1)].clone();
        next += 1;
        follow_modes(hypr, lua);
    }));
}

type Body = Box<dyn Fn(&Sandbox) -> String>;

/// A laptop on battery; `set_body` and `profile_set` are bash run for each set.
pub(crate) struct TravelEnv {
    brightness: u32,
    profile: &'static str,
    names: Vec<&'static str>,
    rows: Vec<Value>,
    travel: Option<String>,
    set_body: Body,
    profile_set: Body,
    profile_get: String,
    replies: Vec<&'static str>,
    cfg: Value,
}

pub(crate) fn travel_env() -> TravelEnv {
    TravelEnv {
        brightness: 80, profile: "performance", names: vec!["power-saver", "balanced", "performance"], rows: vec![panel()], travel: None,
        set_body: Box::new(|_| DEFAULT_SET.into()), profile_set: Box::new(|_| "echo $v".into()), profile_get: "cat".into(),
        replies: vec!["ok"], cfg: json!({"ac": {"profile": "performance"}}),
    }
}

impl TravelEnv {
    pub fn brightness(self, brightness: u32) -> Self {
        Self { brightness, ..self }
    }

    pub fn profile(self, profile: &'static str) -> Self {
        Self { profile, ..self }
    }

    pub fn names(self, names: &[&'static str]) -> Self {
        Self { names: names.to_vec(), ..self }
    }

    pub fn rows(self, rows: Vec<Value>) -> Self {
        Self { rows, ..self }
    }

    pub fn travel(self, point: Value) -> Self {
        Self { travel: Some(point.to_string()), ..self }
    }

    pub fn travel_text(self, text: &str) -> Self {
        Self { travel: Some(text.into()), ..self }
    }

    pub fn set_body(self, body: &str) -> Self {
        let body = body.to_string();
        Self { set_body: Box::new(move |_| body.clone()), ..self }
    }

    pub fn set_body_of(self, body: impl Fn(&Sandbox) -> String + 'static) -> Self {
        Self { set_body: Box::new(body), ..self }
    }

    pub fn profile_set(self, body: &str) -> Self {
        let body = body.to_string();
        Self { profile_set: Box::new(move |_| body.clone()), ..self }
    }

    pub fn profile_set_of(self, body: impl Fn(&Sandbox) -> String + 'static) -> Self {
        Self { profile_set: Box::new(body), ..self }
    }

    pub fn profile_get(self, body: &str) -> Self {
        Self { profile_get: body.into(), ..self }
    }

    pub fn replies(self, replies: &[&'static str]) -> Self {
        Self { replies: replies.to_vec(), ..self }
    }

    pub fn cfg(self, cfg: Value) -> Self {
        Self { cfg, ..self }
    }
}

impl Setup for TravelEnv {
    fn setup(&self, s: &Sandbox) {
        s.profiles(&self.names);
        s.config(self.cfg.clone());
        s.discharging(70);
        Dim { start: self.brightness, travel: self.travel.clone(), monitors: self.rows.clone(), set_body: (self.set_body)(s), state: None }.setup(s);
        let state = s.base.join("profile.state");
        fs::write(&state, format!("{}\n", self.profile)).unwrap();
        let state = state.display();
        let get = if self.profile_get == "cat" { format!("cat {state}") } else { self.profile_get.clone() };
        let body = (self.profile_set)(s);
        s.stateful("powerprofilesctl", &format!("case \"$1\" in\n  get) {get};;\n  set) v=$2; out=$({body}) || {{ echo \"$out\"; exit 1; }}; echo \"$out\" > {state};;\n  *) exit 1;;\nesac"));
        eval_replies(s, &self.replies);
    }
}

pub(crate) fn tweaks_on(abm: &'static str, effects: (bool, bool)) -> impl Fn(&Sandbox) {
    move |s| {
        Tweaks::default().effects(effects.0, effects.1).setup(s);
        s.write("/sys/class/drm/card1-eDP-1/amdgpu/panel_power_savings", abm);
    }
}

/// `after(travel_env(), tweaks_on(abm, effects))`.
fn with_tweaks(env: TravelEnv, abm: &'static str, effects: (bool, bool)) -> impl Fn(&Sandbox) {
    move |s| {
        env.setup(s);
        tweaks_on(abm, effects)(s);
    }
}

fn malformed_pending_base() -> Value {
    let unavailable = json!({"available": false, "detail": "Unavailable"});
    json!({"on": true, "complete": false, "reduceCpu": true, "source": "battery", "profile": null, "brightness": null, "monitor": null,
           "cpu": {"turbo": null, "epp": []}, "error": "",
           "controls": {"profile": unavailable, "turbo": unavailable, "epp": unavailable, "brightness": unavailable, "refresh": unavailable}})
}

fn changed_calls(r: &crate::support::Outcome) -> Vec<String> {
    r.calls.iter().filter(|c| c.contains("--no-osd") || c.contains(" set ")).cloned().collect()
}

mod travel_parity {
    use crate::write::starting;
    use super::*;

    const ON: &[&str] = &["travel", "on"];
    const OFF: &[&str] = &["travel", "off"];
    const NO_RESTORE: Replay = Replay { stdout: false, ..BOTH };

    /// `both(..., inspect=no_leftovers)`.
    fn check(name: &str, setup: &impl Setup, steps: &[&[&str]]) -> (Sandbox, crate::support::Outcome) {
        checked_each(name, setup, steps, BOTH, &mut |s, _| no_leftovers(s))
    }

    #[test]
    fn test_internal_output_only_is_changed_and_restored() {
        check("TravelParity.test_internal_output_only_is_changed_and_restored", &travel_env(), &[ON, OFF]);
    }

    /// Expected values were recorded from the old implementation.
    #[test]
    fn test_quick_dim_and_travel_keep_nested_restore_points_independent() {
        let s = Sandbox::new();
        travel_env().brightness(82).setup(&s);
        let brightness = || fs::read_to_string(s.base.join("brightness.state")).unwrap().trim().to_string();
        assert_eq!(s.rust(&["quick-dim", "on"]).code, 0);
        assert_eq!(brightness(), "40");
        assert_eq!(s.rust(ON).code, 0);
        let refused = s.rust(&["quick-dim", "off"]);
        assert_eq!((refused.code, refused.stdout.as_str()), (1, "Turn off Travel mode before changing Quick dim\n"));
        assert_eq!(parsed(&read(&s, DIM_FILE))["on"], true);
        assert_eq!(s.rust(OFF).code, 0);
        assert_eq!(brightness(), "40");
        assert_eq!(s.rust(&["quick-dim", "off"]).code, 0);
        assert_eq!(brightness(), "82");
    }

    #[test]
    fn test_travel_sets_power_saver_and_leaves_cpu_boost_alone() {
        let s = Sandbox::new();
        travel_env().setup(&s);
        let boost = s.write("/sys/devices/system/cpu/cpufreq/boost", "1\n");
        let profile = || fs::read_to_string(s.base.join("profile.state")).unwrap().trim().to_string();
        assert_eq!(s.rust(ON).code, 0);
        assert_eq!((profile().as_str(), fs::read_to_string(&boost).unwrap().as_str()), ("power-saver", "1\n"));
        // A power source change keeps Power saver while Travel is on.
        fs::write(s.supply("BAT0").join("status"), "Charging\n").unwrap();
        fs::write(s.base.join("profile.state"), "performance\n").unwrap();
        s.rust(&["apply"]);
        assert_eq!(profile(), "power-saver");
        let off = s.rust(OFF);
        assert_eq!(off.code, 0);
        assert_eq!(profile(), "performance");
        assert!(["sudo", "pkexec", "systemctl"].iter().all(|tool| starting(&off, tool).is_empty()), "{:?}", off.calls);
        assert_eq!(starting(&off, "powerprofilesctl set"), ["powerprofilesctl set performance"]);
        assert_eq!(fs::read_to_string(&boost).unwrap(), "1\n");
    }

    #[test]
    fn test_already_dim_display_stays() {
        check("TravelParity.test_already_dim_display_stays", &travel_env().brightness(30), &[ON, OFF]);
    }

    #[test]
    fn test_refresh_needs_the_same_resolution_and_never_rises() {
        let other_sizes = json!(["1920x1200@165.00Hz", "1920x1080@60.00Hz", "2560x1600@50.00Hz"]);
        let rotated = json!(["1920x1200@165.00Hz", "1200x1920@30.00Hz", "1920x1200@59.94Hz"]);
        let rows = [modes(json!({"availableModes": other_sizes})), modes(json!({"transform": 1, "availableModes": rotated})),
                    modes(json!({"refreshRate": 50.0, "availableModes": ["1920x1200@50.00Hz"]})),
                    modes(json!({"refreshRate": 60.0, "availableModes": ["1920x1200@48.00Hz"]})), modes(json!({"transform": 9}))];
        for row in rows {
            check("TravelParity.test_refresh_needs_the_same_resolution_and_never_rises", &travel_env().rows(vec![row]), &[ON, OFF]);
        }
    }

    #[test]
    fn test_omarchys_remembered_profiles_are_held_then_restored() {
        let cfg = json!({"ac": {"profile": "performance"}, "battery": {"profile": "balanced"}});
        check("TravelParity.test_omarchys_remembered_profiles_are_held_then_restored", &travel_env().cfg(cfg.clone()), &[ON, OFF]);
        let s = Sandbox::new();
        travel_env().cfg(cfg).setup(&s);
        s.rust(ON);
        let held: Vec<String> = ["ac", "battery"].map(|side| fs::read_to_string(s.home.join(".local/state/omarchy/powerprofiles").join(side)).unwrap()).into();
        assert_eq!(held, ["power-saver\n"; 2]);
    }

    #[test]
    fn test_abm_and_effects_are_applied_then_restored() {
        check("TravelParity.test_abm_and_effects_are_applied_then_restored", &with_tweaks(travel_env(), "0\n", (true, true)), &[ON, OFF]);
    }

    #[test]
    fn test_tweaks_it_did_not_turn_on_stay() {
        const T: &str = "TravelParity.test_tweaks_it_did_not_turn_on_stay";
        check(T, &with_tweaks(travel_env(), "3\n", (false, false)), &[ON, OFF]);
        check(T, &with_tweaks(travel_env(), "0\n", (true, false)), &[ON, OFF]);
    }

    #[test]
    fn test_tweak_that_cannot_be_turned_on_is_not_restored() {
        let env = travel_env();
        let setup = move |s: &Sandbox| {
            env.setup(s);
            tweaks_on("0\n", (false, false))(s); // abm is the only target, so nothing is saved after it is dropped
            s.stub("pkexec", "", 1, "denied");
        };
        check("TravelParity.test_tweak_that_cannot_be_turned_on_is_not_restored", &setup, &[ON, OFF]);
    }

    #[test]
    fn test_unreadable_profile_is_skipped() {
        const T: &str = "TravelParity.test_unreadable_profile_is_skipped";
        check(T, &travel_env().rows(vec![]).profile_get("echo profile unavailable; exit 1"), &[ON]);
        check(T, &travel_env().profile_get("echo profile unavailable; exit 1"), &[ON, OFF]);
    }

    #[test]
    fn test_without_a_power_saver_profile_only_the_display_changes() {
        const T: &str = "TravelParity.test_without_a_power_saver_profile_only_the_display_changes";
        check(T, &travel_env().names(&["balanced", "performance"]), &[ON, OFF]);
        check(T, &travel_env().names(&["balanced", "performance"]).rows(vec![]), &[ON]);
    }

    #[test]
    fn test_no_controls() {
        let env = || travel_env().rows(vec![]).names(&["balanced"]);
        check("TravelParity.test_no_controls", &env(), &[ON]);
        let s = Sandbox::new();
        env().setup(&s);
        assert_eq!(s.rust(ON).stdout, "No supported battery-saving controls\n");
    }

    #[test]
    fn test_failing_power_saver_rolls_back_to_off_with_the_error() {
        const T: &str = "TravelParity.test_failing_power_saver_rolls_back_to_off_with_the_error";
        check(T, &travel_env().profile_set(SAVER_DENIED), &[ON]);
        check(T, &travel_env().profile_set(SAVER_DENIED).rows(vec![]), &[ON]);
    }

    #[test]
    fn test_brightness_failing_after_the_profile_rolls_both_back() {
        check("TravelParity.test_brightness_failing_after_the_profile_rolls_both_back", &travel_env().set_body(CAP_REFUSED), &[ON, OFF]);
    }

    #[test]
    fn test_brightness_that_misses_the_cap_is_unverified() {
        check("TravelParity.test_brightness_that_misses_the_cap_is_unverified", &travel_env().set_body(r"echo $((${v%\%}+2))"), &[ON]);
    }

    #[test]
    fn test_monitor_change_failing_rolls_back() {
        const T: &str = "TravelParity.test_monitor_change_failing_rolls_back";
        check(T, &travel_env().replies(&["error: nope", "ok"]), &[ON]);
        check(T, &travel_env().replies(&["error: nope"]), &[ON, OFF]);
    }

    #[test]
    fn test_monitor_change_failing_on_a_travel_off_retry() {
        check("TravelParity.test_monitor_change_failing_on_a_travel_off_retry", &travel_env().replies(&["ok", "error: nope", "ok"]), &[ON, OFF, OFF]);
    }

    #[test]
    fn test_failed_restore_stays_incomplete_then_retries() {
        let counted = |s: &Sandbox| {
            let n = s.base.join("sets").display().to_string();
            format!("n=$(cat {n} 2>/dev/null || echo 0); echo $((n+1)) > {n}; if [ $n = 1 ]; then echo restore failed; false; else echo ${{v%\\%}}; fi")
        };
        check("TravelParity.test_failed_restore_stays_incomplete_then_retries", &travel_env().set_body_of(counted), &[ON, OFF, OFF]);
    }

    #[test]
    fn test_restore_that_cannot_set_the_profile_stays_incomplete() {
        const T: &str = "TravelParity.test_restore_that_cannot_set_the_profile_stays_incomplete";
        let deny_restore = r#"if [ "$v" = power-saver ]; then echo $v; else echo "authorization denied"; false; fi"#;
        check(T, &travel_env().profile_set(deny_restore), &[ON, OFF]);
        check(T, &travel_env().profile_set(deny_restore).set_body(CAP_REFUSED), &[ON]);
    }

    #[test]
    fn test_unwritable_tweaks_file_keeps_effects_owned() {
        if root_user() {
            return; // root ignores file modes
        }
        let setup = |s: &Sandbox| {
            travel_env().setup(s);
            tweaks_on("0\n", (true, true))(s);
            s.write(TWEAKS_FILE, "{}");
            chmod(&s.target(TWEAKS_FILE), 0o400);
        };
        checked_each("TravelParity.test_unwritable_tweaks_file_keeps_effects_owned", &setup, &[ON, OFF], NO_RESTORE, &mut |s, _| no_leftovers(s));
        let s = Sandbox::new();
        setup(&s);
        let r = s.rust(ON);
        assert_eq!(r.code, 1);
        assert!(r.stdout.starts_with(SAVE_FAILED));
        assert!(parsed(&read(&s, TRAVEL_FILE))["pending"].as_array().unwrap().contains(&json!("tweak:effects")));
    }

    #[test]
    fn test_tweaks_file_that_turns_read_only_before_travel_off() {
        if root_user() {
            return; // root ignores file modes
        }
        let setup = |s: &Sandbox| {
            travel_env().setup(s);
            tweaks_on("0\n", (true, true))(s);
        };
        let mut locked = false;
        checked_each("TravelParity.test_tweaks_file_that_turns_read_only_before_travel_off", &setup, &[ON, OFF], NO_RESTORE, &mut |s: &Sandbox, _: &_| {
            no_leftovers(s);
            if s.target(TRAVEL_FILE).exists() && !locked {
                chmod(&s.target(TWEAKS_FILE), 0o400);
                locked = true;
            }
        });
        let s = Sandbox::new();
        setup(&s);
        s.rust(ON);
        chmod(&s.target(TWEAKS_FILE), 0o400);
        let r = s.rust(OFF);
        let point = parsed(&read(&s, TRAVEL_FILE));
        assert_eq!((r.code, &point["error"], point["pending"].as_array().unwrap().contains(&json!("tweak:effects"))), (1, &json!(""), true));
        assert!(r.stdout.starts_with(SAVE_FAILED));
    }

    #[test]
    fn test_malformed_state_is_kept_as_it_is() {
        let source = travel_snapshot(json!({"source": []})).to_string();
        for raw in ["{not json", "[1]", r#"{"on": "yes"}"#, &source] {
            check("TravelParity.test_malformed_state_is_kept_as_it_is", &travel_env().travel_text(raw), &[ON, OFF]);
        }
    }

    #[test]
    fn test_malformed_pending_tokens_are_kept_as_they_are() {
        const T: &str = "TravelParity.test_malformed_pending_tokens_are_kept_as_they_are";
        for pending in [json!(["epp:0"]), json!(["refresh"]), json!(["unknown"])] {
            check(T, &travel_env().travel(merged(malformed_pending_base(), json!({"pending": pending}))), &[OFF]);
        }
        for changes in [json!({"source": []}), json!({"cpu": {"turbo": null, "epp": [{"cpu": 0, "before": [], "target": null}]}})] {
            check(T, &travel_env().travel(merged(merged(malformed_pending_base(), changes), json!({"pending": []}))), &[OFF]);
        }
        let point = merged(malformed_pending_base(), json!({"complete": true, "reduceCpu": false, "pending": [], "brightness": {"monitor": "eDP-1", "before": 80}}));
        check(T, &travel_env().travel(point), &[OFF]);
    }

    #[test]
    fn test_a_point_with_nothing_pending_but_the_profile_restores_it() {
        let env = travel_env().travel(merged(malformed_pending_base(), json!({"pending": []}))).profile("power-saver");
        check("TravelParity.test_a_point_with_nothing_pending_but_the_profile_restores_it", &env, &[OFF]);
    }

    #[test]
    fn test_display_only_source_transition() {
        check("TravelParity.test_display_only_source_transition", &travel_env().names(&["balanced"]).travel(display_only()), &[OFF]);
    }

    #[test]
    fn test_already_on_and_already_off_write_nothing() {
        const T: &str = "TravelParity.test_already_on_and_already_off_write_nothing";
        check(T, &travel_env().travel(travel_snapshot(json!({}))), &[ON]);
        check(T, &travel_env(), &[OFF]);
    }

    #[test]
    fn test_bad_arguments() {
        for argv in [&["travel"][..], &["travel", "maybe"], &["travel", "on", "nonsense"], &["travel", "off", "--reduce-cpu"]] {
            check("TravelParity.test_bad_arguments", &travel_env(), &[argv]);
        }
        let s = Sandbox::new();
        let r = s.rust(&["travel", "maybe"]);
        assert_eq!((r.stdout.as_str(), r.code), ("usage: travel on|off\n", 2));
    }

    #[test]
    fn test_older_restore_points_ask_for_a_reboot() {
        let legacy = json!({"on": true, "brightness": 80, "monitors": [{"name": "eDP-1", "width": 1920, "height": 1080, "refresh": 165, "x": 0, "y": 0, "scale": 1}]});
        let epp = travel_snapshot(json!({"cpu": {"turbo": null, "epp": [{"cpu": 0, "before": "power", "target": null}]},
                                         "pending": ["profile", "epp:0", "brightness", "refresh"]}));
        for point in [legacy, travel_snapshot(json!({})), epp] {
            let s = Sandbox::new();
            travel_env().travel(point).setup(&s);
            let before = s.snapshot(None);
            let r = s.rust(OFF);
            assert_eq!((r.stdout.as_str(), r.code), (format!("{OLDER}\n").as_str(), 1));
            assert_eq!(s.snapshot(None), before);
            assert!(changed_calls(&r).is_empty());
            assert!(r.evals.is_empty());
        }
    }

    #[test]
    fn test_restore_point_exists_before_the_first_change() {
        let s = Sandbox::new();
        let probe = |name: &'static str, tail: &'static str| move |s: &Sandbox| {
            format!("[ -e {base}/{name} ] || cp {run}/omarchy-power-hub/travel.json {base}/{name}; {tail}", base = s.base.display(), run = s.run.display())
        };
        travel_env().set_body_of(probe("seen-brightness", r"echo ${v%\%}")).profile_set_of(probe("seen-profile", "echo $v")).setup(&s);
        let mut mode = s.hypr().on_eval.take().unwrap();
        let eval_point: Arc<Mutex<Option<Value>>> = Arc::default();
        let (seen, point) = (eval_point.clone(), s.run.join("omarchy-power-hub/travel.json"));
        s.hypr().on_eval = Some(Box::new(move |hypr, lua| {
            seen.lock().unwrap().get_or_insert_with(|| parsed(&fs::read_to_string(&point).unwrap()));
            mode(hypr, lua);
        }));
        assert_eq!(s.rust(ON).code, 0);
        let seen = [eval_point.lock().unwrap().clone().unwrap(), parsed(&fs::read_to_string(s.base.join("seen-brightness")).unwrap()),
                    parsed(&fs::read_to_string(s.base.join("seen-profile")).unwrap())];
        for point in seen {
            assert_eq!((&point["on"], &point["complete"]), (&json!(true), &json!(false)));
        }
    }

    #[test]
    fn test_unwritable_run_dir_changes_nothing() {
        fn attempt(prepare: fn(&Path), prefix: &str) -> (bool, i32, Vec<String>, Vec<String>) {
            let s = Sandbox::new();
            travel_env().setup(&s);
            let run = s.target("run/omarchy-power-hub");
            prepare(&run);
            let _restore = Restore(run);
            let r = s.rust(ON);
            (r.stdout.starts_with(prefix), r.code, changed_calls(&r), r.evals)
        }
        assert_eq!(attempt(as_file, SAVE_FAILED), (true, 1, vec![], vec![]));
        if !root_user() {
            assert_eq!(attempt(read_only, "Could not save Travel restore point: "), (true, 1, vec![], vec![]));
        }
    }
}

/// Logs the restore point before every mutating external call, and kills the caller at the call named in `kill`.
const WRAP: &str = r#"#!/bin/bash
B=@BASE@
if [[ "$*" == *--no-osd* || "$1" == set || "$0" == *pkexec ]]; then
  n=$(( $(cat $B/cnt 2>/dev/null || echo 0) + 1 )); echo $n > $B/cnt
  { echo "== $n $(basename $0) $*"; cat $B/run/omarchy-power-hub/travel.json 2>/dev/null; } >> $B/trace
  k=$(cat $B/kill 2>/dev/null)
  if [ "$k" = "$n:before" ]; then kill -9 $PPID; exit 1; fi
fi
"$0.real" "$@"; rc=$?
if [ -n "$n" ] && [ "$k" = "$n:after" ]; then kill -9 $PPID; fi
exit $rc
"#;

unsafe extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

/// SIGKILL this sandbox's power-hub process, found by the sandbox root in its environment.
fn kill_travel(root: &Path) {
    let mark = format!("POWER_HUB_ROOT={}", root.display()).into_bytes();
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
        let (Ok(argv), Ok(environ)) = (fs::read(entry.path().join("cmdline")), fs::read(entry.path().join("environ"))) else { continue };
        let first = argv.split(|b| *b == 0).next().unwrap_or_default();
        if first.ends_with(b"power-hub") && environ.split(|b| *b == 0).any(|e| e == mark.as_slice()) {
            unsafe { kill(pid, 9) };
        }
    }
}

/// `setup` with every mutating external call (brightness set, profile set, pkexec, eval) logging travel.json to `trace`
/// first, and killing the calling process at the call named in `kill` ("N:before" or "N:after").
fn watched(s: &Sandbox, setup: &impl Setup) {
    setup.setup(s);
    for tool in ["omarchy", "powerprofilesctl", "pkexec"] {
        let stub = s.stubs.join(tool);
        fs::rename(&stub, s.stubs.join(format!("{tool}.real"))).unwrap();
        executable(&stub, &WRAP.replace("@BASE@", &s.base.display().to_string()));
    }
    let mut inner = s.hypr().on_eval.take().unwrap();
    let (base, run, root) = (s.base.clone(), s.run.clone(), s.root.clone());
    s.hypr().on_eval = Some(Box::new(move |hypr, lua| {
        let n = fs::read_to_string(base.join("cnt")).map_or(1, |c| c.trim().parse::<u32>().unwrap() + 1);
        fs::write(base.join("cnt"), n.to_string()).unwrap();
        let point = fs::read_to_string(run.join("omarchy-power-hub/travel.json")).unwrap_or_default();
        let trace = format!("== {n} eval {lua}\n{point}");
        fs::OpenOptions::new().create(true).append(true).open(base.join("trace")).unwrap().write_all(trace.as_bytes()).unwrap();
        let kill = fs::read_to_string(base.join("kill")).unwrap_or_default();
        if kill == format!("{n}:before") {
            kill_travel(&root);
            return;
        }
        inner(hypr, lua);
        if kill == format!("{n}:after") {
            kill_travel(&root);
        }
    }));
}

mod travel_recovery {
    use super::*;

    const ON: &[&str] = &["travel", "on"];
    const OFF: &[&str] = &["travel", "off"];

    /// Runs `steps`; `kill` = (step index, "N:before"|"N:after"). Returns the last step's mutating-call count.
    fn run_steps(name: &str, setup: &impl Setup, steps: &[&[&str]], kill: Option<(usize, String)>) -> usize {
        let s = Sandbox::new();
        watched(&s, setup);
        for (i, step) in steps.iter().enumerate() {
            fs::write(s.base.join("cnt"), "0").unwrap();
            fs::write(s.base.join("kill"), kill.as_ref().filter(|(at, _)| *at == i).map_or("", |(_, mode)| mode.as_str())).unwrap();
            replay(&s, name, step, BOTH);
        }
        fs::read_to_string(s.base.join("cnt")).unwrap().trim().parse().unwrap()
    }

    #[test]
    fn test_restore_point_at_every_mutating_call() {
        type Case = (&'static str, Box<dyn Fn(&Sandbox)>, Vec<&'static [&'static str]>);
        let env = |e: TravelEnv| -> Box<dyn Fn(&Sandbox)> { Box::new(move |s| e.setup(s)) };
        let cases: Vec<Case> = vec![
            ("plain", Box::new(with_tweaks(travel_env(), "0\n", (true, true))), vec![ON, OFF]),
            ("saver denied", env(travel_env().profile_set(SAVER_DENIED)), vec![ON]),
            ("cap refused", env(travel_env().set_body(CAP_REFUSED)), vec![ON, OFF]),
            ("mode fails, rollback works", env(travel_env().replies(&["error: nope", "ok"])), vec![ON]),
            ("mode fails, rollback fails", env(travel_env().replies(&["error: nope"])), vec![ON, OFF]),
            ("travel off retried", env(travel_env().replies(&["ok", "error: nope", "ok"])), vec![ON, OFF, OFF]),
            ("display-only point", env(travel_env().names(&["balanced"]).travel(display_only())), vec![OFF]),
        ];
        for (_, setup, steps) in cases {
            run_steps("TravelRecovery.test_restore_point_at_every_mutating_call", &setup, &steps, None);
        }
    }

    #[test]
    fn test_killed_travel_recovers_the_same() {
        const T: &str = "TravelRecovery.test_killed_travel_recovers_the_same";
        let setup = with_tweaks(travel_env(), "0\n", (true, true));
        let steps = [ON, OFF, OFF];
        for step in 0..2 {
            let calls = run_steps(T, &setup, &steps[..=step], None);
            for n in 1..=calls {
                for mode in ["before", "after"] {
                    run_steps(T, &setup, &steps, Some((step, format!("{n}:{mode}"))));
                }
            }
        }
    }
}
