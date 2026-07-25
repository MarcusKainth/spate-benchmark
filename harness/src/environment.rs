//! Environment profiles: the unit of comparability for hardware.
//!
//! Records carry an `env_id` and the site never draws two environments on one
//! axis. That is why a profile is a committed file with a stable id rather than
//! a hostname — `Marcuss-MBP.kainth.co.uk` is not a hardware disclosure, cannot
//! be compared across machines, and tells a reader nothing they can reproduce
//! against.
//!
//! The profile also owns the **infrastructure envelope**, and that placement is
//! the fix for a specific failure. Previously the broker and ClickHouse CPU
//! caps came from environment variables: a runner script set one pair, the
//! driver's defaults declared another, and the written methodology stated a
//! third, while every recorded number stayed silent about which had been in
//! force. One source, applied and then read back from the running containers.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

/// A loaded environment profile and its content digest.
#[derive(Debug, Clone)]
pub struct Environment {
    /// The profile.
    pub spec: Profile,
    /// Hash of the file's bytes, recorded on every result so a later edit
    /// cannot retroactively re-describe runs that already happened.
    pub digest: String,
    /// Directory the profile was loaded from, for resolving relative paths.
    pub dir: PathBuf,
}

/// The profile's contents.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Stable identifier, equal to the file stem.
    pub id: String,
    /// Whether numbers from here are authoritative.
    pub class: Class,
    /// Hardware description, published on the site.
    pub host: Host,
    /// Shared infrastructure, identical for every arm.
    pub infra: Infra,
    /// Where the measured ceiling lives.
    pub ceiling: CeilingRef,
}

/// How much weight a reader should give numbers from this environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// Dedicated hardware; numbers stand on their own.
    Authoritative,
    /// A shared or virtualised host. The site renders its caveat banner from
    /// this value, not from a string match on the OS — so the banner disappears
    /// on its own when an authoritative environment is added, rather than
    /// having to be remembered.
    Indicative,
    /// Synthetic data for development. Never published.
    Fixture,
}

/// Hardware description.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub description: String,
    pub cpu: String,
    pub cores: u32,
    #[serde(default)]
    pub core_layout: String,
    pub memory: String,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub vm_cpus: u32,
    #[serde(default)]
    pub vm_memory: String,
    #[serde(default)]
    pub caveats: String,
}

/// Shared infrastructure, outside every arm's envelope.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Infra {
    /// Topic partition count. Bounds consume parallelism for every arm equally.
    pub partitions: i32,
    pub broker: Broker,
    pub clickhouse: ClickHouse,
}

/// The broker and its built-in registry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Broker {
    pub kind: String,
    pub image: String,
    pub cpus: String,
    pub memory: String,
    pub registry: String,
}

/// The ingestion target.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClickHouse {
    pub image: String,
    pub cpus: String,
    pub memory: String,
}

/// Where the measured ceiling for this environment lives.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CeilingRef {
    /// Path relative to the environments directory.
    pub file: String,
}

/// The measured consume ceiling, in **messages** per second.
#[derive(Debug, Clone, Deserialize)]
pub struct Ceiling {
    /// Messages per second the shared consume path sustained.
    ///
    /// Messages, not rows, deliberately: the ceiling is a property of the
    /// consume path, and rows per second depends on the fan-out factor. Storing
    /// rows would mean that changing `events_per_batch` silently invalidated
    /// this file — which is exactly the class of mistake that produced several
    /// retracted numbers already. The driver multiplies by the current fan-out
    /// itself.
    pub consume_msgs_per_s: u64,
}

/// The fraction of the measured ceiling above which an arm is infra-bound.
///
/// Above this we are measuring the shared consume path rather than the system,
/// and the run is recorded with `status: infra_bound` rather than published.
pub const HEADROOM_LIMIT: f64 = 0.70;

impl Environment {
    /// Loads the profile named `id` from `dir`.
    ///
    /// # Errors
    ///
    /// If the file is missing, does not parse, or its `id` disagrees with the
    /// filename — a mismatch would let two profiles claim the same identity and
    /// silently merge two hardware configurations into one comparison group.
    pub fn load(dir: &Path, id: &str) -> Result<Self, String> {
        let path = dir.join(format!("{id}.toml"));
        let src =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let spec: Profile = toml::from_str(&src).map_err(|e| format!("{}: {e}", path.display()))?;
        if spec.id != id {
            return Err(format!(
                "{}: declares id {:?} but is named {id:?}",
                path.display(),
                spec.id
            ));
        }
        let digest = short_digest(src.as_bytes());
        Ok(Self {
            spec,
            digest,
            dir: dir.to_path_buf(),
        })
    }

    /// The measured ceiling for this environment.
    ///
    /// # Errors
    ///
    /// If the referenced file is missing or does not parse.
    pub fn ceiling(&self) -> Result<Ceiling, String> {
        let path = self.dir.join(&self.spec.ceiling.file);
        let src =
            std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        serde_json::from_str(&src).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Digest over the **envelope-defining** subset of the infrastructure.
    ///
    /// Deliberately excludes versions. A ClickHouse patch release is soft
    /// provenance — recorded, rendered as a footnote — because refusing to
    /// compare across one would make the suite unusable. What splits a
    /// comparability group is a change in the *shape* of the infrastructure:
    /// CPU, memory, partitions, broker family.
    #[must_use]
    pub fn infra_digest(&self) -> String {
        let i = &self.spec.infra;
        short_digest(
            format!(
                "{}|{}|{}|{}|{}|{}",
                i.broker.kind,
                i.broker.cpus,
                i.broker.memory,
                i.clickhouse.cpus,
                i.clickhouse.memory,
                i.partitions
            )
            .as_bytes(),
        )
    }

    /// Whether results from here may be published.
    #[must_use]
    pub fn is_publishable(&self) -> bool {
        self.spec.class != Class::Fixture
    }
}

/// Twelve hex characters of SHA-256 — enough to identify, short enough to read.
fn short_digest(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_stable_and_short() {
        let a = short_digest(b"hello");
        assert_eq!(a.len(), 12);
        assert_eq!(a, short_digest(b"hello"));
        assert_ne!(a, short_digest(b"hello "));
    }

    #[test]
    fn the_headroom_limit_is_the_documented_seventy_percent() {
        // Named rather than inlined at the call site so the methodology and the
        // code cannot state different limits.
        assert!((HEADROOM_LIMIT - 0.70).abs() < f64::EPSILON);
    }
}
