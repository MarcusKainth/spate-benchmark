//! The measurement harness for the Spate streaming ETL benchmark.
//!
//! Everything a published number depends on lives here, and nothing here depends
//! on any system under test. That direction of dependency is the point: a
//! competitor must be able to audit how they were measured without trusting the
//! code of the system they are being compared against.
//!
//! The modules split along the same seam as the repository:
//!
//! - [`report`] — the versioned record. What gets published.
//! - [`corpus`] — the deterministic data, as a pure function of `batch_id`.
//! - [`sampler`] — cgroup v2 measurement from outside the container under test.
//! - [`docker`], [`http`] — plumbing.
//!
//! `METHODOLOGY.md` is normative. Where this code and that document disagree,
//! the document is what the competitor implementations were written against, so
//! the code is wrong.

// The driver narrates progress on stderr by design: a 30-hour sweep that says
// nothing until it finishes is unusable.
#![allow(clippy::print_stderr, clippy::print_stdout)]

/// `key` from the environment, else `default`.
///
/// Deliberately rare in this crate: the measurement envelope comes from an
/// environment profile, not from ambient variables. This exists for the few
/// values that genuinely are ambient — local credentials, an image override.
pub fn env_str(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

pub mod corpus;
pub mod docker;
pub mod http;
pub mod kafka;
pub mod report;
pub mod sampler;
