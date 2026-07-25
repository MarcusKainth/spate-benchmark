-- Shared ClickHouse target schema for the cross-framework comparison.
--
-- Every framework writes THESE tables, with THIS column order. The order is the
-- wire contract for both RowBinary and Native, so a reordering here is a
-- breaking change to every implementation in entrants/.
--
-- The driver TRUNCATEs between arms rather than recreating, so all arms share one
-- table definition and therefore one set of server-side costs. Nothing about the
-- target may differ per framework — if it ever has to, that arm is not
-- comparable and must not be published.

-- ---------------------------------------------------------------------------
-- Tier A — transport. Decode, flatten, insert. Column mapping only.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS sensor_events
(
    batch_id   UInt64,
    event_seq  UInt16,
    sensor     LowCardinality(String),
    -- NOT LowCardinality(Nullable(String)): the Native encoder rejects a
    -- non-String inner. The Avro null is coalesced to '' by the pipeline, which
    -- is why even tier A does a small amount of real work.
    region     LowCardinality(String),
    name       LowCardinality(String),
    unit       LowCardinality(String),
    value      Int64,
    quality    Nullable(Float64),
    tags       Array(LowCardinality(String)),
    batch_ts   DateTime64(3),
    -- Producer's INTENDED send time. The Native leaf writer does no DateTime64
    -- rescaling, so the Spate side must use the DateTime64Micros wrapper
    -- newtype here or every value silently lands in 1970.
    send_ts    DateTime64(6),
    -- Computed server-side at insert, identically for every framework and every
    -- wire format. This column is the entire latency measurement: no framework
    -- reports its own latency, and there is only one definition of "arrived".
    ingest_ts  DateTime64(6) MATERIALIZED now64(6)
)
ENGINE = MergeTree
ORDER BY (sensor, batch_ts, batch_id, event_seq)
-- On plain MergeTree this window defaults to 0, which makes
-- `insert_deduplication_token` silently do nothing. It is set here so the
-- setting is identical for all arms; only Spate actually sends tokens today,
-- and the page discloses that. Any framework could send them.
SETTINGS non_replicated_deduplication_window = 1000;

-- ---------------------------------------------------------------------------
-- Tier B — transform. Tier A plus filtering and derivation.
-- ---------------------------------------------------------------------------
-- Specified work, in this order:
--   1. drop rows where unit = 'drop'                       (~12.5% of rows)
--   2. drop rows where quality IS NOT NULL AND quality < 0.2
--   3. coalesce a null region to ''
--   4. value_scaled = value * 1000 / (event_seq + 1), integer division,
--      truncating toward zero
--   5. name_upper  = ASCII-only uppercase of name
--
-- Step 5 is ASCII-only ON PURPOSE. Java's String.toUpperCase() is
-- locale-dependent, so an unqualified "uppercase" would not be the same
-- operation in every implementation. Metric names are drawn from a fixed set of
-- lowercase ASCII identifiers, which makes ASCII uppercase unambiguous.
CREATE TABLE IF NOT EXISTS sensor_events_t
(
    batch_id     UInt64,
    event_seq    UInt16,
    sensor       LowCardinality(String),
    region       LowCardinality(String),
    name_upper   LowCardinality(String),
    unit         LowCardinality(String),
    value        Int64,
    value_scaled Int64,
    quality      Nullable(Float64),
    tags         Array(LowCardinality(String)),
    batch_ts     DateTime64(3),
    send_ts      DateTime64(6),
    ingest_ts    DateTime64(6) MATERIALIZED now64(6)
)
ENGINE = MergeTree
ORDER BY (sensor, batch_ts, batch_id, event_seq)
SETTINGS non_replicated_deduplication_window = 1000;

-- ---------------------------------------------------------------------------
-- The gates the driver runs after every arm, before it emits anything.
-- ---------------------------------------------------------------------------
-- Loss (must equal the expected row count exactly — a framework that drops
-- rows is faster for the wrong reason, and its arm is void):
--
--   SELECT uniqExact((batch_id, event_seq)) FROM sensor_events;
--
-- Duplicates (reported as a metric, never suppressed: these are all
-- at-least-once systems and some duplication is legitimate):
--
--   SELECT count() - uniqExact((batch_id, event_seq)) FROM sensor_events;
--
-- Same-work checksum (proves two frameworks did the same arithmetic, not just
-- that they moved the same number of rows; compared against the driver's
-- closed-form expectation).
--
-- Taken over DEDUPLICATED rows, which matters: every arm here is at-least-once,
-- so a legitimate duplicate would otherwise inflate the sum and fail a correct
-- arm. `toInt128` because `sum` over `Int64` returns `Int64` and a large corpus
-- would overflow it silently.
--
--   SELECT sum(toInt128(value)) FROM
--     (SELECT DISTINCT batch_id, event_seq, value FROM sensor_events);
--
-- The driver additionally excludes the lowest and highest batch_id: a sealed sink
-- chunk can split one message's rows across two batches, so the boundary batches
-- may be only partially landed at the instant of the snapshot.
--
-- End-to-end latency (one definition, server-side, no framework cooperation):
--
--   SELECT quantiles(0.5, 0.9, 0.99, 0.999)(
--            (toUnixTimestamp64Micro(ingest_ts) - toUnixTimestamp64Micro(send_ts)) / 1e6
--          ) FROM sensor_events;
