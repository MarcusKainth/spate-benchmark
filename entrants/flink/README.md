# The Apache Flink arm

Kafka → Confluent-framed Avro → `flatMap` → ClickHouse, held to
[`../README.md`](../README.md), which is normative. Read that first; this file
records only what is specific to Flink: the exact coordinates, every
configuration value and its reason, every deviation, and the findings about
Flink's own defaults that the comparison is obliged to publish.

## Build and run

```sh
# Build context is the REPOSITORY ROOT, uniformly for every entrant: the build
# needs entrants/flink/ and workload/schema/ and nothing else.
docker build -f entrants/flink/Dockerfile \
  -t spate-bench-flink .

# Shared checkpoint storage. Both halves mount it, so recovery is real rather
# than nominal (see "Checkpoint storage" below).
docker volume create spate-bench-flink-checkpoints

# JobManager: 1 CPU / 2 GiB, control plane, allocated on top of the envelope.
# `standalone-job` is Application Mode — the JobManager runs the job's main()
# and submits it, so there is no separate `flink run` step.
docker run -d --name spate-bench-flink-jm --network spate-bench-net \
  --cpus 1 --memory 2g --memory-swap 2g \\
  -e JOB_MANAGER_RPC_ADDRESS=spate-bench-flink-jm \
  -e TIER=a \
  -v spate-bench-flink-checkpoints:/opt/flink/checkpoints \
  -p 18085:8081 \
  spate-bench-flink standalone-job

# TaskManager: the full 4 CPU / 16 GiB data-plane envelope.
docker run -d --name spate-bench-flink-tm --network spate-bench-net \
  --cpus 4 --memory 16g --memory-swap 16g \\
  -e JOB_MANAGER_RPC_ADDRESS=spate-bench-flink-jm \
  -v spate-bench-flink-checkpoints:/opt/flink/checkpoints \
  spate-bench-flink taskmanager
```

`--memory-swap` equals `--memory` on both, so memory pressure surfaces instead
of hiding in a swapfile. The job never terminates itself: the source is
unbounded and the driver removes the containers.

A session cluster works too (`jobmanager` instead of `standalone-job`, then
`flink run /opt/flink/usrlib/comparison-flink.jar`), which is useful when you
want to submit the same jar twice with different `TIER`.

`TIER=b` targets `sensor_events_t` automatically; `CLICKHOUSE_TABLE` overrides
it. `DESER=reusing` selects the secondary arm described at the bottom of this
file — never the headline.

## Exact versions

| Component | Coordinate / image | Version |
|---|---|---|
| Flink runtime | `flink:2.2.1-java17` (digest `sha256:3d050f35…8f1c`) | 2.2.1 |
| JVM | Temurin (image default) | 17.0.19+10 |
| Build JDK | `maven:3.9-eclipse-temurin-17` | 17 |
| Kafka connector | `org.apache.flink:flink-connector-kafka` | `5.0.0-2.2` |
| Kafka client | `org.apache.kafka:kafka-clients` (transitive) | 4.2.0 |
| Avro format | `org.apache.flink:flink-avro` | 2.2.1 |
| Confluent registry format | `org.apache.flink:flink-avro-confluent-registry` | 2.2.1 |
| Avro | `org.apache.avro:avro` (transitive) | 1.11.4 |
| Schema Registry client | `io.confluent:kafka-schema-registry-client` (transitive) | 7.5.3 |
| ClickHouse sink | `com.clickhouse.flink:flink-connector-clickhouse-2.0.0`, classifier `all` | 0.2.0 |

The full resolved graph is baked into the image at
`/opt/flink/usrlib/dependencies.txt`, because Maven has no lockfile and the
resolved graph is the only thing a later re-run can be compared against. The job
jar is ~38 MB.

**Two coordinate corrections to the brief.** The ClickHouse connector is *not*
`com.clickhouse.flink:flink-connector-clickhouse-2.0.0` at version `2.0.0`:
`2.0.0` is part of the **artifactId** (it names the Flink minor the artifact
targets) and the connector's own version is `0.2.0`. The artifact is also
`pom`-packaged with a single `all`-classifier jar, so the dependency needs
`<classifier>all</classifier>` or Maven resolves only the pom and the build fails
with `NoClassDefFoundError` at submission rather than at resolution. And
`flink-avro-confluent-registry` pulls `io.confluent:kafka-schema-registry-client`,
which is **not on Maven Central** — `pom.xml` declares
`https://packages.confluent.io/maven/`, exactly as Flink's own pom does.

## Insert format

**`RowBinaryWithNamesAndTypes`, uncompressed, over HTTP.**

The connector's typed (POJO) mode forces this format and ignores
`setClickHouseFormat`, so it is not a choice we made; the alternative shipped
path is String mode, which means building CSV or `JSONEachRow` text per row and
moving parsing to the server. Typed mode also makes the connector set
`input_format_null_as_default=1` and `input_format_defaults_for_omitted_fields=1`
server-side; we supply every non-materialised column, so neither changes the
result here.

Compared to the Spate arm this is not the same amount of server-side work.
`RowBinaryWithNamesAndTypes` carries a names+types header per insert (~240 bytes
at 11 columns — negligible) but, more importantly, it is a **row**-oriented
format, so ClickHouse pivots every row into columns server-side, where Spate's
`native` path arrives pre-pivoted. The Spate arm publishes a `rowbinary` control
number for exactly this reason; that control, not the `native` number, is the
like-for-like comparison for this arm.

Request compression is off (the client's default). Turning it on would trade our
3 TaskManager CPUs for network bytes on a link that is a loopback bridge.

## Configuration, and why

Everything tunable lives in [`config.yaml`](config.yaml) or in the sink's
env-driven batch settings, and nothing tunable lives in the Java. That is
deliberate: a reviewer should be able to read the whole tuning surface without
decompiling a jar. `config.yaml` is heavily commented; this table is the summary.

### Flink 2.x reads `config.yaml` only

`flink-conf.yaml` was removed in 2.0. A file by that name is silently ignored —
which is the worst possible failure mode for a benchmark, because the job runs
and the numbers are simply wrong.

Our `config.yaml` **replaces** the image's, so it must also carry forward what
the image needs. The dangerous entry is `env.java.opts.all`: Flink 2.x on Java 17
needs those `--add-opens`/`--add-exports` and loses them silently if you write a
fresh file. The Dockerfile diffs our copy against the base image's and **fails
the build on drift**.

### Memory

| Key | Value | Why |
|---|---|---|
| `taskmanager.memory.process.size` | `2900m` | The right knob inside a container: total process memory, from which Flink derives heap, direct and metaspace. Leaves 172 MiB of the 3 GiB container as slack outside the JVM's accounting. |
| `taskmanager.memory.managed.fraction` | **`0.0`** | The single most important line. Default `0.4` *reserves* managed memory whether or not anything uses it, and a stateless job on the `hashmap` backend uses none — no RocksDB/ForSt block cache, no batch sorter, no Python. At 0.4 of 2354m that is ~940 MiB of a 3 GiB budget allocated and never touched. Leaving the default would have made this comparison unfair to Flink. |
| `taskmanager.memory.task.off-heap.size` | `128m` | `-XX:MaxDirectMemorySize` is derived from framework off-heap + task off-heap + network. The ClickHouse sink is the only user-code component doing socket I/O; 128m of headroom means a burst surfaces as backpressure rather than as `OutOfMemoryError: Direct buffer memory`, which would read as a Flink defect. |
| `jobmanager.memory.process.size` | `960m` | 1 GiB container, 64 MiB slack. Yields ~384m JM heap, ample for one stateless job at parallelism 8 with filesystem checkpoint storage. |

Derived, for the record: TaskManager `-Xmx 1863m`,
`-XX:MaxDirectMemorySize 491m`, network 235m, metaspace 256m.

### Parallelism

| Key | Value | Why |
|---|---|---|
| `parallelism.default` | `8` | The topic has 8 partitions. At 8, each subtask owns exactly one partition: none multiplexes and none idles. |
| `taskmanager.numberOfTaskSlots` | `8` | One slot per subtask in a single TaskManager. |

**This is the setting most likely to be wrong, and it is called out rather than
buried.** 8 subtasks share 3 CPUs, so the TaskManager is oversubscribed 2.7:1 —
8 Kafka consumers with their own fetch buffers, 8 independent sink buffers, and 8
sets of thread stacks inside a 1.86 GiB heap. Matching parallelism to *cores*
(4, two partitions per subtask) would cut sink buffer memory in half and reduce
context switching, at the cost of one subtask per two partitions. Both are
defensible and the difference is measurable; the driver can sweep it because
parallelism is config, not code. See "Where we may be unfair to Flink".

### Throughput

| Key | Value | Why |
|---|---|---|
| `execution.buffer-timeout.enabled` | `false` | The documented maximum-throughput setting: flush a network buffer only when it is full, never because a timer fired. **In 2.x the old `execution.buffer-timeout` duration no longer exists** — it split into `.enabled` (default `true`) and `.interval` (default 100ms), so setting the old key is accepted-but-deprecated rather than doing what you meant. |
| `pipeline.object-reuse` | `true` | Chained operators hand the same reference downstream instead of copying. `FlattenTierA`/`FlattenTierB` are written for it: one output row is re-used across the whole fan-out. |
| `pipeline.operator-chaining.enabled` | `true` (default, stated) | Load-bearing. Source, flatMap and sink writer are **one chain**, so no `GenericRecord` and no output row is ever serialized. |
| `taskmanager.network.memory.buffer-debloat.enabled` | `false` (default, stated) | Debloating trades throughput for lower checkpoint latency. This arm is measured on throughput. |
| `partition.discovery.interval.ms` | `-1` | Set on the source. Partition count is fixed for the whole comparison, so rediscovery would only add a metadata request every 5 minutes. |

Network buffers are left at their defaults throughout.

### Fault tolerance — matched, not weakened

| Key | Value | Why |
|---|---|---|
| `execution.checkpointing.interval` | `5s` | Matched to the Spate arm's 5s offset-commit interval: the same at-least-once guarantee at the same cadence. **Checkpointing is off entirely unless an interval is set**, so a missing line here would silently remove Flink's fault tolerance and flatter its number. |
| `execution.checkpointing.mode` | `AT_LEAST_ONCE` | The default is `EXACTLY_ONCE`. This is the one place where reading the default as "the safe choice" would have handicapped Flink: exactly-once means aligned barriers and the buffer blocking that comes with them, for a guarantee no other arm in the comparison provides. |
| `execution.checkpointing.storage` | `filesystem` | See below. |
| `execution.checkpointing.dir` | `file:///opt/flink/checkpoints` | See below. |
| `state.backend.type` | `hashmap` (default, stated) | The only state is Kafka offsets plus the sink's buffered entries. A disk-backed backend would add compaction and native memory for no benefit — and it is why managed memory can be zero. |

Nothing about Flink's durability is turned off. Restart strategy is left at
Flink's default for a checkpointed job (`exponential-delay`), so a transient
ClickHouse or broker error self-heals rather than ending the run.

#### Checkpoint storage — a real constraint, not boilerplate

The ClickHouse sink extends `AsyncSinkBase`, and `AsyncSinkWriter.snapshotState`
returns **the buffered request entries**. `SinkWriter.flush(false)` runs
pre-barrier and only drains *in-flight* requests, not the buffer — so at every
5s checkpoint, whatever is buffered is serialized into checkpoint state through
the connector's `TypeTags` encoder, one tagged value per column per row.

Consequences:

- Checkpoint state is proportional to `SINK_MAX_BUFFERED_ROWS × parallelism`, not
  to zero. The default `jobmanager` checkpoint storage caps state at 5 MiB and
  would begin *failing* checkpoints under load, so storage has to be a filesystem
  path.
- The run recipe mounts one Docker volume at that path in both containers, which
  makes the shared-filesystem assumption true and recovery genuine. Running with
  two separate local directories would still pay the full write cost — the
  measured work is identical — but recovery would not actually work, so it is not
  what we run.

This is inherent to the connector's design at any buffer size, and it is the
main reason the sink batch settings below are moderate rather than huge.

### Sink batch shape (env, all overridable)

| Env | Default | Why |
|---|---|---|
| `SINK_MAX_BATCH_ROWS` | `25000` | ≈2.2 MiB per INSERT at the measured ~87 bytes/row. The connector's own default is **500 rows**, which for a 100k-message drain would mean thousands of tiny MergeTree parts; 25k is inside ClickHouse's recommended 10k–100k rows per insert. |
| `SINK_MAX_BUFFERED_ROWS` | `50000` | Must be *strictly* greater than the batch size — `AsyncSinkWriter` enforces it, with a message that names neither knob, so `ComparisonJob` checks it up front with a message that does. Also the bound on checkpoint state and on the worst-case retained payload memory (~50k × 8 subtasks). |
| `SINK_MAX_BATCH_BYTES` | `16 MiB` | Well above 25k rows' worth, so rows are the binding limit and batch size is predictable. |
| `SINK_MAX_ROW_BYTES` | `1 MiB` | Connector default. Must be ≤ batch bytes. |
| `SINK_LINGER_MS` | `1000` | Bounds latency in sustained mode. In drain mode batches fill on size long before the timer. |
| `SINK_MAX_IN_FLIGHT` | `2` | The connector's default is **50**, which at parallelism 8 would mean up to 400 concurrent INSERTs against a 6-CPU ClickHouse and a corresponding flood of parts. 2 per subtask (16 total) keeps the pipe full while the next batch builds. |

Server settings sent with every insert: `async_insert=0`, matching the Spate
arm. It is also ClickHouse's default; it is set explicitly so a future ClickHouse
release cannot change it under one arm and not the other.

Not sent: `insert_deduplication_token`. The shared DDL sets
`non_replicated_deduplication_window = 1000` for every arm, and today only Spate
sends tokens. Any framework could; this arm does not, and the duplicate count is
reported rather than suppressed.

### Logging

[`log4j-console.properties`](log4j-console.properties) keeps the root logger at
`INFO` (start-up, checkpointing and failure diagnostics survive) and turns
`org.apache.flink.connector.clickhouse`, `com.clickhouse`, `org.apache.kafka` and
`org.apache.avro` down to `WARN`. Reasons, in order of size:

- The Kafka consumer logs its full resolved configuration per subtask at `INFO` —
  ~800 lines before the first record at parallelism 8.
- The ClickHouse writer logs three `INFO` lines per submitted batch, on the
  sink's completion path.

`CH_LOG_LEVEL=INFO` puts the ClickHouse lines back for debugging. This is
configuration, not a code change, and rule 2 of the contract asks for no debug
logging on the hot path.

### GC

G1, the Java 17 / Flink 2.x default, for the headline number. Swapping in ZGC or
Shenandoah would be tuning past what the framework ships. `-Xlog:gc*` writes
`/opt/flink/log/gc.log` (TaskManager) and `gc-jm.log` (JobManager) for the
driver to read. Adaptive Scheduler and ForSt are **not** enabled.

## Deviations from the contract

1. **`STARTING_OFFSETS` defaults to `earliest`, not "committed else earliest".**
   The Spate arm uses a stable group id with `auto.offset.reset=earliest`, so its
   second run resumes. `earliest` replays the same corpus every run, which a drain
   measurement needs, and it is also `flink-connector-kafka`'s own builder
   default. `STARTING_OFFSETS=committed` restores Spate's exact semantics. This
   changes only which records are read, never the work done per record.

2. **`GenericRecord` string fields are converted to `java.lang.String` in the
   flatMap.** Avro's `GenericDatumReader` produces `org.apache.avro.util.Utf8`.
   The conversion is not avoidable overhead: the ClickHouse connector's
   `DataWriter` would stringify a `Utf8` with `String.valueOf` anyway, *and* its
   checkpointed payload map accepts only a fixed set of value types — a `Utf8` in
   there fails at checkpoint serialization rather than at write time. Recorded
   because it is a per-field allocation the shared schema could have avoided with
   an `avro.java.string` annotation, and we deliberately did not edit the shared
   schema to get it.

3. **ASCII uppercase is hand-written (`Rows.asciiUpper`).** `toUpperCase()` is
   locale-dependent and `toUpperCase(Locale.ROOT)` is still Unicode-aware — it
   maps `ß` to `SS` and `ı` to `I` — so neither matches the other arms'
   `to_ascii_uppercase`. Only `a-z` is folded. On this corpus (`metric_N`) all
   three agree; the difference is latent, and the contract calls it out for
   exactly that reason.

4. **`OffsetResetStrategy` is used through a deprecated kafka-clients type.**
   `OffsetsInitializer.committedOffsets` in connector `5.0.0-2.2` takes only that
   type, and kafka-clients 4.x has deprecated it in favour of
   `AutoOffsetResetStrategy`. There is no non-deprecated way to express "resume,
   else earliest" through the connector's public API. Suppressed, not avoided.

5. **The job jar duplicates ~1490 classes that `flink-dist` also ships** (all in
   `commons-io`, `commons-compress`, `commons-lang3`, `lz4`, `snappy`; they arrive
   transitively via Avro and the Kafka connector). Versions match `flink-dist`'s,
   they load child-first, and no `org.apache.flink` class is duplicated. Left in
   place: excluding them saves ~5 MB of image and adds a `NoClassDefFoundError`
   failure mode for no throughput effect.

Nothing else in the pipeline deviates. The flatten, the tier-B filter order, the
derived columns, the null-`region` coalesce, the `LowCardinality`/`DateTime64`
column types and the column order all follow
[`../common/clickhouse/ddl.sql`](../common/clickhouse/ddl.sql) exactly, verified
end to end against the live server.

### A verified trap worth recording

`DataWriter.writeDateTime64` accepts only `LocalDateTime` or `ZonedDateTime` and
serializes with a hardcoded `ZoneId.of("UTC")`; a bare `Long` in the payload map
throws, and a `LocalDateTime` built in any other zone lands offset. This is the
same class of bug the shared DDL warns about for the Spate `Native` encoder
("or every value silently lands in 1970"). `SensorBatchSchema.fromEpochMillis` /
`fromEpochMicros` build UTC `LocalDateTime`s, and a standalone probe against the
live ClickHouse confirmed exact round-trip of `DateTime64(3)` and
`DateTime64(6)`, `Nullable(Float64)` nulls, `LowCardinality(String)` and
`Array(LowCardinality(String))` before any Flink code existed.

## Verified, 2026-07-25

Run against the live bench infrastructure (Redpanda `v26.1.13`, ClickHouse
`26.3`) inside the documented caps. **These are correctness results, not
performance results** — another arm was measuring on the same Redpanda and
ClickHouse throughout, so no throughput figure from this session is publishable.

What was proven:

- **The job runs as one operator chain.** The JobManager REST API reports exactly
  one vertex at parallelism 8:
  `Source: kafka-sensor-batches -> flatten-tier-a -> clickhouse-sensor_events: Writer`.
  Nothing is serialized between the source, the flatMap and the sink writer.
- **One Kafka partition per subtask**, all 8 assigned, and the source drained to
  `pendingRecords = 0`.
- **A full drain completed**: 118 064 040 rows inserted, `numRecordsSendErrors = 0`
  on every subtask. Attributed server-side in `system.query_log` to
  `Flink-ClickHouse-Sink/0.2.0 (fv:flink/2.2.1, lv:scala/2.12) clickhouse-java-v2/0.9.5 (Linux; jvm:17.0.19) Apache-HttpClient/5.4.4`
  — 5051 INSERTs, ~23.4k rows each, consistent with `SINK_MAX_BATCH_ROWS=25000`.
- **The resolved configuration is the one in `config.yaml`.** TaskManager JVM came
  up with `-Xmx1953077651` (1862 MiB), `-XX:MaxDirectMemorySize=515270249`
  (491 MiB), `-XX:MaxMetaspaceSize=268435456`, matching the arithmetic in the
  memory table above. `taskmanager.memory.managed.fraction 0.0`,
  `parallelism.default 8`, `taskmanager.numberOfTaskSlots 8`,
  `pipeline.object-reuse true`, `pipeline.operator-chaining.enabled true`,
  `execution.buffer-timeout.enabled false`,
  `execution.checkpointing.interval 5s`, `execution.checkpointing.mode AT_LEAST_ONCE`,
  `state.backend.type hashmap` all loaded. Checkpoints completed every 5s.
- **The JVM sees the cgroup cap**: `gc.log` opens with `Using G1` and
  `CPUs: 18 total, 3 available`.

Correctness was gated against the generator in closed form, in ClickHouse, on a
dedicated table (the shared `sensor_events` was being `TRUNCATE`d by the other
driver mid-run — five times inside a 90-second window — which is worth knowing
before anyone tries to verify an arm against a table a driver owns).

Tier A, 20 541 820 rows, **zero** mismatches on every column:

| Check | Result |
|---|---|
| duplicate `(batch_id, event_seq)` | 0 |
| `sensor`, `name`, `unit`, `value`, `tags` vs generator | 0 bad |
| `region` — including the null → `''` coalesce | 0 bad |
| `quality` — both nullity *and* value | 0 bad |
| `batch_ts` vs `BASE_TS_MS + batch_id` | 0 bad |
| `send_ts` vs `BASE_TS_MS*1000 + batch_id`, over the 2 000 000 prefilled rows | 0 bad, all 2 000 000 identities present |
| `send_ts` in 1970 | **0** |

`min(send_ts) = 2026-02-25 06:13:20.000000`, exactly `BASE_TS_MS`, at microsecond
precision. (Note the base topic now also holds live-produced batches with a 1e9
`batch_id` origin whose `send_ts` is a real clock time; the closed-form `send_ts`
check applies to the prefilled range and passes completely there.)

Tier B, 22 113 700 rows, again zero mismatches — and checked in **both**
directions, which matters more than it sounds:

| Check | Result |
|---|---|
| `unit = 'drop'` rows leaked through | 0 (7 distinct units remain of 8) |
| `quality < 0.2` rows leaked through | 0 |
| `name_upper` vs `METRIC_n` | 0 bad |
| `value_scaled` vs `intDiv(value * 1000, event_seq + 1)` | 0 bad |
| `region`, `value`, `tags`, `batch_ts` | 0 bad |
| duplicates | 0 |
| **expected-kept identities missing** | **0** |
| **rows present that should have been dropped** | **0** |

The last two lines are a set difference against the generator over the prefilled
corpus: 1 470 000 rows expected to survive both filters, 1 470 000 present, 0
missing, 0 extra. "No bad rows leaked" alone would not have caught a filter that
was too aggressive.

### One connector observation from the metrics, not fully diagnosed

The sink's own `numRecordsSend` reads exactly **2×** `numRecordsIn` on every
subtask (29 516 080 against 14 758 040), while `numRequestSubmitted` and
`system.query_log`'s `written_rows` both agree with one insert per batch and
`uniqExact` shows zero duplicates. So no row is inserted twice; the counter is
incremented inside the client's request-body callback, which appears to run twice
per HTTP request. Since the row bytes are already serialized into
`ClickHousePayload.getCachedBytes()` by then, the cost is a duplicated copy of the
batch into the output stream rather than duplicated encoding — but it does mean
**the connector's own `numRecordsSend`/`numBytesSend` metrics over-report by 2×**
and should not be used as a throughput source. This comparison does not read
framework metrics for any published figure, so it changes nothing here; it is
recorded because anyone else reading those counters would be misled.

## Findings about Flink's shipped defaults

Rule 2 of the contract requires that a measurably suboptimal shipped default be
published **as avoidable**, with the cost quantified where a secondary arm can do
it. Three, in descending order of size.

### 1. `AvroDeserializationSchema` allocates a record per message — avoidable in ~20 lines

`RegistryAvroDeserializationSchema.deserialize` re-uses the `BinaryDecoder` and
the input stream, but then calls:

```java
return datumReader.read(null, getDecoder());
```

`read(null, …)` means a fresh `GenericData.Record` per message and, through it, a
fresh `Utf8` plus backing `byte[]` for **each** string field. For this corpus —
20 events, 4 top-level fields, ~44 strings per message — that is on the order of
150 allocations per message that Avro's own reuse contract is designed to avoid.
It also calls `setSchema`/`setExpected` on every message, which invalidates the
datum reader's resolver and costs two identity-map lookups per message.

Both are avoidable by passing the previous record back as the reuse argument and
only re-pointing the reader when the writer schema identity changes.
`ReusingAvroDeserializationSchema` is those ~20 lines, kept deliberately
identical to Flink's implementation in every other respect — same
`ConfluentSchemaRegistryCoder`, same `MutableByteArrayInputStream`, same single
reused `BinaryDecoder` — so the delta is attributable to record reuse alone.

**It is a secondary, labelled arm (`DESER=reusing`) and never the headline.**
Rule 1 forbids hand-writing a competitor's internals for the published number,
because at that point we would be measuring our Java rather than Flink.

Reviewer caveat: reuse is only safe because the row is fully copied out inside
the flatMap before the next message is decoded, which holds because the job is
one chain. It would be wrong in a job that buffered `GenericRecord`s.

### 2. The ClickHouse connector allocates an `Object[]` per column per row for a disabled log call

`com.clickhouse.utils.Serialize.writeValuePreamble` — called once per column per
row — opens with:

```java
LOG.debug("writeValuePreamble[isNullable={}, dataType={}, column={}, value={}]", ...);
```

Four arguments means slf4j's varargs overload, so javac allocates an `Object[]`
at the call site **before** the level check, plus boxing for the primitives. That
is 11–12 arrays and their boxes per row, unconditionally, at any log level.

**This one cannot be configured away.** Setting the logger to `OFF` does not
remove an allocation the caller already made. It is inside the connector, so
rule 1 keeps us out of it, and unlike finding 1 there is no secondary arm that
can isolate it without rewriting the sink.

### 3. The connector's batch defaults are two orders of magnitude off for ClickHouse

`maxBatchSize = 500` rows and `maxInFlightRequests = 50`. At parallelism 8 the
shipped defaults would send up to 400 concurrent INSERTs of 500 rows each. This
one *is* configuration and we fixed it (25 000 / 2), so it costs the published
number nothing — but a user who deploys the connector as shipped will hit it, and
it is worth saying so.

Related, and also disclosed rather than fixed: the connector retains both the
`Map<String,Object>` payload *and* the serialized `byte[]` for every buffered
row, so buffered memory per row is roughly an order of magnitude larger than the
~87 bytes of wire data it represents. That is what bounds
`SINK_MAX_BUFFERED_ROWS`.

### A correction to a claim we started from

The brief warned that "`GenericRecord` has no Flink `TypeInformation`, so any
non-chained boundary falls back to Kryo". That is **not** what happens here:
`AvroDeserializationSchema.getProducedType()` returns
`GenericRecordAvroTypeInfo`, and `setValueOnlyDeserializer` propagates it, so a
chain break would cost an *Avro* serializer round trip, not Kryo. The conclusion
is unchanged — stay in one chain — but the reason is weaker than stated, and the
`GenericRecordAvroTypeInfo` workaround the brief suggests is already in effect.

## Where we may be unfair to Flink

Stated plainly, because a comparison page that only lists a competitor's
weaknesses is read as marketing.

1. **8 subtasks on 3 CPUs.** Matching parallelism to the topic's partitions is
   the standard advice, but on a 3-CPU TaskManager it means 2.7:1
   oversubscription plus 8 copies of every per-subtask buffer inside a 1.86 GiB
   heap. Parallelism 4 may well be faster here. Both are one config line; if the
   sweep shows 4 winning, 4 is what should be published, with 8 shown alongside.

2. **The JobManager tax is real and inherent.** A quarter of the CPU and a
   quarter of the memory go to a control plane that processes no records. The
   contract already requires the TaskManager-only figure to be recorded
   alongside, and it should be, because the single-process arms have no
   equivalent cost.

3. **A JVM on Docker Desktop for macOS is not a JVM on Linux.** The host caveat
   in the parent README applies with extra force to a garbage-collected runtime:
   G1's heuristics react to a vCPU count the hypervisor maps non-deterministically
   across 6 performance and 12 efficiency cores. Flink is the arm most likely to
   improve on bare metal.

4. **JIT warm-up is inside the measured window unless the driver excludes it.**
   The first seconds of any Flink run are interpreted and profiling. A drain that
   finishes in tens of seconds attributes a real share of its time to warm-up
   that a steady-state deployment would not pay. Either the measured window
   should start after a warm-up period, or the run should be long enough for
   warm-up to be a rounding error — and whichever is chosen should be stated.

5. **Format asymmetry.** This arm cannot choose its wire format:
   `RowBinaryWithNamesAndTypes` is forced by the connector's typed mode, and it is
   row-oriented. Comparing it to Spate's `native` (columnar, pre-pivoted) number
   measures the format as much as the framework. The Spate `rowbinary` control
   is the fair row in the table.

6. **The connector is young.** `flink-connector-clickhouse` 0.2.0 is a 0.x
   release from May 2026, and findings 2 and 3 above read like a connector that
   has not had a throughput pass yet. A conclusion of the form "Flink is slower"
   would be wrong; "Flink plus ClickHouse's current official connector is slower"
   is what the data can support, and the page should say the second thing.

## Why was throughput X and not 2X?

<!-- TODO(driver): one sentence, from the measured configuration, naming the
     binding constraint and the evidence for it. Populate after the measurement
     pass; do not guess it from the design.

     The candidates this design already knows about, so the measurement can
     discriminate between them rather than starting from scratch:
       * ClickHouse-bound   — server-side row->column pivot for
                              RowBinaryWithNamesAndTypes; cross-check against the
                              ceiling pass and system.query_log ProfileEvents.
       * allocation-bound   — the per-message record allocation (finding 1) plus
                              the connector's per-value Object[] (finding 2);
                              evidence is gc.log allocation rate and the
                              DESER=reusing delta.
       * CPU-oversubscribed — 8 subtasks on 3 CPUs (unfairness 1); evidence is a
                              parallelism=4 run at the same caps.
       * checkpoint-bound   — 5s serialization of buffered sink entries;
                              evidence is checkpoint duration and size in the
                              JobManager REST API against SINK_MAX_BUFFERED_ROWS.
-->

*Not yet measured — see the note in the source of this file for the candidate
constraints and the evidence that would separate them.*

**No throughput number should be taken from the 2026-07-25 session.** Another arm
was measuring on the same Redpanda and ClickHouse for its entire duration
(996 million rows inserted by a `clickhouse-rs` client into the same table), so
every rate observed here is contended. The measurement pass needs the
infrastructure to itself.
