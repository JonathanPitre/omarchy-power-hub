// Later stages consume the shared modules; keep them compiled and warning-free meanwhile.
#![allow(dead_code)]

mod charge;
mod config;
mod data;
mod exec;
mod hypr;
mod paths;
mod power;
mod system;
mod travel;

use std::io::{self, Write};

/// Stdout that never panics: a closed or full pipe must not abort the process.
fn out(line: impl std::fmt::Display) {
    let _ = writeln!(io::stdout(), "{line}");
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
        Some("boost-check") => {
            let (code, msg) = charge::boost_check();
            out(msg);
            std::process::exit(code);
        }
        _ => {
            eprintln!("usage: power-hub status|data|config|ac|drain-to|gpu|helper-needed|led-sudo-needed|battery-history|boost-check");
            std::process::exit(2);
        }
    }
}
