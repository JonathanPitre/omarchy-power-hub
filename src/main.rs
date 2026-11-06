// Later stages consume the shared modules; keep them compiled and warning-free meanwhile.
#![allow(dead_code)]

mod exec;
mod hypr;
mod paths;
mod system;

use std::io::{self, Write};

/// Stdout that never panics: a closed or full pipe must not abort the process.
fn out(line: impl std::fmt::Display) {
    let _ = writeln!(io::stdout(), "{line}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gpu") => out(system::gpu_status()),
        _ => {
            eprintln!("usage: power-hub gpu");
            std::process::exit(2);
        }
    }
}
