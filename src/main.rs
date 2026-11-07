mod charge;
mod config;
mod data;
mod exec;
mod hypr;
mod idle;
mod names;
mod paths;
mod power;
mod settings;
mod state;
mod system;
mod travel;

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("status") => out(system::status()),
        Some("gpu") => out(system::gpu_status()),
        Some("data") => out(data::collect()),
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
        Some("boost-check") => {
            let (code, msg) = charge::boost_check();
            out(msg);
            std::process::exit(code);
        }
        _ => {
            eprintln!("usage: power-hub status|data|config|ac|drain-to|gpu|helper-needed|led-sudo-needed|battery-history|set|profile|boost-check|idle-wake-poll|displays sync");
            std::process::exit(2);
        }
    }
}
