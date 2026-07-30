-- The Kafka Connect arm's ClickHouse objects, applied by the harness per-entrant
-- DDL hook before every repetition (and torn down by arm_teardown.sql first, so
-- nothing survives from one repetition into the next).
--
-- Connect has no fan-out operator: one Kafka record cannot become ~100 rows
-- inside the runtime. So the connector lands the NESTED batch here and the
-- materialized view below performs the flatten, both filters and both derived
-- columns on the server. That moves the transform's CPU into the shared
-- ClickHouse, where the cgroup sampler cannot see it — declared in
-- [[deviations]] in entrant.toml, and the reason this arm's efficiency numbers
-- lean on system.query_log's ProfileEvents.
--
-- ENGINE = Null: a batch is consumed by the MV at insert time and never stored.
-- Storing the nested form as well would add roughly 6 GiB of writes (1.5M
-- messages at a ~4 KiB mean framed message) plus the merges they trigger —
-- server-side work that belongs to no other arm and that query_log's insert
-- rows would not even carry (merges live in part_log).
--
-- Column types are the Avro schema's, verbatim: long -> Int64, int -> Int32,
-- ["null","double"] -> Nullable(Float64). Every conversion to the target's
-- types happens in the MV, so the landing insert is a plain decode-and-write —
-- the same division of labour every other arm has between its decoder and its
-- transform. `Nested(...)` is NOT usable here: the connector serializes against
-- DESCRIBE of the target (describe_include_subcolumns=1) and skips Nested
-- columns with a warning rather than mapping them (Column.java at v1.4.0), so
-- the array-of-record maps to Array(Tuple(...)).
--
-- Plain CREATE, not IF NOT EXISTS: the harness tears these objects down before
-- every repetition's create, so an object that already exists here means the
-- teardown file drifted — and that should fail this arm loudly, not persist a
-- stale definition across repetitions.
CREATE TABLE sensor_batches_landing
(
    batch_id    Int64,
    sensor      String,
    region      Nullable(String),
    batch_ts_ms Int64,
    send_ts_us  Int64,
    events Array(Tuple(
        seq     Int32,
        name    String,
        unit    String,
        value   Int64,
        quality Nullable(Float64),
        tags    Array(String)
    ))
)
ENGINE = Null;

-- The transform, restated per methodology/ (the flatten, the filters and the
-- derived columns are user code in every system, and every arm writes them):
--
--   1. drop rows where unit = 'drop'        \  WHERE runs before SELECT, so the
--   2. drop rows where quality < 0.2 (non-null) /  filters precede the derives
--   3. coalesce null region to ''              (ifNull)
--   4. value_scaled = value * 1000 / (seq + 1) (the spec asks truncation
--      toward zero; ClickHouse's intDiv is floor division per its docs — both
--      operands are non-negative by corpus construction, so the two agree)
--   5. name_upper = ASCII-only uppercase       (ClickHouse's upper() is
--      documented ASCII-only — the locale trap the contract names is Java's)
--
-- The 'drop' and 0.2 literals are the workload's (workload.toml: drop_unit,
-- quality_floor); harness/tests/each_arm_restates_the_transform.rs holds this
-- file to them.
--
-- ingest_ts is deliberately NOT selected: it is MATERIALIZED on the target and
-- stamps during the parent INSERT — the same moment, by the same server clock,
-- as every other arm's rows.
--
-- (If `e.seq` dot-access ever fails on a future ClickHouse in the MV context,
-- `tupleElement(e, 'seq')` is the equivalent spelling.)
CREATE MATERIALIZED VIEW sensor_batches_mv TO sensor_events AS
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
FROM sensor_batches_landing
ARRAY JOIN events AS e
WHERE e.unit != 'drop'
  AND (e.quality IS NULL OR e.quality >= 0.2);
