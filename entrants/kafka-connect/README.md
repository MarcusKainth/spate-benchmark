# The Kafka Connect arm

Kafka → Confluent-framed Avro → `clickhouse-kafka-connect` → ClickHouse
materialized view → `sensor_events`, held to
[the fairness contract](../../methodology/), which is normative. Read that
first; this file records only what is specific to Kafka Connect.

Delivery is **at-least-once** (`exactlyOnce=false`, the connector's
`AtLeastOnceBufferStrategy`), with worker offset flushes matched to the 5 s
durability cadence every arm runs. The insert format is **RowBinary**,
uncompressed, over HTTP — verified in the v1.4.0 source
(`ClickHouseWriter.java:1066`; `RowBinaryWithDefaults` is chosen only when the
target has `DEFAULT` columns, and the landing table has none).

**The structural deviation, first.** Connect has no fan-out operator: one Kafka
record cannot become ~100 rows inside the runtime. So the connector lands the
*nested* batch into a `Null`-engine landing table and a ClickHouse materialized
view performs the flatten, both filters and both derived columns
([`clickhouse/arm.sql`](clickhouse/arm.sql), applied per repetition by the
harness DDL hook). That is a legitimate, widely-deployed real-world pattern —
and it moves the transform's CPU into the shared ClickHouse, where the cgroup
sampler cannot see it. This arm's efficiency comparison therefore leans on
ClickHouse's own `ProfileEvents` via `system.query_log` (the MV's cost rides on
the parent insert; background merges are excluded — and the landing table,
being `Null`, causes none). Declared in `[[deviations]]` in
[`entrant.toml`](entrant.toml); the site renders it beside the numbers.

## Configuration

Everything tunable lives in the two properties templates, rendered by
[`entrypoint.sh`](entrypoint.sh) from the container environment at start-up.
There is no Java in this arm at all: the pipeline is configuration plus the
materialized view's SQL, which is exactly what makes it worth measuring.

### Knobs the driver sets per run

| Knob | Value | Reaches Connect as | What it controls |
|---|---|---|---|
| `tasks` | **8** | `tasks.max` | One task per **partition** (8). A ninth task would own no partitions; fewer leaves a task owning two partitions and pacing the drain, the same arithmetic as Flink's parallelism. |
| `buffer_count` | **2000** | `bufferCount` | Records (messages) per buffered insert. At 100 events/message that is ~200,000 landed events and ~147,000 surviving rows per insert after the MV's filters — the same order as the other arms' batch sizes, inside ClickHouse's recommended 10k–100k+ band. The connector's own default is 10000 messages, which at this corpus's fan-out is a 1M-event insert. |
| `buffer_flush_ms` | **1000** | `bufferFlushTime` | Bounds sustained-mode latency, matching Flink's 1000 ms linger. **Must be > 0 whenever `bufferCount` > 0**: the buffer flushes on size *or* time, so a zero flush time strands a sub-`bufferCount` tail and a drain never completes. Not expressible as a `[[constraints]]` knob-exceeds-knob relation; enforced by comment and by the smoke recipe below. |

These are **per task**, so cross-arm quantities are products: up to
`tasks × buffer_count` = 16,000 messages (~1.6M events) buffered across the arm.

### Values fixed in the templates

| Key | Value | Why |
|---|---|---|
| `exactlyOnce` | `false` | At-least-once, matched guarantee-for-guarantee. `true` adds state-store round-trips no other arm pays for a guarantee no other arm offers. |
| `errors.tolerance` | `none` (default kept) | A poison record fails the task loudly. `all` drops silently, and a silent drop voids the loss gate — faster for the wrong reason. |
| `offset.flush.interval.ms` | `5000` | The matched durability cadence (shipped: 60000). |
| `consumer.max.poll.records` | `2000` | One poll fills one buffered insert. Shipped 500 puts four polls under every flush. |
| `consumer.max.partition.fetch.bytes` | `8388608` | Shipped 1 MiB is ~200 messages/partition/fetch at ~5 KiB messages, starving a 2000-record buffer. `fetch.max.bytes` stays at the shipped 50 MiB. |
| `ignorePartitionsWhenBatching` | `false` (default kept) | Per-partition batching is what keeps the connector's derived dedup token coherent. |
| `client_version` | unset (effective `V1`) | RowBinary either way; the shipped default is measured. |
| `clickhouseSettings` | unset | The connector already pins `async_insert=0, wait_end_of_query=1` on every insert (`ClickHouseSinkConfig.java:232-236`), which is exactly what server-side attribution requires. |
| JVM sizing | `-Xms20480m -Xmx20480m -XX:MaxDirectMemorySize=768m -XX:MaxMetaspaceSize=256m` | 21504m in the 24 GiB container — the same figure and limit/8 slack rule as the Flink TaskManager, enforced by `entrants_are_valid`. GC is the launcher's shipped G1 (rule 1). |

`GROUP_ID` is the **connector name**, not a consumer `group.id`: Connect derives
a sink's consumer group as `connect-<name>` and refuses `group.id` overrides for
sinks, so the fresh consumer group each repetition needs (a drain replays from
offset zero) arrives by naming the connector with the driver's fresh id.

[`log4j2.yaml`](log4j2.yaml) is root `WARN`, console only — Kafka 4.x
configures log4j2 in YAML, and the shipped Connect config is root `INFO` with a
rolling *file* appender: disk writes on the hot path, inside the measured
cgroup, for output nothing reads.

GC is G1 (the launcher's shipped `KAFKA_JVM_PERFORMANCE_OPTS`). `-Xlog:gc*`
writes `/opt/kafka/logs/gc.log` for the driver to read — set via `KAFKA_OPTS`,
not `KAFKA_GC_LOG_OPTS`, because `kafka-run-class.sh` only assembles its own GC
logging in `-daemon` mode and this container runs foreground.

## Build

```sh
bench build kafka-connect
```

By hand — the build context is the **repository root**, uniformly for every
entrant:

```sh
docker build -f entrants/kafka-connect/Dockerfile -t spate-bench-kafka-connect .
```

The build fetches the connector's GitHub release zip and fails unless its
sha256 matches the pinned `CKC_SHA256` — a release asset can be replaced under
the same tag, and unverified bytes do not ship. The Avro converter's resolved
dependency graph is recorded at `/opt/connect/dependencies.txt` (Maven has no
lockfile; the graph is the only provenance a re-run can be compared against).

## Run

```sh
bench run kafka-connect --reps 3
```

By hand, which is what a reviewer runs to look inside the container. One
container, the full 6 CPU / 24 GiB data-plane envelope; standalone Connect has
no control plane by design:

```sh
docker run -d --name spate-bench-kafka-connect --network spate-bench-net \
  --cpus 6 --memory 24g --memory-swap 24g \
  spate-bench-kafka-connect
```

`--memory-swap` equals `--memory` so memory pressure surfaces instead of hiding
in a swapfile. The worker never terminates itself; the driver removes the
container. **That recipe runs the image's defaults**, which are kept equal to
the published knobs; the driver additionally sets the variables `[env]` names.

The rendered configuration in force is readable out of the running container:

```sh
docker exec spate-bench-kafka-connect cat /opt/kafka/connect-data/worker.properties \
  /opt/kafka/connect-data/clickhouse-sink.properties
```

Smoke-checking a drain by hand: watch `SELECT count() FROM sensor_events` reach
the expected row count. If it stalls a few thousand rows short with the
consumer group at zero lag, the first suspect is `bufferFlushTime=0` stranding
the tail — see the knob table.

## Versions

| Component | Coordinate / image | Version |
|---|---|---|
| Connect runtime | `apache/kafka:4.3.1` (ASF's own image) | 4.3.1 |
| JVM | Temurin JRE (image default) | 21 |
| ClickHouse sink | `clickhouse-kafka-connect-v1.4.0.zip` (GitHub release, sha256-pinned) | v1.4.0 |
| Avro converter | `io.confluent:kafka-connect-avro-converter` | 8.3.0 |
| Registry client | `io.confluent:kafka-schema-registry-client` (transitive) | 8.3.0 |
| Avro | `org.apache.avro:avro` (transitive) | per `dependencies.txt` |

`[version]` in the descriptor resolves `4.3.1-v1.4.0` from the jar filenames
that actually run (the Connect runtime jar and the plugin jar); the Dockerfile
fails the build if either filename pattern drifts. Joined with `-` rather than
`+` because the driver's version parser accepts only `[0-9A-Za-z.-]` tokens.

## Licence analysis

Everything in the container is Apache-2.0, which is what closed the
`[planned].licence_gate` this arm used to carry:

- `apache/kafka:4.3.1` is the ASF's own image — **not** a Confluent Platform
  image, which is what the Confluent Community Licence covers.
- `clickhouse-kafka-connect` is Apache-2.0 (LICENSE in the release zip).
- `kafka-connect-avro-converter` 8.3.0 and its runtime closure are Apache-2.0
  **at the artifact level**, verified against the POMs on
  `packages.confluent.io/maven/`. The CCL covers the Schema Registry *server*,
  which is not present (the registry here is Redpanda's Confluent-API
  implementation). [Aiven's licence analysis](https://aiven.io/blog/aiven-statement-on-kafka-license)
  reaches the same reading.
- Apicurio's Apache-2.0 converter was evaluated and rejected on function, not
  licence: its Confluent compatibility is server-side only
  ([adr/0001](https://github.com/Apicurio/apicurio-registry/blob/main/adr/0001-confluent-schema-registry-compatibility.md)),
  so its deserializer cannot resolve schemas from a Confluent-API registry.

The converter's non-Central origin is declared in `[[deviations]]`.

## Differences worth knowing

- **The transform runs in ClickHouse, not in Connect.** The arm's client-side
  CPU (decode + RowBinary re-encode) and the other arms' client-side CPU
  (decode + transform + encode) are not the same work. Read this arm's numbers
  with the server-side CPU column, which is where its flatten/filter/derive
  cost lands, and which excludes background merges — though the `Null`-engine
  landing table produces no parts and therefore no merges of its own.
- **The upstream integration test for array-of-Struct → `Array(Tuple)` is
  `@Disabled`** (for an unrelated flag) at v1.4.0, so the nested write path
  this arm depends on is not exercised by the connector's own CI. First local
  smoke of a live drain is the verification, not the upstream suite.
- **The MV-attribution belief in `harness/src/serverside.rs` is what this arm
  verifies.** An insert into the landing table names the view's target in
  `tables` as well, so attribution by `hasAny` catches it; the landing table is
  also declared in `[clickhouse].attribution_tables` as the module recommends.
- **The connector derives `insert_deduplication_token`
  (`topic-partition-minOffset-maxOffset`) on every schema-path insert**, even
  with `exactlyOnce=false`. Inert here — the landing table is `ENGINE = Null`,
  no parts, no dedup window — but it is the same mechanism `ddl.sql` discloses
  for the Spate arm, so it is declared rather than discovered.
- **Connect ≥ 2.7 compatibility statement**: the connector declares Kafka
  Connect 2.7+ compatibility; Kafka 4.3's Connect API is well inside that
  range, but a base-image major bump should re-check it.

## Gregg's question (candidate)

Why X and not 2X: the leading candidate is per-record materialization on the
client — the converter builds a `GenericRecord`, converts it to a Connect
`Struct` (a second full copy with per-field boxing), and the connector then
walks that `Struct` against the target's `DESCRIBE` to serialize RowBinary — three
traversals of every batch where Spate does one. To be confirmed against the GC
log and the server-side CPU split on the reference environment.
