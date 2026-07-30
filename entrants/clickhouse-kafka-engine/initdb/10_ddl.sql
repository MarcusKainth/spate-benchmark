-- The whole pipeline, as three static objects: a Kafka engine table (consume +
-- decode), a materialized view (flatten + filter + derive), and a Distributed
-- table (forward to the shared server's `sensor_events`). STATIC on purpose:
-- every tunable lives in the `kafka_src` named collection and the default
-- profile (config.d/, users.d/), fed by `from_env`, so this file never needs a
-- template pass and what a reviewer reads here is exactly what runs.
--
-- Ordering matters once: the MV is created LAST. A Kafka engine table only
-- consumes while it has a dependent view, so nothing is read from the topic
-- until the full decode -> transform -> forward path exists.

-- ---------------------------------------------------------------------------
-- 1. Consume + decode. `ENGINE = Kafka(kafka_src)` takes every kafka_* setting
--    from the named collection in config.d/20-kafka-collection.xml — broker,
--    topic, group, consumer count, block size, flush cadence — so the DDL
--    carries the schema and nothing else.
--
--    The column set is the AvroConfluent mapping of workload/schema/
--    sensor_batch.avsc, mechanical and total:
--      union(null,T)  -> Nullable(T)      (region, and events[].quality below)
--      record         -> Tuple(...)       (the Event record)
--      array<record>  -> Array(Tuple(...))(the events fan-out array)
--    The schema itself is fetched from the live registry per schema id in the
--    Confluent frame (format_avro_schema_registry_url, users.d/10-profile.xml)
--    — nobody re-declares the schema inline, same as every arm.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS default.sensor_batches_queue
(
    batch_id    Int64,
    sensor      String,
    region      Nullable(String),
    batch_ts_ms Int64,
    send_ts_us  Int64,
    events      Array(Tuple(
        seq     Int32,
        name    String,
        unit    String,
        value   Int64,
        quality Nullable(Float64),
        tags    Array(String)
    ))
)
ENGINE = Kafka(kafka_src);

-- ---------------------------------------------------------------------------
-- 2. Forward. A Distributed table over the one-node `bench_target` cluster
--    (config.d/10-remote-cluster.xml), whose single replica is the SHARED
--    infra ClickHouse that owns `sensor_events` for every arm. With
--    distributed_foreground_insert = 1 (users.d/10-profile.xml) each MV insert
--    blocks until the remote server has acked the block, so the Kafka engine's
--    offset commit — which happens after the block is written through the MVs
--    — implies the rows are on the shared server. That one setting is what
--    makes this topology honest: the default (0) spools to local disk and acks
--    early, and offsets would commit before the rows existed remotely.
--
--    The 12 physical columns and NO ingest_ts, deliberately: MATERIALIZED columns
--    must not transit Distributed inserts (ClickHouse issues #4015, #9439).
--    The shared server computes `ingest_ts MATERIALIZED now64(6)` when the
--    forwarded INSERT lands there — the same per-INSERT stamp every arm gets,
--    so this arm's latency honestly includes the forward hop.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS default.sensor_events_dist
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
    send_ts      DateTime64(6)
)
ENGINE = Distributed('bench_target', 'default', 'sensor_events');

-- ---------------------------------------------------------------------------
-- 3. Transform. The specified work, restated here as this arm's own SQL (an
--    arm may not import its transform from the oracle that marks it), in the
--    specified order: the two filters in WHERE, then the coalesce and the two
--    derived columns in the SELECT.
--
--    * ARRAY JOIN is the flatten: one decoded message row becomes one row per
--      element of `events`, aliased `e`.
--    * The unit sentinel and the quality floor in the WHERE clause are the
--      workload's literals; harness/tests/each_arm_restates_the_transform.rs
--      holds their spellings to workload/workload.toml. (Deliberately not
--      spelled out in this comment: that test checks the predicates as
--      source text, and a comment that contained them would satisfy it.)
--    * `ifNull(region, '')` is the null coalesce forced by the target's
--      LowCardinality(String).
--    * `upper()` is ASCII-only per ClickHouse's string-function docs — which
--      is exactly what the contract specifies. upperUTF8 is the Unicode one
--      and must NOT be used here.
--    * `intDiv` is integer division truncating toward zero. Range is safe:
--      corpus value >= 0, seq+1 >= 1, and value*1000 <~ 2.1e12 fits Int64.
--    * fromUnixTimestamp64Milli/Micro take no timezone argument: they map an
--      epoch integer to DateTime64 1:1, which is the identity the gate's
--      closed-form expectation assumes.
-- ---------------------------------------------------------------------------
CREATE MATERIALIZED VIEW IF NOT EXISTS default.sensor_events_mv
TO default.sensor_events_dist
AS
SELECT
    toUInt64(batch_id)                    AS batch_id,
    toUInt16(e.seq)                       AS event_seq,
    sensor                                AS sensor,
    ifNull(region, '')                    AS region,
    upper(e.name)                         AS name_upper,
    e.unit                                AS unit,
    e.value                               AS value,
    intDiv(e.value * 1000, e.seq + 1)     AS value_scaled,
    e.quality                             AS quality,
    e.tags                                AS tags,
    fromUnixTimestamp64Milli(batch_ts_ms) AS batch_ts,
    fromUnixTimestamp64Micro(send_ts_us)  AS send_ts
FROM default.sensor_batches_queue
ARRAY JOIN events AS e
WHERE e.unit != 'drop'
  AND NOT (e.quality IS NOT NULL AND e.quality < 0.2);
