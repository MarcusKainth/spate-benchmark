# The Spate arm

Kafka → Confluent-framed Avro → `flat_map` → ClickHouse, held to
[`../../METHODOLOGY.md`](../../METHODOLOGY.md), which is normative. Read that
first; this file records only what is specific to Spate.

**This is the vendor's own entrant.** Everything below is written to be attacked.
If a competitor's arm is tuned worse than this one, that is a bug in the
benchmark — open an issue.

## Build and run

```sh
bench build spate
bench run spate --reps 3
```

The build context is the repository root: the arm is a cargo workspace member and
needs the workspace manifest and lockfile. While the framework is a private git
dependency the build also needs a credential, passed as a BuildKit secret so no
token is ever baked into an image layer.

## Configuration, and why each value

| Setting | Value | Reason |
|---|---|---|
| `commit_interval` | `5s` | Matched to Flink's 5s `AT_LEAST_ONCE` checkpoint interval. Both arms pay for the same guarantee at the same cadence; making ours cheaper would be exactly the fault rule 3 forbids. |
| `threads` | 4 | One per CPU in the envelope. |
| `shards` × `inflight` | 4 × 4 | Egress concurrency. Several shards against one server is how a single-node target gets concurrent inserts: each shard is an independent worker with its own in-flight permits. Measured: widening this from 2 to 32 moved drain throughput 3.25M → 4.81M rows/s, so it is a real lever and leaving it at the default would have under-reported the framework. |
| `linger` | `500ms` | Not a throughput constraint at these rates — `max_rows` fills in ~279ms at 940k rows/s, so the batch seals on rows first. It **is** a hard p99 floor at low rates, so latency runs should lower it rather than raise it. |
| `max_rows` | 262144 | The batch size at which the sink stops being per-insert-overhead-bound. |
| `max_inflight_bytes` | derived | Scales with `shards × inflight × max_rows`. Fixed here it would cap the pipeline on a number *we chose* while a sweep concluded that egress concurrency does not help. |
| `async_insert` | `0` | Off, matching every other arm. Async inserts would move batching into the server and make the comparison one of ClickHouse settings rather than of frameworks. |
| `metrics.exporter` | `none` | Nothing this arm reports about itself is used for any published number. |

## Deviations

None. This arm implements the specification directly: it has a fan-out operator,
a Native encoder, and a Kafka source with offset commits, so nothing in the
workload has to be worked around.

That is worth stating plainly rather than leaving as an empty section, because it
is an *advantage* of being the system the benchmark was designed alongside. The
Kafka Connect arm has no fan-out operator and must flatten in a materialized
view; that deviation is real work the workload forces on it and not on us.

## Where this arm may still be unfair — to us or to others

- **The workload suits us.** Kafka → Avro → ClickHouse is the pipeline this
  framework was built for, and the benchmark was written by its author. A
  workload chosen by someone else would be a stronger test, which is why the
  contract invites competitor pull requests and why the corpus, DDL and rules are
  all committed rather than described.
- **`native` is not like-for-like with Flink.** It is what a real deployment
  runs, so it is published — but the arm to read against Flink is
  `tier-a-rowbinary`, because ClickHouse's official Flink connector can only
  write `RowBinaryWithNamesAndTypes`. Native measured 1.58× RowBinary, and that
  gap is server-side parse and wire volume, not our encoder: client CPU per row
  is nearly equal between the two. Presenting Native against Flink would be
  claiming credit for a gap in the Java client.
- **The typed decode path is published even though it loses.** `build_serde` cost
  +34% CPU per row for no throughput gain, held at both 20 and 100 events per
  message. Rule 1 requires the same treatment of our own shipped APIs that we
  give everyone else's, so both are shown.
- **Tier B has not been run.** All current records are tier A.

## Why was throughput X and not 2X?

**Rule 6, and it is currently unanswered for this arm.**

At its ceiling the arm ran at roughly 0.6 µs of CPU per row against a 4-CPU
budget — about 3 of 4 cores — which is the healthy picture of a pipeline that is
near CPU-bound rather than blocked on something. But "near" is not an answer, and
the remaining core is not accounted for.

<!-- TODO(bench): fill this in from the next recorded run. Candidates, in the
     order they should be eliminated:
       - egress round-trip: shards x inflight sealed batches pending against a
         single ClickHouse; the insert acknowledgement latency bounds it.
       - the consume path: 8 partitions gives 8 fetch streams to one process.
       - server-side parse: measurable directly from ClickHouse ProfileEvents.
       - allocator pressure in the flatten, which is the one thing here that is
         our own code rather than a boundary.
     The results table publishes this sentence per arm; an empty one is a gap in
     the evidence, not a formatting problem. -->
