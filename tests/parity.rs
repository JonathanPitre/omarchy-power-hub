//! Replays the recorded Python results in `tests/golden` against the binary, on the same fake systems.
#![cfg(feature = "fixture-root")]

#[allow(dead_code)] // The write classes use the rest.
#[path = "parity/support.rs"]
mod support;

#[path = "parity/read.rs"]
mod read;
