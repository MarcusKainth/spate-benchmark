//! The Spate arm's library half.
//!
//! This crate is a lib *and* a bin, and the split is load-bearing rather than
//! stylistic. `[[bin]]` carries `test = false`, so a `#[cfg(test)]` module inside
//! `main.rs` would never be compiled or run — the tests would be silently dead.
//! The flatten in [`rows`] is logic the published numbers depend on being
//! correct, so it lives here where `cargo test` actually reaches it, and
//! `main.rs` stays a thin shell around it.

pub mod rows;
