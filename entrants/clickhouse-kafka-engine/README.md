# The ClickHouse Kafka table engine arm

Kafka → Confluent-framed Avro → materialized view → the shared ClickHouse,
held to [the fairness contract](../../methodology/), which is normative. Read
that first; this file records only what is specific to this arm.

The arm is a dedicated **ingest tier**: one ClickHouse container gets the
whole 6 CPU / 24 GiB data-plane envelope and runs a Kafka engine table
(consume + AvroConfluent decode), a materialized view (flatten + filter +
derive), and a Distributed table that forwards the finished rows —
**synchronously** — to the shared infra ClickHouse that owns `sensor_events`
for every arm. It is *not* a zero-hop baseline: the container does no
MergeTree storage of its own, pays one network hop to storage like every arm,
and is published as "ClickHouse as its own ETL tier". The alternative — local
storage inside the 6-CPU envelope, paying merges the 16-CPU shared server
gives every other arm for free — would have been an unfair handicap dressed
as a stronger claim.

Delivery is **at-least-once**: the engine commits offsets once per flushed
block, *after* the block has been written through the materialized view, and
`distributed_foreground_insert = 1` makes that write block until the shared
server has acked — so an offset commit implies the rows exist remotely. A
crash between the remote ack and the commit replays the block, which surfaces
as the duplicate metric, never as loss. The insert format is **Native**
(lz4-compressed columnar blocks over the interserver TCP link) — with the
declared deviation that this is the Distributed engine's internal transfer,
not a client insert API, so read it against Spate's `native` rather than
Flink's `rowbinary_nt`.

There is **no custom code in this arm at all**: three static SQL objects
([`initdb/10_ddl.sql`](initdb/10_ddl.sql)), declarative XML
([`config.d/`](config.d/), [`users.d/`](users.d/)), and an assert script
([`initdb/20_assert.sh`](initdb/20_assert.sh)) that reads the configuration
back and **refuses the container start** on any mismatch.

## Configuration

Every tunable reaches the server as an environment variable, carried by
`from_env` into the `kafka_src` named collection or the default profile. The
DDL is static — `ENGINE = Kafka(kafka_src)` — so what a reviewer reads in the
SQL is exactly what runs, and what the driver set is exactly what
`20_assert.sh` verified.

### Knobs the driver sets per run

| Knob | Value | Engine default | What it controls |
|---|---|---|---|
| `num_consumers` | **8** | 1 | `kafka_num_consumers`: one consumer per **partition** (8). Fewer leaves the slowest consumer owning two partitions — 6 measures like 4. Oversubscribed on the 6-CPU envelope exactly as Spate `threads = 8` and Flink `parallelism = 8` are. The CREATE-time cap against detected cores (6) is lifted by `kafka_disable_num_consumers_limit`; see the post-start check below. |
| `block_msgs` | **16384** | 1,048,576 | `kafka_max_block_size`, in **messages**, not rows: 16384 messages ≈ 1.2M surviving rows per forwarded INSERT. The shipped default is `max_insert_block_size` = 1,048,576 *messages* — never reached here, so under the default the flush timer always binds and block size stops being a knob. Sweep candidates: 8192 / 16384 / 32768. |
| `flush_ms` | **5000** | 7500 | `kafka_flush_interval_ms` — **the commit cadence**: offsets commit once per flushed block. The engine's own default (7500, from `stream_flush_interval_ms`) would be a *laxer* durability interval than every other arm's 5 s, so 5000 is matched, not tuned. |
| `poll_timeout_ms` | **500** | 500 | `kafka_poll_timeout_ms`, the stream thread's poll bound. Declared as a knob (at its default) so the `flush_ms > poll_timeout_ms` constraint in `entrant.toml` is checkable against stated values rather than an image default no record reports. |

### Values fixed in `config.d/` and `users.d/`

| Setting | Value | Why |
|---|---|---|
| `kafka_thread_per_consumer` | **1** | THE trap ([#35153](https://github.com/ClickHouse/ClickHouse/issues/35153)): the shipped default 0 squashes all consumers into ONE flush thread — 8 consumers measure like 1. Not a knob: no correct configuration has another value. |
| `kafka_skip_broken_messages` | **0** | One skipped message silently drops 100 rows; the loss gate then voids the arm. |
| `distributed_foreground_insert` | **1** | The guarantee-bearing setting. Default 0 spools the MV's insert to local disk and acks early, so offsets would commit before the shared server had the rows. Renamed from `insert_distributed_sync` in 23.11; the old name is an alias. |
| `kafka_commit_every_batch` | 0 (shipped) | One offset commit per flushed block, not per librdkafka batch — this is what makes `flush_ms` the durability cadence. |
| `materialized_views_ignore_errors` | 0 (pinned) | 1 would turn a refused remote insert into a dropped block plus a log line; the loss gate must see stall-and-replay, never skip. |
| `background_message_broker_schedule_pool_size` | 16 (shipped, recorded) | The pool the streaming jobs run in; with `thread_per_consumer = 1` the arm needs ≥ 8. Recorded so a default change cannot move it silently. |
| `queued_max_messages_kbytes` | 262144 | librdkafka prefetch, 64 MiB → 256 MiB per consumer: the default sits at the edge of one 16384-msg × ~5 KiB block. |
| `async_insert` | untouched | Not applicable: the Distributed engine forwards materialized blocks; there is no client-side small-insert stream to coalesce. |

Server sizing for a node that stores nothing (shrunk caches, 2-thread merge
pool with the matching `merge_tree` free-entries floors, system logs removed
**except `query_log`** — the reviewer's window, ~2 rows/s) is justified
setting-by-setting in [`config.d/30-server-sizing.xml`](config.d/30-server-sizing.xml).

## Build

```sh
bench build clickhouse-kafka-engine
```

By hand — the build context is the **repository root**, uniformly for every
entrant:

```sh
docker build -f entrants/clickhouse-kafka-engine/Dockerfile -t spate-bench-ch-kafka .
```

## Run

```sh
bench run clickhouse-kafka-engine --reps 3
```

By hand, which is what a reviewer runs to look inside the container:

```sh
docker run -d --name spate-bench-ch-kafka --network spate-bench-net \
  --cpus 6 --memory 24g --memory-swap 24g \
  spate-bench-ch-kafka
```

`--memory-swap` equals `--memory` so memory pressure surfaces instead of
hiding in a swapfile. **That recipe runs the image's defaults**, which are
kept equal to the published knob values; the driver additionally sets the
nine variables in `entrant.toml [env]`, and those are what a published
record's knobs mean. The image's official entrypoint runs the initdb DDL and
the assert script against a localhost-only init server, then starts the real
one — a container that comes up at all has already proven its configuration.

**Post-start operator check** — the one thing initdb cannot see, because
consumers materialise when streaming starts:

```sh
docker exec spate-bench-ch-kafka clickhouse-client --query \
  "SELECT count() FROM system.kafka_consumers WHERE table = 'sensor_batches_queue'"
```

Must print **8**. Anything less is the silent-clamp failure mode
(consumer-count cap against detected cores) and the run is invalid.

The SQL endpoint accepts no network connections: with no `CLICKHOUSE_PASSWORD`
set, the official entrypoint restricts the `default` user to localhost, so a
reviewer goes through `docker exec` as above.

## Versions

| Component | Coordinate / image | Version |
|---|---|---|
| ClickHouse server | `clickhouse/clickhouse-server:26.3` (digest `sha256:85c43481…ea49`) | 26.3.17.4 |
| librdkafka | bundled in the server build | (reported in `system.build_options`) |

Same major as the shared storage server (`environments/*.toml`): one
ClickHouse version in the provenance, and the consumer is not tuned on a
newer codebase than its own storage tier. `[version].pinned` asserts the
version string the image reports, so a base bump refuses the run.

## Gregg's-question candidate

Each consumer's loop is strictly serial: poll → decode → ARRAY JOIN →
**synchronous** remote insert → commit, with no in-flight pipelining — the
remote-ack stall per block is dead time, and the arm is 8 such serial loops
on 6 CPUs. If the number is X and not 2X, this is the first place to look,
and it is the price of the setting that makes the guarantee real.

## Traps, verified

- **[#35153](https://github.com/ClickHouse/ClickHouse/issues/35153)** —
  `kafka_thread_per_consumer = 0` (the default) is one flush thread for all
  consumers. Fixed at 1 above.
- **Consumer-count clamp under cgroups** ([#26642](https://github.com/ClickHouse/ClickHouse/issues/26642),
  [#35926](https://github.com/ClickHouse/ClickHouse/issues/35926),
  [#40670](https://github.com/ClickHouse/ClickHouse/issues/40670)): the CREATE-time
  cap compares against *detected* cores — 6 in this cgroup — and would
  silently clamp 8 consumers. `kafka_disable_num_consumers_limit = 1` lifts
  it; the post-start check above proves all 8 materialised (verified against
  the built image: `system.kafka_consumers` reports 8 with this config).
- **AvroConfluent `array<record>`** decodes as `Array(Tuple(...))` and the
  nested fields are addressed `e.seq`, `e.name`, … after `ARRAY JOIN` — the
  decode path is not a flat-struct fast case, by workload design.
- **MATERIALIZED columns through Distributed**
  ([#4015](https://github.com/ClickHouse/ClickHouse/issues/4015),
  [#9439](https://github.com/ClickHouse/ClickHouse/issues/9439)): the
  Distributed shim declares the 13 physical columns and **no `ingest_ts`**;
  the shared server stamps it when the forwarded insert lands, so latency
  honestly includes the forward hop.
- **Foreground-insert error propagation**: kill the shared ClickHouse and the
  MV insert fails, the block is not committed, and the engine stalls and
  replays — a duplicate-metric event, never a skip. That is the
  `materialized_views_ignore_errors = 0` + `kafka_skip_broken_messages = 0`
  pairing doing its job.
- **A broker that is down does not stop the server** (verified standalone):
  the Kafka table's consumers retry resolution/connection in the background
  and the server serves queries throughout — so a mis-ordered bring-up
  degrades to lag, not to a crash loop.

## Differences worth knowing

- **The data plane is a database server.** It carries a server's overhead
  (scheduler, system tables, an idle SQL endpoint) inside the envelope, and
  does its storage on the shared server outside it — both directions are
  declared in `[[deviations]]` and rendered by the site.
- **The latency floor is the flush cadence.** Rows wait up to `flush_ms` in
  the engine's block buffer before the forwarded insert exists to be
  stamped. Same trade as Spate's `linger_ms = 500`, at 5000 ms.
- **Durability is "at most 5 s"**: blocks sealing on `block_msgs` before the
  timer commit sooner — stricter than the convention, never looser.
- **No `insert_deduplication_token` is sent** (like every arm except Spate);
  the shared table's `non_replicated_deduplication_window = 1000`
  content-hashes this arm's forwarded blocks, and replays are reported as
  duplicates, never suppressed.
