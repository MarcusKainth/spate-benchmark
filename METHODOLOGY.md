# The fairness contract

**This file is normative.** Every entrant here — including Spate's own — conforms
to it. If you are implementing an arm, this is your complete specification; read
it before writing code, and if it is ambiguous, treat that as a bug in this file
and say so rather than guessing. An arm that quietly deviates is worse than no arm
at all, because it produces a number we would then publish.

## The one-sentence goal

Find out, honestly, how much throughput each system delivers per unit of CPU and
memory on the same pipeline, the same bytes, and the same hardware — and publish
it whether or not Spate wins.

We expect to lose some arms. The ClickHouse Kafka engine has no network hop.
Vector is Rust with the same no-GC story. Those results get published with equal
prominence, because a comparison page containing only wins is read as marketing
and convinces nobody.

## The pipeline

Consume one Kafka topic of Confluent-framed Avro `SensorBatch` messages, decode
them, flatten each message's `events` array into one row per event, and insert
those rows into ClickHouse.

- Schema: [`workload/schema/sensor_batch.avsc`](workload/schema/sensor_batch.avsc)
  — read this file, do not re-declare the schema inline. The `events` array
  carries **100** events per message; the array is unbounded in the schema, so
  this is a generator constant and not a schema change. It was raised from 20 to
  buy consume-path headroom.
- Target: [`workload/clickhouse/ddl.sql`](workload/clickhouse/ddl.sql) —
  `sensor_events` for tier A, `sensor_events_t` for tier B. Column order is the
  wire contract.
- Wire format: Confluent framing (`0x00` + big-endian u32 schema id + datum),
  subject `sensor-batches-value`, against a live Schema Registry.

**Tier A (transport):** decode → flatten → insert. Column mapping only, including
coalescing a null `region` to `''` (forced by the target column type).

**Tier B (transform):** tier A plus, in this order — drop rows where
`unit = 'drop'`; drop rows where `quality` is non-null and `< 0.2`; coalesce null
`region` to `''`; compute `value_scaled = value * 1000 / (event_seq + 1)` as
integer division truncating toward zero; compute `name_upper` as the **ASCII-only**
uppercase of `name`. ASCII-only is specified because Java's `String.toUpperCase()`
is locale-dependent, so "uppercase" alone would not be the same operation in every
language.

## Delivery semantics: at-least-once, matched

Every arm runs **at-least-once**, and every arm's durability mechanism is
configured to a comparable interval (Spate commits offsets every 5s; Flink
checkpoints every 5s in `AT_LEAST_ONCE` mode; `clickhouse-kafka-connect` runs
`exactlyOnce=false`).

Turning a system's fault tolerance off to make it faster is not permitted, even
though it would flatter the numbers of whichever arm we did it to. We are
comparing guarantee-for-guarantee. If a system can only offer exactly-once, we run
it that way and label it, rather than pretending the guarantee is free.

Each entrant declares its semantics in `[guarantees]` in its descriptor, and the
site renders them beside the numbers.

## The resource envelope

**4 CPUs and 16 GiB of _data plane_ per system.** A system's control plane — a
Flink JobManager, a Connect worker's coordinator — is allocated **on top** of that
budget, and its **measured** consumption is published alongside the arm's total
rather than pre-charged against it.

This is a deviation from the more obvious rule ("4 CPU / 16 GiB total, control
plane included"), it is deliberate, and it is disclosed here and on the site
because it favours the multi-process arms:

> Charging a whole JobManager against a single TaskManager is an artefact of
> running one TaskManager. In production one JobManager serves an entire cluster,
> so a per-TM share of it is a rounding error. Measurement bears this out — the
> JobManager consumed **0.03 cores**, so charging it a full core would have taxed
> Flink roughly 33× its real cost, and the resulting "win" would have been an
> artefact of our own accounting.

Every arm therefore publishes two figures: the arm total, and
`data_plane_cores_used` / `data_plane_peak_anon_mb` for the data plane alone. A
reader who disagrees with this rule can apply the stricter one from the published
numbers; a reader given only a blended total could not.

Each entrant declares its containers with roles in `[[envelope.container]]`, and
validation asserts that exactly one is `data-plane` and that the data-plane
containers sum to the declared `[envelope]` totals. The driver applies exactly
what is declared, then **reads the caps back out of the running containers' cgroups
and asserts they match**. A mismatch fails the run; it does not warn.

Swap is disabled (`--memory-swap` equals `--memory`) so memory pressure surfaces
instead of hiding in a swapfile.

### Why memory is generous, and what that does to the memory number

CPU is the scarce resource here and memory is not. The reference host has 128 GiB
with 72 GiB given to the Docker VM, one arm runs at a time, and no arm in this
workload needs more than a couple of gigabytes to do its job. So every arm gets
16 GiB — several times what any of them will touch.

That is a fairness decision rather than a convenience. A garbage-collected
runtime held to a tight heap collects more often, and the resulting pauses would
be an artefact of *our* allocation choice rather than a property of the system.
Sizing a JVM down until it strains and then publishing its pause distribution is
a way to win an argument on purpose. The same allowance goes to every arm
including the Rust one, which will leave most of it untouched.

**The honest cost is that the memory figure stops being a requirement and becomes
a revealed preference.** Under a tight cap, peak anonymous memory approximates
what a system *needs*. Under a generous one it approximates what a system
*chooses to use when nothing forces it to economise* — a JVM will grow its heap
toward its maximum under load without ever being close to needing it. Both are
real quantities, but they are different ones, and this suite measures the second.

So the memory panel is labelled as what it is and is **not** presented as a
minimum footprint. "How small can this run?" is a different question, and
answering it properly means a separate sweep that tightens each arm until it
degrades. That would be worth publishing; it is not what these numbers are.

Every arm publishes `peak_anon` and `memory.peak`, and JVM arms publish
configured versus actually-used heap, so the gap between allocation and use is
visible rather than implied.

Infrastructure sits **outside** that budget and is identical for every arm, and is
declared per environment rather than passed on the command line: Redpanda
(8 CPUs, 8 GiB) and ClickHouse (5 CPUs, 12 GiB) on the reference environment.

The Schema Registry is **Redpanda's built-in, Confluent-compatible one** on port
8081 rather than a separate Confluent container. That removes a second JVM from
the measurement environment entirely and returns a CPU and a GiB to the infra
budget, which is headroom the ceiling pass needs. It speaks the same REST API that
Kafka Connect's `AvroConverter` and ClickHouse's `AvroConfluent` expect. Host-side
it is published on `localhost:18081`; containers reach it at
`http://spate-bench-redpanda:8081`.

Before any arm is published, a ceiling pass measures what ClickHouse and the
broker can actually absorb at those caps. **An arm exceeding 70% of either ceiling
is infra-bound and cannot be published as a system comparison** — at that point we
are measuring ClickHouse, not the system. If every arm hits the ceiling, the
envelope shrinks until they are engine-bound, and that search is documented rather
than quietly performed. Such a run is recorded with `status: infra_bound` rather
than discarded, so "we ran it and it blew the limit" is distinguishable from "we
never ran it".

## How you are measured — and why you should not instrument anything

**Do not add metrics, timers, or counters for the benchmark's benefit.** Nothing a
system reports about itself is used for any published number. If your
implementation exposes metrics that it would expose in production, leave them;
they are useful for debugging. They will not be read as results.

Everything published comes from outside the system under test:

| Quantity | Source |
|---|---|
| Throughput | `SELECT count()` against ClickHouse, sampled by the driver. Broker offset delta as cross-check. |
| CPU | cgroup v2 `cpu.stat` (`usage_usec`, `user_usec`, `system_usec`), sampled at 1 Hz by a sidecar container |
| Memory | cgroup v2 peak **anonymous** memory, plus `memory.peak` and (for JVM arms) configured vs used heap |
| Latency | `ingest_ts - send_ts` computed in ClickHouse, where `ingest_ts` is a `MATERIALIZED now64(6)` column |
| Server-side cost | ClickHouse's own `ProfileEvents` CPU-per-row, via `system.query_log` |
| GC (JVM arms) | `-Xlog:gc*` / JFR `jdk.GCPhasePause` |

The sampler is a sidecar container that mounts the cgroup filesystem read-only
with `--cgroupns=host`. It deliberately does not `docker exec` into the arm, because
an arm's image may have no shell — Spate's is distroless — and the same measurement
must work for every arm regardless of base image.

### Which mode measures what, and why

**Throughput and efficiency come from DRAIN mode. Latency comes from SUSTAINED
mode.** That split is forced by the host, and it was measured rather than assumed.

The reference box has 18 vCPU. The system's 4, plus the broker's, plus
ClickHouse's, plus a load generator wide enough to offer millions of rows/s, plus
the driver, **exceeds 18**. The consequence is not subtle: under sustained load,
widening the Spate arm's egress concurrency from 2 to 32 concurrent inserts changed
its throughput not at all, which reads exactly like "egress concurrency does not
matter". Measured properly in drain mode it gives 3.25M → 4.81M rows/s, a 1.48×
gain. The sustained result was host contention, not a property of the system.

**Produce first, then consume.** Drain mode populates the topic completely before
any arm starts, and runs no producer during the measurement. Two things follow, and
both matter: the broker does only its *read* path rather than serving writes and
reads at once, and the generator's CPU is entirely outside the window. We are not
benchmarking the broker, so it must never be doing concurrent work it would not be
doing in the measurement we claim to be making. `drain` is therefore the default
mode; `sustained` has to be asked for.

Two disclosures follow from prefilling. The corpus is largely served from the
broker's page cache, which is favourable — equally, for every arm — and is stated
rather than hidden. And the broker's *fetch* path is still inside the measurement,
because the pipeline under test is Kafka → ClickHouse; the requirement is not that
the broker is absent but that it is not the **bottleneck**, which is what the
ceiling pass exists to prove.

Latency is only meaningful in **sustained** mode: in drain mode `send_ts` is a
prefill timestamp, so the difference measures backlog age rather than pipeline
latency. Drain mode therefore reports throughput only.

A sustained arm that cannot keep up with the offered rate is recorded as
`SATURATED` with a `kept_up_share` metric. Such a point is a genuine ceiling
measurement, but its latency figures describe backlog age and must not be read as
latency at the offered rate.

## Rules

1. **Use the best API the system ships — do not hand-write its internals.** This
   is the rule that decides what is being compared. Decoding, encoding and
   transport must go through the system's own public, documented APIs, choosing
   the fastest one it offers. Configuration tuning is unlimited and expected
   (`pipeline.object-reuse`, buffer timeouts, memory sizing, parallelism), because
   configuration is not code we wrote.

   The reason is symmetry. On the Spate side we choose between two shipped
   deserializers (`build_value` and `build_serde`) and publish both numbers. If we
   hand-optimised a competitor's decoder we would no longer be measuring that
   system, we would be measuring our own Java or Go — and the mirror-image
   accusation, that we tuned a competitor until it lost, would be just as fair.

   Pipeline *logic* is different and is yours to write well: the flatten, the
   tier-B filter and the derived columns are user code in every system, and every
   arm writes them.

2. **Optimise hard within rule 1.** Tune as an expert who wants to win would:
   correct parallelism, correct memory sizing, no needless copying, no debug
   logging on the hot path. A slow competitor arm is a bug in our benchmark, not a
   result. Deliberately leaving a *configuration* win on the table is the same
   failure as fabricating a number.

   Where a system's shipped default is measurably suboptimal, that is a publishable
   finding about the system — but it must be **stated as avoidable**, with the cost
   quantified if a secondary arm can do so. Publishing a number that a competitor's
   expert could beat, without saying so, is the failure mode that destroys a
   comparison page's credibility.

3. **Only realistic configurations.** Use what a competent user would actually
   deploy. No pre-computing work outside the measured window; no dropping
   durability.

   Each variant declares `approach`, and the site defaults every chart to
   `realistic`:

   | `approach` | Meaning |
   |---|---|
   | `realistic` | Rules 1 and 3 satisfied. Headline-eligible. |
   | `tuned` | Rule-1 compliant, but a configuration a typical user would not deploy. Shown, filterable, never the headline. |
   | `stripped` | Uses code the project does not ship, or drops a guarantee. Never the headline; exists to quantify a specific effect. |

   The valve is not hypothetical: the Flink arm's `ReusingAvroDeserializationSchema`
   is code *we* wrote, so rule 1 bars it from the headline even though it makes
   Flink look better.

4. **Record every deviation.** If the system cannot express part of the spec, put
   it in `[[deviations]]` in the descriptor — machine-readable, so the site renders
   it from the same source the driver reads, and prose cannot drift from behaviour.
   Kafka Connect, for instance, has no fan-out operator, so its arm must land the
   nested array and flatten with a ClickHouse materialized view — a legitimate
   real-world pattern and an interesting result, but it moves CPU to the server and
   must be disclosed, not smoothed over.

5. **Report the insert format.** Native, RowBinary, JSONEachRow and a Go SQL driver
   are not the same amount of server-side work. Every arm's format appears in the
   results table, from `reports.wire_format` in its descriptor.

6. **Answer Gregg's question.** For your final configuration, write one sentence
   saying why throughput was X and not 2X — what the binding constraint was, and
   the evidence. This goes in the published results table. A table where every row
   explains its own bottleneck is what makes the page hard to attack.

7. **Expect your config to be reviewed upstream.** We intend to send competitor
   configurations to the relevant maintainers and ask whether we handicapped them,
   then link the answers. `[maintainer].reviewed_upstream` records whether that has
   happened, and the site shows it. Write configs you would be comfortable
   defending.

## Deterministic data generation

The producer is shared, so every system receives byte-identical input. The
generator is a pure function of `batch_id`, which is what lets the driver compute
the expected checksum in closed form and prove that two systems performed the same
arithmetic rather than merely moving the same row count.

For `batch_id` in `0..N`, with `EVENTS_PER_BATCH = 100`:

```
sensor       = "sensor-{batch_id % 1024}"
region       = null if batch_id % 10 == 0 else "region-{batch_id % 7}"
batch_ts_ms  = BASE_TS_MS + batch_id
send_ts_us   = intended schedule time (NOT the actual send time)

for seq in 0..100:
    name    = "metric_{(batch_id * 31 + seq) % 32}"
    unit    = UNITS[(batch_id * 7 + seq) % 8]     // UNITS[3] == "drop"
    value   = (batch_id * 1_000_003 + seq * 97) % 2_147_483_647
    quality = null if (batch_id + seq) % 5 == 0
              else ((batch_id * 13 + seq * 7) % 100) / 100.0
    tags    = ["tag-{(batch_id + seq + j) % 16}" for j in 0..((batch_id + seq) % 4)]
```

`(batch_id, seq)` is the row identity. `uniqExact((batch_id, event_seq))` is
therefore the exact loss gate, and `count() - uniqExact(...)` is the exact
duplicate count.

The generator's tunables live in [`workload/workload.toml`](workload/workload.toml)
so that `dataset_version` can be derived from their content rather than
hand-maintained.

## What invalidates a comparison

Three properties are **hard**: records that differ in any of them are never drawn
on the same axis, and the site renders an explicit "not comparable" note instead of
quietly averaging them.

| Field | Bumped when | Effect |
|---|---|---|
| `harness_version` | The measurement protocol changes in a way that moves numbers — the steady-state detector, the drain protocol, sampler semantics, the gate set, envelope enforcement. Not when a log message changes. | Whole result set split |
| `dataset_version` | The corpus, Avro schema, DDL or generator constants change. Derived from file content, so it cannot be forgotten. | Whole result set split |
| `env_id` | Different hardware or a different infrastructure envelope. | Comparable only within an environment |

Softer provenance — a ClickHouse patch release, a compiler version, a broker
version — is recorded on every record and rendered as a footnote. Refusing to
compare across a ClickHouse patch would make the suite unusable; refusing across a
protocol change is the entire point.

### Harness versions

`harness_version` is hand-maintained rather than derived, deliberately: "did this
change move numbers?" is a judgement, and a content hash would answer yes to every
typo fix and shatter every comparability group. This table is the record, and CI
asserts it stays in step with the constant in `harness/src/report.rs`.

| Version | Date | Change |
|---|---|---|
| 1 | 2026-07-25 | Initial protocol. Full-drain throughput, 1 Hz cgroup sampling, plateau-detected steady state, quiesce-then-gate correctness checks, 70%-of-ceiling headroom limit, envelope read back from cgroups. |

## Results are never overwritten

`bench run` appends. There is no code path in it that truncates a results file, and
that is enforced by the absence of the capability rather than by discipline.

A run later found to be wrong is **retracted, not deleted**: `bench retract` appends
a `superseded_by` marker naming the reason, and the site renders the number struck
through with that reason attached. A reader who saw a number once can always find
out what happened to it.

Re-running one system does not touch any other system's results. Records are
partitioned by `results/<env_id>/<entrant>/<YYYY-MM>.jsonl`, so a partial re-run
produces a diff confined to one file.

## Host caveat

The reference numbers are produced on Docker Desktop for macOS on Apple Silicon: a
Linux VM, arm64, with 6 performance and 12 efficiency cores that the hypervisor
maps to VM vCPUs non-deterministically. A JVM here is not a JVM on Linux bare
metal.

Benchmarking guidance we consulted recommended dropping macOS results from scope
entirely. That recommendation was considered and declined, so every environment
declares a `class`, this one declares `indicative`, and the site renders the
caveat from that field — prominently and in its own right rather than in a
footnote. It shows run-to-run spread on every chart, and carries full environment
provenance in every record, so a later bare-metal re-run is an added dataset rather
than a rewrite.
