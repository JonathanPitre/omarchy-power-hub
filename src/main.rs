mod action;
mod charge;
mod config;
mod data;
mod exec;
mod hypr;
mod idle;
mod monitor;
mod names;
mod paths;
mod power;
mod settings;
mod state;
mod system;
mod travel;
mod tweaks;

use std::io::{self, Write};

/// Stdout that never panics: a closed or full pipe must not abort the process.
fn out(line: impl std::fmt::Display) {
    let _ = writeln!(io::stdout(), "{line}");
}

/// Print the error, or `ok` without one, and exit 1 or 0.
fn report(err: String) -> ! {
    out(if err.is_empty() { "ok" } else { &err });
    std::process::exit(i32::from(!err.is_empty()))
}

pub(crate) fn on(arg: &str) -> bool {
    matches!(arg, "1" | "on" | "true")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("status") => out(system::status()),
        Some("gpu") => out(system::gpu_status()),
        Some("data") => out(data::collect()),
        Some("action") => action::run(&args[1..]).into_iter().for_each(out),
        Some("config") => {
            let cfg = config::load();
            out(serde_json::json!({"side": power::active_side(&cfg), "config": cfg}));
        }
        Some("drain-to") => out(power::drain_to(&config::load())),
        Some("ac") => out(u8::from(!power::on_battery())),
        Some("helper-needed") => out(if charge::helper_needed() { "needed" } else { "ok" }),
        Some("led-sudo-needed") => out(system::led_sudo_needed()),
        Some("battery-history") => out(system::battery_history()),
        Some("idle-wake-poll") => out(idle::poll(matches!(args.get(1).map(String::as_str), Some("blanked" | "off")))),
        Some("displays" | "dpms") if args.get(1).map(String::as_str) == Some("sync") => match idle::sync() {
            Some(result) => out(result),
            None => std::process::exit(1),
        },
        Some("set") if args.len() >= 4 => report(settings::set_key(&args[1], &args[2], &args[3])),
        Some("profile") if args.len() >= 2 => report(settings::set_key(power::active_side(&config::load()), "profile", &args[1])),
        Some("charge") if args.len() >= 2 => report(charge::charge_verb(&args[1])),
        Some("boost") if args.len() >= 2 => report(charge::boost(on(&args[1]))),
        Some("health") if args.len() >= 2 => report(charge::set_health(on(&args[1]))),
        Some("tweak") if args.len() >= 3 => report(tweaks::apply_tweak(&args[1], on(&args[2]), args.get(3).is_some_and(|a| matches!(a.as_str(), "1" | "persist" | "pin")))),
        Some("quick-dim") => match args.get(1).map(String::as_str).filter(|a| args.len() == 2 && matches!(*a, "on" | "off")) {
            Some(arg) => report(travel::quick_dim(arg == "on").unwrap_or_else(|e| format!("Quick dim could not save its recovery state: {e}"))),
            None => {
                out("usage: quick-dim on|off");
                std::process::exit(2);
            }
        },
        Some("travel") => match args.get(1).map(String::as_str).filter(|a| args.len() == 2 && matches!(*a, "on" | "off")) {
            Some(arg) => report(travel::travel(arg == "on").unwrap_or_else(|e| format!("Travel operation could not save its recovery state: {e}"))),
            None => {
                out("usage: travel on|off");
                std::process::exit(2);
            }
        },
        Some("apply") => report(settings::apply()),
        Some("apply-profile") => report(settings::apply_active().unwrap_or_else(|e| e.to_string())),
        Some("apply-tweaks") => report(tweaks::apply_saved_tweaks()),
        Some("unpin") if args.len() >= 2 => report(tweaks::unpin(&args[1])),
        Some("daylight") if args.len() >= 2 => report(tweaks::daylight(args[1] == "on")),
        Some("cpu") if args.len() >= 3 => report(tweaks::cpu(&args[1], &args[2])),
        Some("helper-install") => {
            let (code, msg) = charge::helper_install();
            out(msg);
            std::process::exit(code);
        }
        Some("boost-check") => {
            let (code, msg) = charge::boost_check();
            out(msg);
            std::process::exit(code);
        }
        _ => {
            eprintln!("usage: power-hub status|data|action|config|ac|drain-to|gpu|helper-needed|led-sudo-needed|battery-history|set|profile|charge|boost|health|quick-dim|travel|tweak|apply|apply-profile|apply-tweaks|unpin|daylight|cpu|helper-install|boost-check|idle-wake-poll|displays sync");
            std::process::exit(2);
        }
    }
}
