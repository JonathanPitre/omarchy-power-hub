// Later stages consume the shared modules; keep them compiled and warning-free meanwhile.
#![allow(dead_code)]

mod exec;
mod hypr;
mod paths;
mod system;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gpu") => println!("{}", system::gpu_status()),
        _ => {
            eprintln!("usage: power-hub gpu");
            std::process::exit(2);
        }
    }
}
