//! The deterministic corpus for the cross-framework comparison.
//!
//! Every framework in the comparison receives byte-identical input, and the
//! correctness gates have to be able to say more than "the same number of rows
//! arrived". Both properties come from the same place: the corpus is a **pure
//! function of `batch_id`**.
//!
//! That buys three things a random or timestamp-seeded generator could not:
//!
//! * The expected row count, the expected checksum, and the expected per-tier
//!   filtered counts are all computable without reading what any framework
//!   produced — so a framework that silently transforms wrongly fails the gate
//!   just as loudly as one that drops rows.
//! * `(batch_id, seq)` is a true row identity, which makes
//!   `uniqExact((batch_id, event_seq))` an exact loss count and
//!   `count() - uniqExact(...)` an exact duplicate count.
//! * A prefilled topic can be regenerated identically months later, on a
//!   different machine, to re-run a published arm.
//!
//! The field derivations here are the normative ones from
//! `benchmarks/comparisons/README.md`. If the two ever disagree, this file is
//! wrong — the README is what the competitor implementations were written
//! against.
//!
//! One deliberate property worth stating: `value` is always non-negative
//! (a `%` of a positive modulus over unsigned arithmetic). That removes a real
//! cross-language hazard from tier B — integer division truncates toward zero in
//! Rust, Java and ClickHouse alike for non-negative operands, so
//! `value * 1000 / (event_seq + 1)` cannot disagree between implementations the
//! way it could if the sign varied.

use apache_avro::types::Value as AvroValue;
use apache_avro::{Schema, to_avro_datum};
use serde::Deserialize;
use std::sync::OnceLock;

/// The one Avro schema, read from the file the competitor implementations also
/// read. Embedded so a rig cannot drift from the registered subject.
pub const SCHEMA_JSON: &str = include_str!("../../workload/schema/sensor_batch.avsc");

// The generator's tunables are NOT written here. They live in
// `workload/workload.toml` and are emitted as constants by `harness/build.rs`,
// which also hashes that file into `DATASET_VERSION`.
//
// The indirection buys one specific guarantee: a change to what the data *is*
// cannot be made without the corpus version moving, so two result sets produced
// from different corpora can never be silently placed on the same axis. Writing
// the constants in both places would let them drift, and a drifted corpus
// constant is invisible until two published numbers disagree for no stated
// reason.
//
// The reasoning behind each value lives beside it in workload.toml.
include!(concat!(env!("OUT_DIR"), "/workload_consts.rs"));

/// The parsed schema, compiled once per process.
///
/// # Panics
/// If the committed `.avsc` does not parse — which would mean the file every
/// framework reads is invalid, and no arm could be trusted.
pub fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA
        .get_or_init(|| Schema::parse_str(SCHEMA_JSON).expect("committed sensor_batch.avsc parses"))
}

// ---------------------------------------------------------------------------
// Field derivations — the single source of truth for producer and gates alike.
// ---------------------------------------------------------------------------

/// `sensor` for a batch.
#[must_use]
pub fn sensor_of(batch_id: u64) -> String {
    format!("sensor-{}", batch_id % SENSORS)
}

/// `region` for a batch: null one batch in ten, which is what forces every
/// implementation through a real union-decode path.
#[must_use]
pub fn region_of(batch_id: u64) -> Option<String> {
    if batch_id.is_multiple_of(10) {
        None
    } else {
        Some(format!("region-{}", batch_id % 7))
    }
}

/// Event timestamp for a batch, epoch milliseconds.
#[must_use]
pub fn batch_ts_ms_of(batch_id: u64) -> i64 {
    BASE_TS_MS + i64::try_from(batch_id).expect("batch_id fits i64")
}

/// `name` for an event.
#[must_use]
pub fn name_of(batch_id: u64, seq: u32) -> String {
    format!("metric_{}", (batch_id * 31 + u64::from(seq)) % NAMES)
}

/// `unit` for an event.
#[must_use]
pub fn unit_of(batch_id: u64, seq: u32) -> &'static str {
    UNITS[usize::try_from((batch_id * 7 + u64::from(seq)) % 8).expect("index fits usize")]
}

/// `value` for an event. Always non-negative — see the module docs.
#[must_use]
pub fn value_of(batch_id: u64, seq: u32) -> i64 {
    let v = (batch_id.wrapping_mul(1_000_003) + u64::from(seq) * 97) % 2_147_483_647;
    i64::try_from(v).expect("value below 2^31")
}

/// `quality` for an event: null one event in five.
#[must_use]
pub fn quality_of(batch_id: u64, seq: u32) -> Option<f64> {
    let s = u64::from(seq);
    if (batch_id + s).is_multiple_of(5) {
        None
    } else {
        #[expect(
            clippy::cast_precision_loss,
            reason = "the numerator is a residue mod 100, exactly representable"
        )]
        Some(((batch_id * 13 + s * 7) % 100) as f64 / 100.0)
    }
}

/// `tags` for an event: 0..=3 elements, the second nesting level.
#[must_use]
pub fn tags_of(batch_id: u64, seq: u32) -> Vec<String> {
    let s = u64::from(seq);
    (0..((batch_id + s) % 4))
        .map(|j| format!("tag-{}", (batch_id + s + j) % TAGS))
        .collect()
}

/// ASCII-only uppercase, the tier-B `name_upper` derivation.
///
/// ASCII-only is specified rather than incidental: Java's
/// `String.toUpperCase()` is locale-dependent, so an unqualified "uppercase"
/// would not be the same operation in every implementation.
#[must_use]
pub fn ascii_upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

/// The tier-B `value_scaled` derivation.
#[must_use]
pub fn value_scaled_of(value: i64, seq: u32) -> i64 {
    value * 1000 / i64::from(seq + 1)
}

/// Whether tier B keeps this event.
#[must_use]
pub fn tier_b_keeps(batch_id: u64, seq: u32) -> bool {
    if unit_of(batch_id, seq) == DROP_UNIT {
        return false;
    }
    !matches!(quality_of(batch_id, seq), Some(q) if q < QUALITY_FLOOR)
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

/// Encode one batch as a bare Avro datum.
///
/// `send_ts_us` is supplied by the caller because it is the producer's
/// **intended** schedule time, not a property of `batch_id` — it is the one
/// field that legitimately varies between a prefill and a live run.
///
/// # Panics
/// If the datum cannot be encoded against the committed schema.
#[must_use]
pub fn encode_batch(batch_id: u64, send_ts_us: i64) -> Vec<u8> {
    let sensor = sensor_of(batch_id);
    let region = match region_of(batch_id) {
        // Branch indices follow the schema's declared union order,
        // `["null","string"]`.
        None => AvroValue::Union(0, Box::new(AvroValue::Null)),
        Some(r) => AvroValue::Union(1, Box::new(AvroValue::String(r))),
    };
    let events = (0..EVENTS_PER_BATCH)
        .map(|seq| {
            let quality = match quality_of(batch_id, seq) {
                None => AvroValue::Union(0, Box::new(AvroValue::Null)),
                Some(q) => AvroValue::Union(1, Box::new(AvroValue::Double(q))),
            };
            let tags = AvroValue::Array(
                tags_of(batch_id, seq)
                    .into_iter()
                    .map(AvroValue::String)
                    .collect(),
            );
            AvroValue::Record(vec![
                (
                    "seq".to_owned(),
                    AvroValue::Int(i32::try_from(seq).expect("seq fits i32")),
                ),
                ("name".to_owned(), AvroValue::String(name_of(batch_id, seq))),
                (
                    "unit".to_owned(),
                    AvroValue::String(unit_of(batch_id, seq).to_owned()),
                ),
                ("value".to_owned(), AvroValue::Long(value_of(batch_id, seq))),
                ("quality".to_owned(), quality),
                ("tags".to_owned(), tags),
            ])
        })
        .collect();

    let record = AvroValue::Record(vec![
        (
            "batch_id".to_owned(),
            AvroValue::Long(i64::try_from(batch_id).expect("batch_id fits i64")),
        ),
        ("sensor".to_owned(), AvroValue::String(sensor)),
        ("region".to_owned(), region),
        (
            "batch_ts_ms".to_owned(),
            AvroValue::Long(batch_ts_ms_of(batch_id)),
        ),
        ("send_ts_us".to_owned(), AvroValue::Long(send_ts_us)),
        ("events".to_owned(), AvroValue::Array(events)),
    ]);

    to_avro_datum(schema(), record).expect("encode sensor batch datum")
}

/// Wrap a datum in Confluent wire format: `0x00`, big-endian u32 schema id,
/// then the datum.
///
/// Confluent framing is used for every arm because three of the five
/// competitors effectively require a registry, and the lookup is cached so it
/// costs nothing at steady state.
#[must_use]
pub fn frame_confluent(schema_id: u32, datum: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(5 + datum.len());
    framed.push(0);
    framed.extend_from_slice(&schema_id.to_be_bytes());
    framed.extend_from_slice(datum);
    framed
}

// ---------------------------------------------------------------------------
// Decode targets
// ---------------------------------------------------------------------------

/// A decoded message. Field names match the Avro schema.
#[derive(Clone, Debug, Deserialize)]
pub struct SensorBatch {
    /// Dense, monotonic batch identifier; half of the row identity.
    pub batch_id: i64,
    /// Sensor identifier.
    pub sensor: String,
    /// Nullable region — the union the decode path must handle.
    pub region: Option<String>,
    /// Event timestamp, epoch milliseconds.
    pub batch_ts_ms: i64,
    /// Producer's intended send time, epoch microseconds.
    pub send_ts_us: i64,
    /// The events to fan out.
    pub events: Vec<Event>,
}

/// One event inside a [`SensorBatch`].
#[derive(Clone, Debug, Deserialize)]
pub struct Event {
    /// Position within the batch; the other half of the row identity.
    pub seq: i32,
    /// Metric name.
    pub name: String,
    /// Metric unit; `"drop"` is the tier-B filter sentinel.
    pub unit: String,
    /// Metric value.
    pub value: i64,
    /// Nullable quality — the second union.
    pub quality: Option<f64>,
    /// Inner array-of-string.
    pub tags: Vec<String>,
}

/// Tier A columns, positional — used to build the Native schema and the sink
/// `columns` list from one definition.
pub const COLUMNS_A: &[(&str, &str)] = &[
    ("batch_id", "UInt64"),
    ("event_seq", "UInt16"),
    ("sensor", "LowCardinality(String)"),
    ("region", "LowCardinality(String)"),
    ("name", "LowCardinality(String)"),
    ("unit", "LowCardinality(String)"),
    ("value", "Int64"),
    ("quality", "Nullable(Float64)"),
    ("tags", "Array(LowCardinality(String))"),
    ("batch_ts", "DateTime64(3)"),
    ("send_ts", "DateTime64(6)"),
];

/// Tier B columns, positional.
pub const COLUMNS_B: &[(&str, &str)] = &[
    ("batch_id", "UInt64"),
    ("event_seq", "UInt16"),
    ("sensor", "LowCardinality(String)"),
    ("region", "LowCardinality(String)"),
    ("name_upper", "LowCardinality(String)"),
    ("unit", "LowCardinality(String)"),
    ("value", "Int64"),
    ("value_scaled", "Int64"),
    ("quality", "Nullable(Float64)"),
    ("tags", "Array(LowCardinality(String))"),
    ("batch_ts", "DateTime64(3)"),
    ("send_ts", "DateTime64(6)"),
];

// ---------------------------------------------------------------------------
// Expectations for the correctness gates
// ---------------------------------------------------------------------------

/// Which workload tier an arm ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Decode, flatten, insert. Column mapping only.
    A,
    /// Tier A plus the specified filtering and derivation.
    B,
}

impl Tier {
    /// The `tier` variant value recorded on every measurement.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Tier::A => "a",
            Tier::B => "b",
        }
    }

    /// The target table for this tier.
    #[must_use]
    pub fn table(self) -> &'static str {
        match self {
            Tier::A => "sensor_events",
            Tier::B => "sensor_events_t",
        }
    }

    /// The positional column list for this tier.
    #[must_use]
    pub fn columns(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Tier::A => COLUMNS_A,
            Tier::B => COLUMNS_B,
        }
    }
}

/// What a correct arm must have produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected {
    /// Distinct `(batch_id, event_seq)` rows. Anything less is data loss.
    pub rows: u64,
    /// Sum of `value` over distinct rows. Computed as `i128` because a large
    /// corpus would overflow the `Int64` that ClickHouse's `sum` would
    /// otherwise return — the gate query casts to `Int128` to match.
    pub value_sum: i128,
    /// Sum of `value_scaled` over distinct rows; zero for tier A, which has no
    /// such column.
    pub value_scaled_sum: i128,
}

/// Compute what `batches` messages must yield for `tier`.
///
/// Deliberately a loop over the generator rather than a closed form: the point
/// of the gate is to catch a transform that disagrees with the specification,
/// and a closed form derived from the same misreading of the spec would agree
/// with the bug. Iterating the actual derivations costs ~100M cheap iterations
/// on the largest planned corpus, which is a fraction of a second in release.
#[must_use]
pub fn expected(batches: u64, tier: Tier) -> Expected {
    expected_range(0, batches, tier)
}

/// Compute what batches `lo..hi` must yield for `tier`.
///
/// Sustained mode needs this rather than [`expected`]: the producer runs
/// continuously and the consumer is stopped mid-stream, so the range that
/// actually landed is some `[min(batch_id), max(batch_id)]` window rather than a
/// prefix starting at zero. Gating against a prefix would either fail a correct
/// arm or, worse, pass a broken one whose totals happened to coincide.
#[must_use]
pub fn expected_range(lo: u64, hi: u64, tier: Tier) -> Expected {
    let mut out = Expected {
        rows: 0,
        value_sum: 0,
        value_scaled_sum: 0,
    };
    for batch_id in lo..hi {
        for seq in 0..EVENTS_PER_BATCH {
            if tier == Tier::B && !tier_b_keeps(batch_id, seq) {
                continue;
            }
            let value = value_of(batch_id, seq);
            out.rows += 1;
            out.value_sum += i128::from(value);
            if tier == Tier::B {
                out.value_scaled_sum += i128::from(value_scaled_of(value, seq));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Target schema
// ---------------------------------------------------------------------------

/// The committed target DDL, applied verbatim so the driver and the competitor
/// implementations cannot disagree about the target tables.
pub const DDL: &str = include_str!("../../workload/clickhouse/ddl.sql");

/// Split [`DDL`] into executable statements.
///
/// Line comments are stripped **before** splitting on `;`, which is load-bearing
/// rather than tidy: the file documents the correctness-gate queries in trailing
/// `--` comments and those contain semicolons. Splitting first would try to
/// execute fragments of prose.
///
/// This lives in the library rather than in the driver binary because the rigs
/// are declared `test = false` — a `#[cfg(test)]` module inside a bin is never
/// compiled or run, so tests placed there would be silently dead.
#[must_use]
pub fn ddl_statements() -> Vec<String> {
    split_sql(DDL)
}

fn split_sql(sql: &str) -> Vec<String> {
    let stripped: String = sql
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    stripped
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// Registry and prefill
// ---------------------------------------------------------------------------

/// The registry subject. Topic-name strategy, which is what Kafka Connect's
/// `AvroConverter` and ClickHouse's `AvroConfluent` both expect by default.
pub const SUBJECT: &str = "sensor-batches-value";

/// `send_ts_us` for a **prefilled** batch: derived from `batch_id`, not the
/// clock.
///
/// This is deliberate. A prefilled corpus is replayed by every arm from offset
/// 0, so deriving the timestamp keeps the corpus byte-identical across
/// re-prefills and makes a published arm reproducible months later. The cost is
/// that drain-mode latency is meaningless by construction — the difference
/// between `ingest_ts` and this value is backlog age, not pipeline latency —
/// which is why drain mode reports throughput only. Sustained mode uses real
/// intended-schedule timestamps instead.
#[must_use]
pub fn send_ts_us_prefill(batch_id: u64) -> i64 {
    BASE_TS_MS * 1000 + i64::try_from(batch_id).expect("batch_id fits i64")
}

/// Register the committed schema under [`SUBJECT`] and return its id.
///
/// Idempotent: re-registering identical schema text returns the existing id.
///
/// # Panics
/// If the registry rejects the schema or returns no id — every arm decodes
/// through this id, so a failure here invalidates the whole run rather than one
/// arm.
#[must_use]
pub fn register_schema(host: &str, port: u16) -> u32 {
    let body = serde_json::json!({ "schema": SCHEMA_JSON, "schemaType": "AVRO" }).to_string();
    let path = format!("/subjects/{SUBJECT}/versions");
    let resp = crate::http::post_typed(
        host,
        port,
        &path,
        Some("application/vnd.schemaregistry.v1+json"),
        &body,
    )
    .expect("schema registry POST");
    let parsed: serde_json::Value = serde_json::from_str(&resp)
        .unwrap_or_else(|e| panic!("schema registry returned non-JSON {resp:?}: {e}"));
    let id = parsed
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("schema registry response carries no id: {resp}"));
    u32::try_from(id).expect("schema id fits u32")
}

/// A live producer running open-loop at a fixed offered rate.
///
/// This is the sustained-mode load source, and it is the whole
/// coordinated-omission defence. Two properties do that work:
///
/// * **Sends are scheduled against a fixed origin**, `origin + n / rate`, never
///   `sleep(1/rate)` in a loop. A per-iteration sleep accumulates its own
///   overhead as drift, so the offered rate would silently sag below the
///   requested one and the arm would look better than it is.
/// * **`send_ts_us` carries the *intended* time, not the actual send time.** If
///   the producer falls behind — because the broker pushed back, or the host was
///   busy — that delay lands in the measured latency instead of vanishing from
///   it. Stamping `now()` at send is precisely the mistake that makes a
///   saturated system report excellent percentiles.
///
/// The caller is expected to reject the arm when [`LoadReport::achieved_share`]
/// falls materially below 1.0: at that point the producer, not the framework,
/// was the constraint, and the measurement is of the harness.
/// The generator is **multi-threaded**, and that is a requirement rather than an
/// optimisation. Measured on this host, one producer thread tops out near 73k
/// messages/s (~1.47M rows/s), at which point the framework under test was still
/// using only 1.05 of its 4 cores. A single-threaded generator would therefore
/// make every arm producer-bound, and the comparison would be between frameworks
/// that are all idling — the most expensive way to measure nothing.
///
/// Threads interleave by stride: thread `i` of `n` sends global indices
/// `i, i+n, i+2n, ...`. The global schedule is preserved exactly, because each
/// message's due time is derived from its **global** index, not from a per-thread
/// counter. `batch_id` therefore remains dense across the whole generator, which
/// the correctness gate's contiguity test depends on.
#[derive(Debug)]
pub struct SustainedLoad {
    handles: Vec<std::thread::JoinHandle<LoadReport>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// Delivery-counting context for the sustained producer.
///
/// Without this the producer counts messages it *enqueued*, not messages the
/// broker *acknowledged* — and `BaseProducer` discards a failed delivery
/// silently. That is not a cosmetic difference: a handful of dropped sends puts
/// gaps in the middle of the `batch_id` sequence, and the correctness gate then
/// reports them as the framework losing rows. This was found exactly that way.
struct DeliveryCounter {
    delivered: std::sync::Arc<std::sync::atomic::AtomicU64>,
    failed: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl rdkafka::ClientContext for DeliveryCounter {}

impl rdkafka::producer::ProducerContext for DeliveryCounter {
    type DeliveryOpaque = ();
    fn delivery(&self, result: &rdkafka::producer::DeliveryResult<'_>, _: ()) {
        use std::sync::atomic::Ordering;
        match result {
            Ok(_) => {
                self.delivered.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// What a sustained producer actually managed to offer.
#[derive(Clone, Copy, Debug)]
pub struct LoadReport {
    /// Messages enqueued.
    pub sent: u64,
    /// Messages the broker acknowledged. Anything less than `sent` means the
    /// **harness** lost messages, not the framework.
    pub delivered: u64,
    /// Messages whose delivery failed.
    pub failed: u64,
    /// Wall seconds the producer ran.
    pub elapsed_s: f64,
    /// Requested offered rate, messages/s.
    pub target_rate: u64,
    /// Achieved rate as a fraction of the target. Below ~0.99 means the load
    /// generator was the bottleneck and the arm is not measuring the framework.
    pub achieved_share: f64,
    /// Largest amount by which any send ran behind its intended schedule.
    /// A large value with `achieved_share` near 1.0 means the producer caught
    /// up in bursts rather than tracking the schedule smoothly.
    pub max_schedule_lag_ms: f64,
}

impl SustainedLoad {
    /// Start producing `rate` messages/s with a **strictly monotonic**
    /// `batch_id` starting at `first_batch_id`.
    ///
    /// Monotonic, never cycling, and that is load-bearing for the correctness
    /// gates rather than incidental. `(batch_id, event_seq)` is the row identity:
    /// if the producer wrapped around a fixed corpus, repeated identities would
    /// be *expected*, and the gates could no longer distinguish a legitimate
    /// replay from a framework emitting duplicates — which is one of the few
    /// ways an arm can look fast for a dishonest reason.
    ///
    /// `first_batch_id` lets a later run continue past an earlier one on the same
    /// topic without colliding with rows already in the target table.
    ///
    /// # Panics
    /// If the producer cannot be created.
    #[must_use]
    pub fn start(
        bootstrap: &str,
        topic: &str,
        partitions: i32,
        schema_id: u32,
        rate: u64,
        first_batch_id: u64,
        threads: u64,
    ) -> Self {
        use rdkafka::config::ClientConfig;
        use rdkafka::producer::{BaseProducer, BaseRecord};
        use std::sync::atomic::AtomicBool;
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        assert!(rate > 0, "sustained load needs a positive rate");
        assert!(threads > 0, "sustained load needs at least one thread");

        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let handles = (0..threads)
            .map(|slot| {
                let stop_thread = std::sync::Arc::clone(&stop);
                let (bootstrap, topic) = (bootstrap.to_owned(), topic.to_owned());

                std::thread::spawn(move || {
                    use std::sync::atomic::AtomicU64;
                    let delivered = std::sync::Arc::new(AtomicU64::new(0));
                    let failed = std::sync::Arc::new(AtomicU64::new(0));
                    let producer: BaseProducer<DeliveryCounter> = ClientConfig::new()
                        .set("bootstrap.servers", &bootstrap)
                        .set("linger.ms", "5")
                        .set("batch.size", "1048576")
                        // Retry a transient send rather than dropping it: a dropped
                        // message becomes a hole in the `batch_id` sequence that the
                        // correctness gate cannot distinguish from the framework losing
                        // a row. Idempotence keeps the retries from duplicating.
                        .set("enable.idempotence", "true")
                        .set("message.send.max.retries", "10")
                        .create_with_context(DeliveryCounter {
                            delivered: std::sync::Arc::clone(&delivered),
                            failed: std::sync::Arc::clone(&failed),
                        })
                        .expect("sustained producer");

                    let origin = Instant::now();
                    let origin_epoch_us = i64::try_from(
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .expect("clock after epoch")
                            .as_micros(),
                    )
                    .expect("epoch micros fit i64");

                    let mut sent = 0u64;
                    let mut max_lag_us = 0i64;
                    while !stop_thread.load(Ordering::Relaxed) {
                        // This thread's `sent`-th message is global index
                        // `slot + sent * threads`. Deriving the due time from the GLOBAL
                        // index is what keeps the aggregate schedule exact: a per-thread
                        // counter would give each thread its own timeline and the offered
                        // rate would drift.
                        let global = slot + sent * threads;
                        let due_us = (global * 1_000_000) / rate;
                        let elapsed_us =
                            u64::try_from(origin.elapsed().as_micros()).unwrap_or(u64::MAX);
                        if elapsed_us < due_us {
                            // Ahead of schedule: serve the client and wait, but never
                            // longer than the remaining gap.
                            let wait = Duration::from_micros((due_us - elapsed_us).min(2_000));
                            producer.poll(wait);
                            continue;
                        }
                        max_lag_us =
                            max_lag_us.max(i64::try_from(elapsed_us - due_us).unwrap_or(i64::MAX));

                        let batch_id = first_batch_id + global;
                        // The intended schedule time, NOT `now()`. See the type docs.
                        let send_ts_us =
                            origin_epoch_us + i64::try_from(due_us).expect("due fits i64");
                        let payload =
                            frame_confluent(schema_id, &encode_batch(batch_id, send_ts_us));
                        let key = sensor_of(batch_id);
                        let partition = i32::try_from(
                            global % u64::try_from(partitions).expect("partitions > 0"),
                        )
                        .expect("partition fits i32");
                        match producer.send(
                            BaseRecord::to(&topic)
                                .partition(partition)
                                .key(&key)
                                .payload(&payload),
                        ) {
                            Ok(()) => sent += 1,
                            Err((e, _))
                                if e.rdkafka_error_code()
                                    == Some(rdkafka::types::RDKafkaErrorCode::QueueFull) =>
                            {
                                producer.poll(Duration::from_millis(1));
                            }
                            Err((e, _)) => panic!("sustained produce: {e}"),
                        }
                        if sent.is_multiple_of(4096) {
                            producer.poll(Duration::ZERO);
                        }
                    }
                    let elapsed_s = origin.elapsed().as_secs_f64();
                    use rdkafka::producer::Producer;
                    producer
                        .flush(Duration::from_secs(60))
                        .expect("flush sustained producer");
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "message counts stay far below f64's exact integer range"
                    )]
                    // This thread's share of the global target.
                    let achieved = sent as f64 / ((rate as f64 / threads as f64) * elapsed_s);
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "microsecond lag stays far below f64's exact integer range"
                    )]
                    let max_schedule_lag_ms = max_lag_us as f64 / 1000.0;
                    use std::sync::atomic::Ordering;
                    LoadReport {
                        sent,
                        delivered: delivered.load(Ordering::Relaxed),
                        failed: failed.load(Ordering::Relaxed),
                        elapsed_s,
                        target_rate: rate,
                        achieved_share: achieved,
                        max_schedule_lag_ms,
                    }
                })
            })
            .collect();

        Self { handles, stop }
    }

    /// Stop producing and collect what was actually offered, summed across
    /// threads.
    ///
    /// `achieved_share` is recomputed from the totals rather than averaged: one
    /// thread keeping up cannot compensate for another falling behind, and the
    /// question the gate asks is whether the *aggregate* offered rate hit its
    /// target. `max_schedule_lag_ms` is the worst lag any thread saw.
    ///
    /// # Panics
    /// If a producer thread panicked, or if there were no threads.
    #[must_use]
    pub fn stop(mut self) -> LoadReport {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let parts: Vec<LoadReport> = std::mem::take(&mut self.handles)
            .into_iter()
            .map(|h| h.join().expect("sustained producer thread"))
            .collect();
        assert!(!parts.is_empty(), "no producer threads");

        let sent: u64 = parts.iter().map(|p| p.sent).sum();
        let delivered: u64 = parts.iter().map(|p| p.delivered).sum();
        let failed: u64 = parts.iter().map(|p| p.failed).sum();
        let elapsed_s = parts.iter().map(|p| p.elapsed_s).fold(0.0_f64, f64::max);
        let target_rate = parts[0].target_rate;
        #[expect(
            clippy::cast_precision_loss,
            reason = "message counts stay far below f64's exact integer range"
        )]
        let achieved_share = sent as f64 / (target_rate as f64 * elapsed_s);
        LoadReport {
            sent,
            delivered,
            failed,
            elapsed_s,
            target_rate,
            achieved_share,
            max_schedule_lag_ms: parts
                .iter()
                .map(|p| p.max_schedule_lag_ms)
                .fold(0.0_f64, f64::max),
        }
    }
}

/// What one prefill produced.
#[derive(Clone, Copy, Debug)]
pub struct PrefillReport {
    /// Batches on the topic after this call.
    pub batches: u64,
    /// Total framed payload bytes produced (0 when the corpus was reused).
    pub bytes: u64,
    /// Wall seconds spent producing (0 when reused).
    pub elapsed_s: f64,
    /// Whether an existing corpus was reused rather than reproduced.
    pub reused: bool,
}

/// Count messages currently on `topic`, for callers outside this module.
///
/// Drain mode needs this to know how much work exists: a window that outlasts the
/// corpus measures an idle pipeline, not its throughput.
///
/// # Panics
/// If a consumer cannot be created.
#[must_use]
pub fn topic_message_count(bootstrap: &str, topic: &str, partitions: i32) -> u64 {
    use rdkafka::config::ClientConfig;
    use rdkafka::consumer::{Consumer, base_consumer::BaseConsumer};
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap)
        .set("group.id", "comparison-depth-probe")
        .create()
        .expect("depth probe consumer");
    (0..partitions)
        .map(|p| {
            consumer
                .fetch_watermarks(topic, p, std::time::Duration::from_secs(10))
                .map_or(0, |(low, high)| u64::try_from(high - low).unwrap_or(0))
        })
        .sum()
}

/// Count messages currently on `topic` by summing per-partition watermarks.
fn topic_depth(producer: &rdkafka::producer::BaseProducer, topic: &str, partitions: i32) -> u64 {
    use rdkafka::producer::Producer;
    (0..partitions)
        .map(|p| {
            producer
                .client()
                .fetch_watermarks(topic, p, std::time::Duration::from_secs(10))
                .map_or(0, |(low, high)| u64::try_from(high - low).unwrap_or(0))
        })
        .sum()
}

/// Fill `topic` with `batches` Confluent-framed messages, or reuse what is
/// already there.
///
/// Reuse is keyed on the topic already holding exactly `batches` messages.
/// Because the corpus is a pure function of `batch_id` and `send_ts_us` is
/// derived, a topic of the right depth necessarily holds the right bytes — so
/// reuse is safe, and it saves re-producing gigabytes on every one of the ~50
/// arms in a sweep.
///
/// Partitions are assigned round-robin explicitly rather than by key hash: an
/// uneven partition distribution would penalise whichever arm is
/// partition-parallelism-bound, for reasons that have nothing to do with the
/// framework.
///
/// # Panics
/// If the producer cannot be created, a send fails for any reason other than a
/// full queue, or the final flush times out.
#[must_use]
pub fn prefill(
    bootstrap: &str,
    topic: &str,
    partitions: i32,
    batches: u64,
    schema_id: u32,
) -> PrefillReport {
    use rdkafka::config::ClientConfig;
    use rdkafka::producer::{BaseProducer, BaseRecord, Producer};
    use std::time::{Duration, Instant};

    crate::kafka::ensure_topic(bootstrap, topic, partitions);
    let producer: BaseProducer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap)
        .set("linger.ms", "20")
        .set("batch.size", "1048576")
        .set("compression.type", "none")
        .create()
        .expect("prefill producer");

    let depth = topic_depth(&producer, topic, partitions);
    if depth == batches {
        eprintln!("prefill: reusing {batches} existing messages on {topic}");
        return PrefillReport {
            batches,
            bytes: 0,
            elapsed_s: 0.0,
            reused: true,
        };
    }
    assert_eq!(
        depth, 0,
        "topic {topic} holds {depth} messages but the run wants {batches}. \
         A partially-filled corpus would make every arm replay different bytes; \
         delete the topic and re-prefill."
    );

    let start = Instant::now();
    let mut bytes = 0u64;
    for batch_id in 0..batches {
        let datum = encode_batch(batch_id, send_ts_us_prefill(batch_id));
        let payload = frame_confluent(schema_id, &datum);
        bytes += payload.len() as u64;
        let key = sensor_of(batch_id);
        let partition =
            i32::try_from(batch_id % u64::try_from(partitions).expect("partitions > 0"))
                .expect("partition fits i32");
        loop {
            match producer.send(
                BaseRecord::to(topic)
                    .partition(partition)
                    .key(&key)
                    .payload(&payload),
            ) {
                Ok(()) => break,
                Err((e, _))
                    if e.rdkafka_error_code()
                        == Some(rdkafka::types::RDKafkaErrorCode::QueueFull) =>
                {
                    producer.poll(Duration::from_millis(5));
                }
                Err((e, _)) => panic!("prefill produce: {e}"),
            }
        }
        if batch_id.is_multiple_of(4096) {
            producer.poll(Duration::ZERO);
        }
    }
    producer.flush(Duration::from_secs(300)).expect("flush");

    let landed = topic_depth(&producer, topic, partitions);
    assert_eq!(
        landed, batches,
        "prefill produced {batches} messages but the topic holds {landed}; \
         every arm would replay a different corpus"
    );
    PrefillReport {
        batches,
        bytes,
        elapsed_s: start.elapsed().as_secs_f64(),
        reused: false,
    }
}

// ---------------------------------------------------------------------------
// Correctness gates
// ---------------------------------------------------------------------------

/// Outcome of the correctness gates for one arm.
///
/// Every field is reported rather than collapsed into a boolean, because the
/// *reason* an arm failed is what tells us whether the framework dropped rows,
/// duplicated them, or transformed them wrongly.
#[derive(Clone, Copy, Debug)]
pub struct Gates {
    /// Lowest `batch_id` present in the table.
    pub min_batch: u64,
    /// Highest `batch_id` present in the table.
    pub max_batch: u64,
    /// Rows in the interior range.
    pub rows: u64,
    /// Distinct `(batch_id, event_seq)` pairs in the interior range.
    pub distinct_ids: u64,
    /// Distinct `batch_id`s in the interior range.
    pub distinct_batches: u64,
    /// `rows - distinct_ids`. Reported, never suppressed: these are all
    /// at-least-once systems and some duplication is legitimate.
    pub duplicates: u64,
    /// Whether the interior `batch_id`s form an unbroken run — the loss test.
    pub contiguous: bool,
    /// Whether the row count matches the generator's expectation.
    pub rows_match: bool,
    /// Whether `sum(value)` matches the generator's expectation.
    pub value_sum_match: bool,
    /// Whether `sum(value_scaled)` matches (tier B only; trivially true for A).
    pub value_scaled_match: bool,
}

impl Gates {
    /// Whether this arm may be published.
    ///
    /// Duplicates deliberately do **not** fail the gate: at-least-once permits
    /// them, and they are published as a metric so a reader can judge. Loss and
    /// wrong arithmetic do fail it.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.contiguous && self.rows_match && self.value_sum_match && self.value_scaled_match
    }

    /// Human-readable reason for a failure, for the driver's refusal message.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        if self.passed() {
            return None;
        }
        let mut why = Vec::new();
        if !self.contiguous {
            why.push(format!(
                "batch_ids are not contiguous ({} distinct across [{}, {}]) — rows were LOST",
                self.distinct_batches, self.min_batch, self.max_batch
            ));
        }
        if !self.rows_match {
            why.push(format!(
                "row count {} disagrees with the generator",
                self.rows
            ));
        }
        if !self.value_sum_match {
            why.push("sum(value) disagrees with the generator — the arm did different work".into());
        }
        if !self.value_scaled_match {
            why.push("sum(value_scaled) disagrees — the tier-B derivation is wrong".into());
        }
        Some(why.join("; "))
    }
}

/// Run the correctness gates for `tier` against the target table.
///
/// **The first and last `batch_id` are excluded.** A sealed sink chunk can split
/// one message's rows across two batches, so at the instant the driver snapshots
/// the table, the boundary batches may be only partially landed. Gating over the
/// interior range removes that fence-post without weakening the test: any loss or
/// mis-transformation strictly inside the range still fails.
///
/// **The exact tests cover at most `max_batches` of the most recent range.**
/// `uniqExact` builds a hash set proportional to cardinality, and over a
/// saturated run's 229M rows that exhausted ClickHouse's memory limit outright —
/// so the exact gate is bounded, and the slice is taken from the top of the range
/// because that is the part produced during and after the measurement window.
/// The slice is still tens of millions of rows; a framework that drops,
/// duplicates or mis-transforms does so systematically, not once.
///
/// # Panics
/// If the table cannot be queried, or ClickHouse returns unparseable output — a
/// gate that silently degrades to "passed" would be worse than no gate.
#[must_use]
pub fn run_gates(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    tier: Tier,
    max_batches: u64,
) -> Gates {
    let sql = |q: &str| -> Vec<String> {
        let body = crate::docker::clickhouse_sql(host, port, user, password, q)
            .unwrap_or_else(|e| panic!("gate query failed ({q}): {e}"));
        body.trim().split(['\t', '\n']).map(str::to_owned).collect()
    };
    let num = |s: &str| -> i128 {
        s.parse::<i128>()
            .unwrap_or_else(|e| panic!("gate expected a number, got {s:?}: {e}"))
    };

    let table = tier.table();
    let bounds = sql(&format!("SELECT min(batch_id), max(batch_id) FROM {table}"));
    assert!(
        bounds.len() >= 2,
        "gate bounds query returned {bounds:?} for {table}"
    );
    // An empty table yields 0/0 from min/max; treat it as a hard failure rather
    // than an empty-but-passing range.
    let (min_batch, max_batch) = (num(&bounds[0]) as u64, num(&bounds[1]) as u64);
    assert!(
        max_batch > min_batch + 1,
        "{table} holds too narrow a batch_id range ([{min_batch}, {max_batch}]) to gate; \
         the arm produced almost nothing"
    );
    // Bounded slice from the top of the range; see the doc comment.
    let hi = max_batch;
    let lo = (min_batch + 1).max(hi.saturating_sub(max_batches));

    let counts = sql(&format!(
        "SELECT count(), uniqExact((batch_id, event_seq)), uniqExact(batch_id) FROM {table} \
         WHERE batch_id >= {lo} AND batch_id < {hi}"
    ));
    assert!(counts.len() >= 3, "gate count query returned {counts:?}");
    let rows = num(&counts[0]) as u64;
    let distinct_ids = num(&counts[1]) as u64;
    let distinct_batches = num(&counts[2]) as u64;

    // The sums are taken over DEDUPLICATED rows, which is not a detail: these are
    // at-least-once systems, so a legitimate duplicate would otherwise inflate
    // `sum(value)` and fail a *correct* arm. Deduplicating on the full row is
    // sound because a replayed record re-encodes identically.
    //
    // `toInt128` because ClickHouse's `sum` over `Int64` returns `Int64`, which a
    // large corpus would overflow silently.
    let (proj, scaled_sum) = if tier == Tier::B {
        (
            "batch_id, event_seq, value, value_scaled",
            ", sum(toInt128(value_scaled))",
        )
    } else {
        ("batch_id, event_seq, value", "")
    };
    let sums = sql(&format!(
        "SELECT sum(toInt128(value)){scaled_sum} FROM \
         (SELECT DISTINCT {proj} FROM {table} WHERE batch_id >= {lo} AND batch_id < {hi})"
    ));
    assert!(!sums.is_empty(), "gate sum query returned {sums:?}");
    let value_sum = num(&sums[0]);
    let value_scaled_sum = if tier == Tier::B { num(&sums[1]) } else { 0 };

    let exp = expected_range(lo, hi, tier);
    Gates {
        min_batch,
        max_batch,
        rows,
        distinct_ids,
        distinct_batches,
        duplicates: rows.saturating_sub(distinct_ids),
        contiguous: distinct_batches == hi - lo,
        // Compared against distinct ids, so a duplicate cannot mask a loss.
        rows_match: distinct_ids == exp.rows,
        value_sum_match: value_sum == exp.value_sum,
        value_scaled_match: value_scaled_sum == exp.value_scaled_sum,
    }
}

/// Consume the first `sample` messages of `topic` and prove that what is
/// actually on the wire matches the contract.
///
/// This exists because the round-trip unit test only proves `encode_batch` and
/// the typed decoder agree with each other. It does not prove that the *framed
/// bytes sitting in Kafka* are what a registry-based consumer expects — and
/// every competitor arm reads those bytes, not our encoder. So this checks the
/// Confluent header byte-for-byte, checks the embedded schema id, decodes the
/// datum, and re-derives every field from `batch_id` to confirm it.
///
/// Returns the number of messages verified.
///
/// # Panics
/// On any framing, schema-id, decode or field mismatch. A corpus that does not
/// match the contract invalidates the entire run, not one arm.
pub fn verify_corpus(bootstrap: &str, topic: &str, schema_id: u32, sample: u64) -> u64 {
    use rdkafka::consumer::{Consumer, base_consumer::BaseConsumer};
    use rdkafka::message::Message;
    use rdkafka::{Offset, TopicPartitionList, config::ClientConfig};
    use std::time::{Duration, Instant};

    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", bootstrap)
        .set("group.id", "comparison-corpus-verify")
        .set("enable.auto.commit", "false")
        .create()
        .expect("verify consumer");
    let mut tpl = TopicPartitionList::new();
    // Partition 0 only: prefill assigns round-robin, so partition 0 holds every
    // `partitions`-th batch_id — enough to exercise both union branches and a
    // spread of tag lengths without consuming the whole corpus.
    tpl.add_partition_offset(topic, 0, Offset::Beginning)
        .expect("assign offset");
    consumer.assign(&tpl).expect("assign");

    let mut seen = 0u64;
    let deadline = Instant::now() + Duration::from_secs(60);
    while seen < sample {
        assert!(
            Instant::now() < deadline,
            "only verified {seen} of {sample} messages before the deadline"
        );
        let Some(result) = consumer.poll(Duration::from_millis(500)) else {
            continue;
        };
        let msg = result.expect("consume");
        let payload = msg.payload().expect("message has a payload");

        assert!(
            payload.len() > 5,
            "payload is {} bytes, too short to be Confluent-framed",
            payload.len()
        );
        assert_eq!(payload[0], 0x00, "Confluent magic byte");
        let embedded = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
        assert_eq!(embedded, schema_id, "embedded schema id");

        let value = apache_avro::from_avro_datum(schema(), &mut &payload[5..], None)
            .expect("decode datum from the wire");
        let batch: SensorBatch = apache_avro::from_value(&value).expect("into SensorBatch");
        let batch_id = u64::try_from(batch.batch_id).expect("batch_id non-negative");

        assert_eq!(batch.sensor, sensor_of(batch_id), "sensor for {batch_id}");
        assert_eq!(batch.region, region_of(batch_id), "region for {batch_id}");
        assert_eq!(
            batch.batch_ts_ms,
            batch_ts_ms_of(batch_id),
            "batch_ts_ms for {batch_id}"
        );
        assert_eq!(
            batch.send_ts_us,
            send_ts_us_prefill(batch_id),
            "send_ts_us for {batch_id}"
        );
        assert_eq!(
            u32::try_from(batch.events.len()).expect("event count fits u32"),
            EVENTS_PER_BATCH,
            "event count for {batch_id}"
        );
        for ev in &batch.events {
            let seq = u32::try_from(ev.seq).expect("seq non-negative");
            assert_eq!(ev.name, name_of(batch_id, seq), "name {batch_id}/{seq}");
            assert_eq!(ev.unit, unit_of(batch_id, seq), "unit {batch_id}/{seq}");
            assert_eq!(ev.value, value_of(batch_id, seq), "value {batch_id}/{seq}");
            assert_eq!(
                ev.quality,
                quality_of(batch_id, seq),
                "quality {batch_id}/{seq}"
            );
            assert_eq!(ev.tags, tags_of(batch_id, seq), "tags {batch_id}/{seq}");
        }
        seen += 1;
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_schema_parses() {
        let s = schema();
        assert!(
            matches!(s, Schema::Record(r) if r.name.name == "SensorBatch"),
            "expected a SensorBatch record schema"
        );
    }

    /// The load-bearing test: an encoded datum must decode back into the typed
    /// struct with every field intact, including both unions and the inner
    /// array. If this passes, the explicit `AvroValue` construction agrees with
    /// the committed schema.
    #[test]
    fn a_batch_round_trips_through_the_typed_decoder() {
        for batch_id in [0u64, 1, 9, 10, 37, 1023, 1024] {
            let datum = encode_batch(batch_id, 42);
            let value = apache_avro::from_avro_datum(schema(), &mut datum.as_slice(), None)
                .expect("decode");
            let decoded: SensorBatch = apache_avro::from_value(&value).expect("into struct");

            assert_eq!(decoded.batch_id, i64::try_from(batch_id).unwrap());
            assert_eq!(decoded.sensor, sensor_of(batch_id));
            assert_eq!(decoded.region, region_of(batch_id));
            assert_eq!(decoded.batch_ts_ms, batch_ts_ms_of(batch_id));
            assert_eq!(decoded.send_ts_us, 42);
            assert_eq!(decoded.events.len() as u32, EVENTS_PER_BATCH);

            for (seq, ev) in (0u32..).zip(&decoded.events) {
                assert_eq!(ev.seq, i32::try_from(seq).unwrap());
                assert_eq!(ev.name, name_of(batch_id, seq));
                assert_eq!(ev.unit, unit_of(batch_id, seq));
                assert_eq!(ev.value, value_of(batch_id, seq));
                assert_eq!(ev.quality, quality_of(batch_id, seq));
                assert_eq!(ev.tags, tags_of(batch_id, seq));
            }
        }
    }

    /// Both nullable branches must actually occur in a small corpus, or the
    /// round-trip test above would be passing without ever exercising a union.
    #[test]
    fn both_union_branches_occur() {
        assert!(region_of(0).is_none(), "batch 0 has a null region");
        assert!(region_of(1).is_some(), "batch 1 has a present region");
        let qualities: Vec<_> = (0..EVENTS_PER_BATCH).map(|s| quality_of(3, s)).collect();
        assert!(
            qualities.iter().any(Option::is_none),
            "some quality is null"
        );
        assert!(qualities.iter().any(Option::is_some), "some quality is set");
    }

    #[test]
    fn the_generator_is_deterministic() {
        assert_eq!(encode_batch(12_345, 7), encode_batch(12_345, 7));
        assert_ne!(encode_batch(12_345, 7), encode_batch(12_346, 7));
    }

    #[test]
    fn confluent_framing_is_five_bytes_of_header() {
        let framed = frame_confluent(0x0102_0304, &[0xAA, 0xBB]);
        assert_eq!(framed, vec![0x00, 0x01, 0x02, 0x03, 0x04, 0xAA, 0xBB]);
    }

    #[test]
    fn value_is_never_negative_so_truncation_cannot_differ_across_languages() {
        for batch_id in [0u64, 1, 999, 1_000_000, 5_000_000] {
            for seq in 0..EVENTS_PER_BATCH {
                assert!(value_of(batch_id, seq) >= 0);
            }
        }
    }

    /// The committed DDL must yield exactly the two `CREATE TABLE` statements
    /// and nothing else — in particular the trailing `--` comments documenting
    /// the gate queries must not be mistaken for executable SQL.
    #[test]
    fn ddl_splits_into_the_two_creates_only() {
        let stmts = ddl_statements();
        assert_eq!(stmts.len(), 2, "expected 2 statements, got {stmts:#?}");
        assert!(stmts[0].contains("CREATE TABLE IF NOT EXISTS sensor_events"));
        assert!(stmts[1].contains("CREATE TABLE IF NOT EXISTS sensor_events_t"));
        for s in &stmts {
            assert!(
                !s.contains("uniqExact"),
                "a documented gate query leaked into executable DDL: {s}"
            );
        }
    }

    #[test]
    fn comment_stripping_does_not_eat_code_on_the_same_line() {
        let stmts = split_sql("CREATE TABLE t (a UInt64) -- note; with a semicolon\n;");
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0], "CREATE TABLE t (a UInt64)");
    }

    /// Every column the DDL declares must appear in the positional column list
    /// for that tier, in the same order. The column order *is* the RowBinary and
    /// Native wire contract, so a silent divergence here would corrupt every
    /// row rather than fail loudly.
    #[test]
    fn declared_columns_match_the_ddl_in_order() {
        let stmts = ddl_statements();
        for (tier, stmt) in [(Tier::A, &stmts[0]), (Tier::B, &stmts[1])] {
            let mut cursor = 0usize;
            for (name, ty) in tier.columns() {
                let needle = format!("\n    {name} ");
                let at = stmt[cursor..]
                    .find(&needle)
                    .map(|i| i + cursor)
                    .unwrap_or_else(|| {
                        panic!(
                            "column {name} missing from {} DDL or out of order",
                            tier.table()
                        )
                    });
                // The declared type must match too, or Native would encode a
                // leaf the server reads as a different type.
                let line_end = stmt[at + 1..].find('\n').map_or(stmt.len(), |i| at + 1 + i);
                let line = &stmt[at..line_end];
                assert!(
                    line.contains(ty),
                    "column {name} in {} is declared {line:?}, expected type {ty}",
                    tier.table()
                );
                cursor = at + 1;
            }
        }
    }

    /// A range's expectation must be the difference of two prefixes, or the
    /// sustained gates and the drain gates would disagree about the same rows.
    #[test]
    fn a_range_expectation_is_the_difference_of_two_prefixes() {
        for tier in [Tier::A, Tier::B] {
            let prefix_700 = expected(700, tier);
            let prefix_200 = expected(200, tier);
            let range = expected_range(200, 700, tier);
            assert_eq!(
                range.rows,
                prefix_700.rows - prefix_200.rows,
                "{tier:?} rows"
            );
            assert_eq!(
                range.value_sum,
                prefix_700.value_sum - prefix_200.value_sum,
                "{tier:?} value_sum"
            );
            assert_eq!(
                range.value_scaled_sum,
                prefix_700.value_scaled_sum - prefix_200.value_scaled_sum,
                "{tier:?} value_scaled_sum"
            );
        }
    }

    #[test]
    fn ascii_upper_is_ascii_only() {
        assert_eq!(ascii_upper("metric_7"), "METRIC_7");
        // Left alone, which is the property that makes the operation identical
        // in Rust, Java and ClickHouse regardless of locale.
        assert_eq!(ascii_upper("straße"), "STRAßE");
    }

    /// Tier A keeps everything; tier B must drop strictly more than nothing and
    /// strictly less than everything, or the filter is not being exercised.
    #[test]
    fn tier_b_filters_a_meaningful_fraction() {
        let batches = 500;
        let a = expected(batches, Tier::A);
        let b = expected(batches, Tier::B);
        assert_eq!(a.rows, batches * u64::from(EVENTS_PER_BATCH));
        assert_eq!(a.value_scaled_sum, 0, "tier A has no scaled column");
        assert!(
            b.rows > 0 && b.rows < a.rows,
            "tier B dropped {} of {}",
            a.rows - b.rows,
            a.rows
        );
        // The unit sentinel alone removes one row in eight; the quality floor
        // removes more. Anything outside this band means a derivation drifted.
        let dropped = (a.rows - b.rows) as f64 / a.rows as f64;
        assert!(
            (0.12..0.45).contains(&dropped),
            "tier B dropped {dropped:.3} of rows, outside the expected band"
        );
        assert!(b.value_scaled_sum > 0);
    }

    /// `expected` must agree with an independent flatten of the same corpus,
    /// so a mistake in the accumulator cannot pass as an expectation.
    #[test]
    fn expectations_agree_with_an_independent_flatten() {
        let batches = 200u64;
        let mut rows = 0u64;
        let mut sum = 0i128;
        for batch_id in 0..batches {
            let datum = encode_batch(batch_id, 0);
            let v = apache_avro::from_avro_datum(schema(), &mut datum.as_slice(), None)
                .expect("decode");
            let b: SensorBatch = apache_avro::from_value(&v).expect("into struct");
            for ev in &b.events {
                let seq = u32::try_from(ev.seq).unwrap();
                if !tier_b_keeps(batch_id, seq) {
                    continue;
                }
                rows += 1;
                sum += i128::from(ev.value);
            }
        }
        let exp = expected(batches, Tier::B);
        assert_eq!(exp.rows, rows);
        assert_eq!(exp.value_sum, sum);
    }
}
