# The Vector arm

Kafka → Confluent-framed Avro → remap fan-out → ClickHouse, held to
[the fairness contract](../../methodology/), which is normative. Read that first;
this file records only what is specific to Vector.

Delivery is **at-least-once**: end-to-end acknowledgements are on, so a source
offset commits (every 5 s) only after ClickHouse has acked every row derived
from the message —
[Vector's own architecture doc](https://vector.dev/docs/architecture/end-to-end-acknowledgements/)
is explicit that acknowledgement state is shared across all events a transform
emits from one input. Two wire formats are published — `arrow_stream` (default)
and `json_each_row` — because they are not the same amount of server-side work;
the `format` option shipped in
[0.57 via vector#24373](https://github.com/vectordotdev/vector/pull/24373),
which quotes JSONEachRow as "~4-5x less efficient" server-side. ArrowStream is
labelled **beta** in the 0.57 docs, so it is also a declared deviation, and the
GA `json-each-row` variant is the control that quantifies what the beta format
buys.

Vector is Rust with the same no-GC story as Spate and may beat it. That is the
reason to run it.

## Topology: 8 sources → 8 remaps → 1 sink

A Vector [kafka source](https://vector.dev/docs/reference/configuration/sources/kafka/)
is one librdkafka consumer whose decoding runs in that source's own task; the
maintainers' scaling guidance is one source per partition
([discussion #15884](https://github.com/vectordotdev/vector/discussions/15884)).
The topic has 8 partitions, so the config declares eight identical sources in
one consumer group under `cooperative-sticky` assignment, which settles at one
partition per consumer. The envelope allows one container, so the eight
consumers share the process. Each source feeds its own
[remap](https://vector.dev/docs/reference/configuration/transforms/remap/)
(a remap is one task; a single shared remap would serialize the fan-out onto one
core), and all eight remaps load the same committed
[`transform.vrl`](transform.vrl). In [`vector.yaml.tmpl`](vector.yaml.tmpl) the
sources `in-1`..`in-7` are YAML aliases of `in-0`, so "identical" is
parser-enforced rather than promised.

## Configuration

Everything tunable lives in [`vector.yaml.tmpl`](vector.yaml.tmpl), reaches the
container as environment variables, and nothing tunable lives anywhere else. The
committed `${...:-default}` values equal the published knobs, so a hand-run
container matches the numbers.

### Knobs the driver sets per run

| Knob | Shipped default | Ours | Why |
|---|---|---|---|
| `threads` | detected core count | **6** | `VECTOR_THREADS`. Vector detects the host's cores, not the cgroup quota, so on the reference host it would start ~32 workers under a 6-CPU cap — pure scheduler churn. Matched to the envelope instead. |
| `batch_events` | ~40k rows effective (the 10 MiB byte bound seals first) | **262144** | Rows per INSERT, equal to Spate's cap so the cross-arm batch quantity is comparable, and inside a fixed 256 MiB byte cap raised so that **events** bind and the declared batch size is the one in force. |
| `batch_timeout_secs` | 1 | **1** | Kept: it is the sustained-mode p99 floor; in drain, batches fill on size first. Sweepable. |
| `request_concurrency` | `adaptive` | **8** | Fixed width over the adaptive (ARC) controller: a drain window is tens of seconds and ARC spends exactly that long probing its way up, so the measurement would be of the controller's warm-up. Eight is the concurrent-INSERT width the other arms hold. |
| `buffer_events` | 500 | **524288** | The shipped 500-event sink buffer cannot feed even one 262144-row batch — the batcher would seal on starvation every time. 2× the batch so the next batch fills while the last drains. |
| `compression` | `gzip` | **`none`** | The default spends the envelope's scarce CPU compressing inserts to save same-host bandwidth the environment has in abundance. |

`buffer_events` must exceed `batch_events` — declared in `[[constraints]]` so a
sweep is refused before a container starts, not discovered as a starved batcher
minutes into a cell.

### Values fixed in the config

| Key | Value | Why |
|---|---|---|
| `commit_interval_ms` | 5000 | The durability cadence every arm pays (Spate's offset commits, Flink's `AT_LEAST_ONCE` checkpoints). Vector's shipped default, set explicitly so it is a statement rather than an inheritance. |
| `acknowledgements.enabled` | `true` | The load-bearing line: connects a ClickHouse ack back to the source's offset commit. Without it the 5 s interval commits offsets for rows the sink may still lose. |
| `query_settings.async_insert_settings.enabled` | `false` | ClickHouse 26.3 defaults `async_insert` on, under which the server acks before writing — an ack the acknowledgement chain would then trust. Matched to every other arm. |
| `buffer.when_full` | `block` | Backpressure, not loss: `drop_newest` breaks at-least-once while flattering throughput. |
| `skip_unknown_fields` | `false` | An unknown field means the transform emitted a column the table lacks — a bug to fail on, not absorb. |
| `date_time_best_effort` | `true` | Lands the epoch-derived `DateTime64(3)`/`(6)` values at full precision. |
| `batch.max_bytes` | 256 MiB | Lifted from the shipped 10 MiB so `batch_events` is what binds (see knob table). |
| 8 sources / 8 remaps | structural | One consumer and one transform task per partition; see Topology. |
| `partition.assignment.strategy` | `cooperative-sticky` | Settles eight consumers on one partition each; incremental rebalance, so a late joiner does not stall the other seven. |
| `fetch.message.max.bytes` | 8 MiB | Above the corpus's largest framed message; the 1 MiB default costs extra round-trips. |
| `queued.max.messages.kbytes` | 262144 (= 256 MiB) | Per-consumer prefetch bound; in drain the consumer must never be the starved side. 8 × 256 MiB is small against 24 GiB. |
| `VECTOR_LOG` | `warn` | No per-batch chatter on the hot path. |
| `api.enabled` | `false` | No published figure comes from an arm's self-report; an idle API server is still a listener on the measured process. |

## Build

```sh
bench build vector
```

By hand — the build context is the **repository root**, uniformly for every
entrant, because the build needs `entrants/vector/` and `workload/schema/`:

```sh
docker build -f entrants/vector/Dockerfile -t spate-bench-vector .
```

Stage 1 bakes the committed `sensor_batch.avsc` into the config (Vector's avro
decoder takes a static inline schema — see Differences), and stage 2 runs
`vector validate --no-environment` against the exact binary that will run it, so
a config the loader rejects or a VRL program that does not compile fails the
build rather than the benchmark run.

## Run

```sh
bench run vector --reps 3
```

By hand, which is what a reviewer runs to look inside the container. One
container, the full 6 CPU / 24 GiB data-plane envelope, no control plane:

```sh
docker run -d --name spate-bench-vector-sut --network spate-bench-net \
  --cpus 6 --memory 24g --memory-swap 24g \
  spate-bench-vector
```

`--memory-swap` equals `--memory`, so memory pressure surfaces instead of hiding
in a swapfile. `FORMAT` selects `arrow_stream` or `json_each_row`; the six knobs
above arrive as `VECTOR_THREADS` and `SINK_*`. **That recipe runs the image's
defaults**, which are kept equal to the published knobs.

The config itself can be re-checked at any time against the shipped binary:

```sh
docker run --rm spate-bench-vector validate --no-environment /etc/vector/vector.yaml
```

## Versions

| Component | Coordinate / image | Version |
|---|---|---|
| Vector | `timberio/vector:0.57.0-debian` | 0.57.0 (2026-07-14) |
| Kafka client | librdkafka (statically linked by upstream) | as shipped in 0.57.0 |
| Avro decoder | `apache-avro` (Vector's `avro` codec) | as shipped in 0.57.0 |

`[version]` in the descriptor resolves the version by running the image
(`vector --version`) and `pinned = "0.57.0"` refuses the run on mismatch, so a
base-image bump cannot publish a mislabelled number. The `-debian` variant
rather than `-distroless-static`: glibc, plus a shell for the version command.

## Differences worth knowing

- **The Schema Registry is never contacted.** Vector has no registry
  integration; the decoder takes a static inline schema and
  `strip_schema_id_prefix` discards the 5-byte Confluent frame *without
  validating the schema id*. The arm pays no registry lookup, ever (the other
  arms pay one and then cache), and cannot detect a writer-schema change
  mid-run. Declared in `[[deviations]]`; the baked schema is manufactured at
  image build from the committed `.avsc` so it cannot drift.
- **`arrow_stream` is beta in 0.57** and fetches the target table's schema from
  `system.columns` once at sink start-up — a single query, outside the hot
  path, but a start-up dependency on the target the `json_each_row` variant
  does not have. Also declared in `[[deviations]]`.
- **It does not send `insert_deduplication_token`** — like every non-Spate arm.
  The shared DDL sets `non_replicated_deduplication_window = 1000`, so
  ClickHouse hashes this arm's blocks and skips hashing Spate's. Its duplicate
  count is reported rather than suppressed.
- **`upcase` is Unicode uppercase, not ASCII.** The contract specifies
  ASCII-only. On this corpus the two agree — metric names are drawn from a
  fixed set of lowercase ASCII identifiers — and the correctness gate's
  checksum over `name_upper` would fail the arm if that ever stopped being
  true. Noted because on another corpus (`ß` → `SS`) this would be a real
  difference, exactly as the Flink arm's hand-rolled `asciiUpper` documents
  from the other direction.
- **`value_scaled` goes through f64.** VRL's `/` is float division; the
  numerator is below 2^41 and the divisor at most 100, both exact in f64 and
  far below 2^53, so the quotient's integer part is exact and `to_int`
  truncates toward zero — the specified semantics. The `?? 0.0` in
  [`transform.vrl`](transform.vrl) only arms the type checker's divide-by-zero
  case, unreachable since `seq >= 0`.

## Traps a reviewer should check us on

- **Multiple kafka sources in one consumer group.**
  [Issue #21329](https://github.com/vectordotdev/vector/issues/21329) reports
  sources in the same group interfering — in the *different-topics* case. This
  arm's eight sources consume the **same** topic, which is the configuration
  the one-source-per-partition guidance in
  [#15884](https://github.com/vectordotdev/vector/discussions/15884) describes,
  and `cooperative-sticky` is set precisely so the eight consumers settle
  cleanly. If upstream review (rule 7) says this topology mis-serves Vector,
  that is exactly the PR this repository most wants.
- **The remap fan-out is the throughput risk.** One event in, an array of up to
  100 objects assigned to `.` — per-element object construction in VRL is the
  hottest code in the arm. If it binds, the rule-1-compliant fallbacks are a
  restructured VRL program (build the child rows with fewer intermediate
  allocations) or splitting the eight shapes across 2–4 clickhouse sinks
  (partitioned by input) to widen the sink side; both are configuration, not
  code Vector does not ship.
- **`arrow_stream` is beta.** If it misbehaves, `json-each-row` is the same arm
  with one env var changed, and both are published regardless.

## Gregg's question (rule 6)

To be answered from the measured run before publication. The candidate: the
per-event object construction in the eight remap tasks — the fan-out allocates
~100 child objects per message in VRL, where Spate's `flat_map` reuses decoded
buffers — with `vector top`/cgroup CPU attribution as the evidence either way.
If the sink side binds instead, the fixed `request.concurrency = 8` against the
measured ingest ceiling is the number to cite.
