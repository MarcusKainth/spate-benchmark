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

pub mod report;
