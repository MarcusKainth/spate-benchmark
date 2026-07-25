//! The versioned record every measurement emits.
//!
//! One JSON object per line, appended to a file under `results/`. Forked from
//! `etl-rs/benchmarks/src/report.rs` at `f41280d51165` and immediately taken to
//! schema 2. **Fixes do not flow between the two copies.** That repository stays
//! on schema 1 for its two dozen self-comparison datasets, which have no system
//! under test, no environment registry and no comparability rules; sharing a
//! crate would force either a pointless migration there or a union schema
//! describing neither well.
//!
//! [`Metric`] carries its own `unit` and `higher_is_better`, which is the single
//! best property inherited from schema 1: a consumer plotting these records
//! cannot silently draw a lower-is-better quantity as a taller bar, because the
//! direction travels with the number rather than living in the plotting code.
//!
//! What schema 2 adds is provenance strong enough to publish:
//!
//! - [`Sut`] — *what was actually run*, including an image digest that is not
//!   optional, because version strings lie and digests do not.
//! - [`RunMeta::env_id`] — an interned hardware profile rather than a hostname.
//!   `Marcuss-MBP.kainth.co.uk` is not a hardware disclosure and cannot be
//!   compared across machines.
//! - [`RunMeta::harness_version`] / [`RunMeta::dataset_version`] — the two
//!   quantities that invalidate an entire result set.
//! - [`Status`] — so "we ran it and it failed the headroom gate" is
//!   distinguishable from "we never ran it".
//! - [`Report::superseded_by`] — retraction without deletion.

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Schema version of the emitted records. Bump on any breaking field change.
pub const SCHEMA_VERSION: u32 = 2;

/// Version of the measurement **protocol**.
///
/// Records with different values are not comparable, and the site refuses to
/// place them on one axis rather than averaging across the change.
///
/// Bump when the protocol changes in a way that **moves numbers**: the
/// steady-state detector, the drain protocol, sampler interval semantics, the
/// gate set, envelope enforcement. Do **not** bump for a log message, a
/// refactor, or a new field that no measurement depends on.
///
/// Hand-maintained rather than derived, deliberately. "Did this change move
/// numbers?" is a judgement; a content hash would answer yes to every typo fix
/// and shatter every comparability group in the archive. `METHODOLOGY.md`
/// carries a row per version and CI asserts the two stay in step.
pub const HARNESS_VERSION: u32 = 1;

/// Version of the **corpus**: the Avro schema, the ClickHouse DDL, and the
/// generator constants.
///
/// Derived rather than hand-maintained, because unlike the protocol this is
/// fully determined by files. `build.rs` hashes them, so a change to what the
/// data *is* cannot be made without the version moving.
pub const DATASET_VERSION: &str = env!("SPATE_BENCH_DATASET_VERSION");

/// Whether a record reports a measurement or a decision derived from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// An observed quantity for one arm.
    Measurement,
    /// A conclusion drawn across arms — a ceiling pass, a go/no-go gate.
    Verdict,
}

/// Whether a record carries publishable numbers, and if not, why not.
///
/// Schema 1 had no counterpart: a refused arm called `exit(3)` and emitted
/// nothing, which was right when a free-text `note` was the only marker
/// available — a note cannot stop a consumer averaging the record in. With a
/// typed status that a loader filters on, the argument inverts: emitting nothing
/// makes "we ran Flink and it blew the headroom limit" indistinguishable from
/// "we never ran Flink", and the first of those is a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ran to completion and passed every gate. Publishable.
    Ok,
    /// Exceeded the infrastructure headroom limit. The number describes
    /// ClickHouse or the broker, not the system under test.
    InfraBound,
    /// The system cannot express this variant at all — no fan-out operator, no
    /// Native writer. Carries no metrics, and exists so the site can render an
    /// explicit gap rather than an absence a reader would read as "not tried".
    Unsupported,
    /// Started but produced no valid measurement. Reason in `note`.
    Failed,
}

impl Status {
    /// Whether this status permits metrics to be attached.
    #[must_use]
    pub fn carries_metrics(self) -> bool {
        matches!(self, Self::Ok | Self::InfraBound)
    }
}

/// What caused a run to happen. Recorded so a PR-triggered measurement on
/// untrusted code can never be mistaken for a published one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    /// A scheduled full-matrix re-run.
    Nightly,
    /// Invoked by hand.
    Manual,
    /// Produced by a pull request. Never published.
    Pr,
    /// Pinned to a release of the system under test.
    Release,
}

/// A machine-readable caveat. `note` is prose for humans; these are filterable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The arm hit its cgroup CPU cap during the measurement window.
    CpuCapThrottled,
    /// No ceiling measurement was available to gate against.
    HeadroomUnproven,
    /// A sustained arm could not keep up with the offered rate.
    Saturated,
    /// Produced by a pull-request run on untrusted code. Never published.
    PrRun,
    /// Produced on hardware we do not control.
    ThirdPartyHardware,
    /// Infrastructure containers were reused rather than recreated.
    ReusedInfra,
}

/// One measured quantity, carrying its unit and its direction of goodness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metric {
    /// The measured value, in `unit`.
    pub value: f64,
    /// Unit of `value`. Constrained to a known set by `results_are_valid`.
    pub unit: String,
    /// `true` when a larger `value` is a better result.
    pub higher_is_better: bool,
    /// 95% confidence interval `(low, high)` when repetitions were taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci95: Option<(f64, f64)>,
    /// Sample count behind `value` (repetitions, not inner iterations).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
}

impl Metric {
    /// A metric where more is better — throughput, rows written.
    pub fn maximize(value: f64, unit: impl Into<String>) -> Self {
        Self {
            value,
            unit: unit.into(),
            higher_is_better: true,
            ci95: None,
            n: None,
        }
    }

    /// A metric where less is better — latency, CPU per row, bytes resident.
    pub fn minimize(value: f64, unit: impl Into<String>) -> Self {
        Self {
            value,
            unit: unit.into(),
            higher_is_better: false,
            ci95: None,
            n: None,
        }
    }

    /// A footprint in **bytes**, unscaled.
    ///
    /// This helper exists because its absence caused a real defect. The previous
    /// harness emitted `peak_anon_mb` holding megabytes while tagging the unit
    /// `"bytes"`; the site's formatter, correctly trusting the unit, divided by
    /// 1e6 again and rendered 1010 MB as "1.0 KB". The value and its label must
    /// be produced together or they drift.
    ///
    /// Scaling for display is the consumer's job — it is the only party that
    /// knows how much space it has.
    pub fn bytes(bytes: f64) -> Self {
        Self::minimize(bytes, "bytes")
    }

    /// A byte throughput, recorded as `MB/s` in the SI sense — 10^6 bytes, not
    /// 2^20. One divisor, so two rigs cannot drift onto different conventions
    /// while emitting the same unit string.
    pub fn bytes_per_s(bytes_per_s: f64) -> Self {
        Self::maximize(bytes_per_s / 1e6, "MB/s")
    }

    /// Attaches a 95% confidence interval.
    #[must_use]
    pub fn with_ci(mut self, low: f64, high: f64) -> Self {
        self.ci95 = Some((low, high));
        self
    }

    /// Attaches the repetition count behind the value.
    #[must_use]
    pub fn with_n(mut self, n: u64) -> Self {
        self.n = Some(n);
        self
    }
}

/// The system under test: what was **actually** run.
///
/// Every field here is resolved at run time from the descriptor plus runtime
/// interrogation of the image and process. None of it is typed by a human into a
/// results file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sut {
    /// Entrant directory name — joins to `entrants/<id>/`.
    pub entrant: String,
    /// Variant id from the descriptor. Stable for the life of the entrant.
    pub variant_id: String,
    /// Released version, resolved by the descriptor's `[version].strategy`.
    ///
    /// `None` only when the system has no release concept, in which case
    /// `commit` must be present — asserted by `results_are_valid`. Between them
    /// they discharge the requirement that every published number says what was
    /// measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Upstream commit, where there is no release or alongside a pre-release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// `sha256:…` of the image actually run, from `docker inspect`.
    ///
    /// Not an `Option`, deliberately. Version strings lie — a tag can be
    /// re-pushed under the same name — and an `Option` here would invite a code
    /// path that skips it on the day it is least convenient. If the digest
    /// cannot be read the run is [`Status::Failed`].
    pub image_digest: String,
    /// The image tag the driver was told to run. Human orientation only.
    pub image: String,
    /// Compiler or runtime that built the arm (`rustc 1.97.0`,
    /// `temurin-17.0.19+10`). Codegen moves throughput, so this is part of a
    /// number's provenance rather than trivia.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
}

/// The shared infrastructure, as **read back** from the running containers.
///
/// Every field is observed, never taken from the request that created it. The
/// previous harness asked for one envelope, warned when it got another, and
/// carried on — which is how three different infrastructure envelopes ended up
/// in one results file with nothing in the records to say which was in force.
/// Two components cannot disagree if only one of them is allowed to speak.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Infra {
    /// Stable hash over the envelope-defining subset only — cpus, memory,
    /// partitions, broker family. Deliberately excludes versions, so a
    /// ClickHouse patch release does not split a comparability group.
    pub digest: String,
    /// Broker family, e.g. `redpanda`.
    pub broker: String,
    /// Broker version, read from the broker.
    pub broker_version: String,
    /// `sha256:…` of the broker image.
    pub broker_image_digest: String,
    /// Broker CPU quota, from the container's cgroup `cpu.max`.
    pub broker_cpus: String,
    /// Broker memory limit, from the container's cgroup `memory.max`.
    pub broker_memory: String,
    /// ClickHouse version, from `SELECT version()`.
    pub clickhouse_version: String,
    /// `sha256:…` of the ClickHouse image.
    pub clickhouse_image_digest: String,
    /// ClickHouse CPU quota, from cgroup `cpu.max`.
    pub clickhouse_cpus: String,
    /// ClickHouse memory limit, from cgroup `memory.max`.
    pub clickhouse_memory: String,
    /// Topic partition count.
    pub partitions: i32,
    /// Schema Registry implementation, e.g. `redpanda-builtin`.
    pub registry: String,
    /// The measured consume ceiling this run was gated against. `0` means no
    /// ceiling was available, which sets [`Flag::HeadroomUnproven`].
    pub ceiling_msgs_per_s: u64,
}

/// Provenance for a run: when, where, under what protocol, on what data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMeta {
    /// Unix epoch milliseconds at which the record was built.
    pub ts_ms: u64,
    /// Interned hardware profile — the file stem in `environments/`.
    pub env_id: String,
    /// Content hash of the resolved environment profile, so a later edit to
    /// `environments/<env_id>.toml` cannot retroactively re-describe old runs.
    pub env_digest: String,
    /// Measurement protocol version. See [`HARNESS_VERSION`].
    pub harness_version: u32,
    /// Corpus version. See [`DATASET_VERSION`].
    pub dataset_version: String,
    /// Commit of *this* repository, for reproducing the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// What caused the run.
    pub trigger: Trigger,
    /// Shared infrastructure, read back from the running containers.
    pub infra: Infra,
}

/// The static half of [`RunMeta`], resolved once per process.
struct StaticMeta {
    commit: Option<String>,
}

fn static_meta() -> &'static StaticMeta {
    static META: OnceLock<StaticMeta> = OnceLock::new();
    META.get_or_init(|| StaticMeta {
        commit: detect_commit(),
    })
}

fn trimmed_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (!s.is_empty()).then_some(s)
}

fn detect_commit() -> Option<String> {
    if let Ok(c) = std::env::var("GIT_COMMIT")
        && !c.is_empty()
    {
        return Some(c);
    }
    trimmed_stdout("git", &["rev-parse", "--short=12", "HEAD"])
}

/// Milliseconds since the Unix epoch.
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl RunMeta {
    /// Builds run provenance around an environment and the infrastructure that
    /// was observed for it.
    pub fn new(
        env_id: impl Into<String>,
        env_digest: impl Into<String>,
        trigger: Trigger,
        infra: Infra,
    ) -> Self {
        Self {
            ts_ms: now_ms(),
            env_id: env_id.into(),
            env_digest: env_digest.into(),
            harness_version: HARNESS_VERSION,
            dataset_version: DATASET_VERSION.to_owned(),
            commit: static_meta().commit.clone(),
            trigger,
            infra,
        }
    }
}

/// A retraction. The record it is attached to stays in the archive and stays
/// visible on the site, struck through with `reason` shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Superseded {
    /// `run_id` of the replacement, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Why the original number should not be believed. Rendered to readers.
    pub reason: String,
    /// When the retraction was recorded.
    pub ts_ms: u64,
}

/// One emitted measurement record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// Schema version; always [`SCHEMA_VERSION`] on write.
    pub schema: u32,
    /// The suite: `kafka_avro_clickhouse` for the workload, `ceiling` for the
    /// infrastructure characterisation pass.
    pub bench: String,
    /// Measurement or verdict.
    pub kind: Kind,
    /// Whether this record carries publishable numbers.
    pub status: Status,
    /// UUIDv7 — time-ordered, so sorting by id sorts by time. One per
    /// (entrant, variant, rep) execution; never repeats.
    pub run_id: String,
    /// 1-based repetition index within one `bench run` invocation.
    pub rep: u32,
    /// Repetitions the invocation asked for, so a reader can see that rep 2 of 3
    /// is *missing* rather than having to infer it from a gap.
    pub reps: u32,
    /// What was actually run.
    pub sut: Sut,
    /// Provenance of the run.
    pub run: RunMeta,
    /// The arm's configuration. Never a measured quantity: two records sharing a
    /// variant identity are repetitions and may be aggregated.
    pub variant: BTreeMap<String, Value>,
    /// Measured quantities, keyed by metric name.
    pub metrics: BTreeMap<String, Metric>,
    /// Free-text caveat carried alongside the numbers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Machine-readable caveats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<Flag>,
    /// Set by a *later* commit when this record is retracted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Superseded>,
}

impl Report {
    /// A new record for one arm of one repetition.
    pub fn new(
        bench: impl Into<String>,
        kind: Kind,
        status: Status,
        sut: Sut,
        run: RunMeta,
    ) -> Self {
        Self {
            schema: SCHEMA_VERSION,
            bench: bench.into(),
            kind,
            status,
            run_id: uuid::Uuid::now_v7().to_string(),
            rep: 1,
            reps: 1,
            sut,
            run,
            variant: BTreeMap::new(),
            metrics: BTreeMap::new(),
            note: None,
            flags: Vec::new(),
            superseded_by: None,
        }
    }

    /// Records which repetition of how many this is.
    #[must_use]
    pub fn rep(mut self, rep: u32, reps: u32) -> Self {
        self.rep = rep;
        self.reps = reps;
        self
    }

    /// Adds one dimension of the arm's configuration.
    #[must_use]
    pub fn variant(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.variant.insert(key.into(), value.into());
        self
    }

    /// Adds one measured quantity.
    #[must_use]
    pub fn metric(mut self, key: impl Into<String>, metric: Metric) -> Self {
        self.metrics.insert(key.into(), metric);
        self
    }

    /// Attaches a caveat that travels with the numbers.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Attaches a machine-readable caveat, idempotently.
    #[must_use]
    pub fn flag(mut self, flag: Flag) -> Self {
        if !self.flags.contains(&flag) {
            self.flags.push(flag);
        }
        self
    }

    /// Serializes to the single JSON line that goes into a results file.
    ///
    /// # Errors
    ///
    /// Returns the underlying `serde_json` error if the record cannot be
    /// serialized.
    pub fn to_line(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sut() -> Sut {
        Sut {
            entrant: "spate".to_owned(),
            variant_id: "tier-a-native".to_owned(),
            version: Some("0.1.0-dev".to_owned()),
            commit: Some("f41280d51165".to_owned()),
            image_digest: format!("sha256:{}", "a".repeat(64)),
            image: "spate-bench-spate".to_owned(),
            toolchain: Some("rustc 1.97.0".to_owned()),
        }
    }

    fn infra() -> Infra {
        Infra {
            digest: "e3b0c44298fc".to_owned(),
            broker: "redpanda".to_owned(),
            broker_version: "v26.1.13".to_owned(),
            broker_image_digest: format!("sha256:{}", "b".repeat(64)),
            broker_cpus: "800000 100000".to_owned(),
            broker_memory: "8589934592".to_owned(),
            clickhouse_version: "26.3.1.1".to_owned(),
            clickhouse_image_digest: format!("sha256:{}", "c".repeat(64)),
            clickhouse_cpus: "500000 100000".to_owned(),
            clickhouse_memory: "12884901888".to_owned(),
            partitions: 8,
            registry: "redpanda-builtin".to_owned(),
            ceiling_msgs_per_s: 305_554,
        }
    }

    fn report() -> Report {
        Report::new(
            "kafka_avro_clickhouse",
            Kind::Measurement,
            Status::Ok,
            sut(),
            RunMeta::new("mac-m5max", "deadbeef", Trigger::Manual, infra()),
        )
    }

    #[test]
    fn round_trips_through_json_on_one_line() {
        let rep = report()
            .rep(2, 3)
            .variant("tier", "a")
            .variant("format", "native")
            .metric("rows_per_s", Metric::maximize(4_383_663.0, "records/s"))
            .metric("cpu_us_per_row", Metric::minimize(0.6187, "us").with_n(3))
            .flag(Flag::CpuCapThrottled)
            .note("330 sampler samples");

        let line = rep.to_line().expect("serialize");
        assert!(!line.contains('\n'), "a record must be one JSON line");

        let back: Report = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, rep);
        assert_eq!(back.schema, 2);
        assert_eq!(back.run.harness_version, HARNESS_VERSION);
    }

    #[test]
    fn optional_fields_are_omitted_when_absent() {
        let line = report().to_line().expect("serialize");
        assert!(!line.contains("note"), "{line}");
        assert!(!line.contains("superseded_by"), "{line}");
        assert!(!line.contains("flags"), "{line}");
        assert!(line.contains(r#""status":"ok""#), "{line}");
    }

    #[test]
    fn run_ids_are_unique_and_time_ordered() {
        // UUIDv7 sorts lexicographically by creation time, which is what lets a
        // results file be scanned in order without parsing timestamps.
        let a = report().run_id;
        let b = report().run_id;
        assert_ne!(a, b);
        assert!(a < b, "v7 ids must sort by time: {a} !< {b}");
    }

    #[test]
    fn footprint_bytes_are_unscaled_and_labelled_bytes() {
        // The regression this helper exists for: a value in megabytes tagged
        // "bytes" renders 1010 MB as "1.0 KB" in a consumer that trusts the unit.
        let m = Metric::bytes(1_059_481_600.0);
        assert_eq!(m.unit, "bytes");
        assert!(!m.higher_is_better);
        assert!(m.value > 1e9, "must be raw bytes, got {}", m.value);
    }

    #[test]
    fn byte_rates_are_si_megabytes() {
        let m = Metric::bytes_per_s(1_048_576.0);
        assert_eq!(m.unit, "MB/s");
        assert!((m.value - 1.048576).abs() < 1e-12);
    }

    #[test]
    fn direction_travels_with_the_number() {
        assert!(Metric::maximize(1.0, "records/s").higher_is_better);
        assert!(!Metric::minimize(1.0, "ns").higher_is_better);
    }

    #[test]
    fn only_successful_statuses_carry_metrics() {
        assert!(Status::Ok.carries_metrics());
        assert!(Status::InfraBound.carries_metrics());
        assert!(!Status::Failed.carries_metrics());
        assert!(!Status::Unsupported.carries_metrics());
    }
}
