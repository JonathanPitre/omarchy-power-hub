//! Replays the recorded Python results in `tests/golden` against the binary, on the same fake systems.
#![cfg(feature = "fixture-root")]

#[path = "parity/support.rs"]
mod support;

#[path = "parity/read.rs"]
mod read;

#[path = "parity/write.rs"]
mod write;

#[path = "parity/energy.rs"]
mod energy;

#[path = "parity/risky.rs"]
mod risky;

/// A golden no test names is never replayed, and the exit audit in `support` cannot see it.
#[test]
fn every_golden_is_replayed_by_a_test() {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let sources: String = std::fs::read_dir(tests.join("parity")).unwrap().flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "rs"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
    let unused: Vec<String> = std::fs::read_dir(tests.join("golden")).unwrap().flatten()
        .filter_map(|e| e.path().file_stem().map(|n| n.to_string_lossy().into_owned()))
        .filter(|name| !sources.contains(&format!("\"{name}\""))).collect();
    assert!(unused.is_empty(), "goldens no test replays: {unused:?}");
}
