# Spate Benchmark

A published, reproducible comparison of streaming ETL systems on one fixed
pipeline: **Kafka → Avro → ClickHouse**.

Results: **https://spate-benchmark.kainth.dev**

## Who runs this, and why that matters

This benchmark is built and run by the author of [Spate][spate], which is one of
the systems it measures. That is a conflict of interest, and the only useful
response to one is to make it impossible to hide:

- Every Spate row on the site carries a "run by the vendor" marker, driven by
  `vendor = "self"` in its entrant descriptor — not by anything hardcoded in the
  site.
- **No published number is reported by the system that produced it.** Throughput
  is `SELECT count()` against ClickHouse; CPU and memory are cgroup v2 counters
  read by a sidecar container; latency is computed inside ClickHouse from a
  materialized ingest timestamp. A framework's own metrics are available for
  debugging and are never read as results.
- Every competitor configuration is in this repository, in full, and we intend to
  send them upstream and ask whether we handicapped anyone. Whatever comes back
  gets linked — including "they told us to change X and we did".
- Where we lose, that is published with the same prominence as where we win. A
  comparison page containing only wins is read as marketing and convinces nobody.

If you think an arm is configured badly, that is a bug and we want the pull
request. See [CONTRIBUTING.md](CONTRIBUTING.md).

[spate]: https://github.com/MarcusKainth/spate-etl

## How to read a number here

**Every result carries the version of the system that produced it, the exact
image digest, the environment it ran on, and the date.** None of that is optional
and none of it is typed in by hand — a run whose image digest cannot be read is
recorded as failed rather than published.

Three things invalidate comparison outright, and the site refuses to draw records
across them rather than quietly averaging:

| If this differs | Then |
|---|---|
| `harness_version` | The measurement protocol changed. Not comparable. |
| `dataset_version` | The corpus or schema changed. Not comparable. |
| `env_id` | Different hardware. Not comparable. |

Softer differences — a ClickHouse patch release, a compiler version — are
recorded and shown as a footnote rather than treated as disqualifying.

**Results are never deleted.** A run found to be wrong is retracted by appending
a `superseded_by` marker; the record stays visible, struck through, with the
reason. Re-running one system does not re-run or overwrite any other.

## Current state

Measurements today come from a single macOS host (Docker Desktop, Apple Silicon,
heterogeneous cores). They are labelled **indicative, not authoritative**, and
that label is rendered from the environment's declared class, so it will
disappear on its own when a bare-metal Linux environment is added rather than
having to be remembered.

## Repository layout

```
harness/       the driver and the `bench` CLI. Has no dependency on any entrant.
entrants/      one directory per system. Adding a system touches nothing else.
workload/      the one canonical workload: Avro schema, ClickHouse DDL, generator.
environments/  hardware profiles, referenced by id from every record.
results/       append-only JSONL, partitioned by environment and system.
website/       the published site.
```

**[METHODOLOGY.md](METHODOLOGY.md) is normative.** Every implementation here,
including Spate's own, conforms to it. If it is ambiguous, that is a bug in the
document — say so rather than guessing.

## Running it

```sh
bench list                      # systems, variants, and when each was last measured
bench validate                  # what CI checks, runnable locally
bench prefill                   # populate the topic once per corpus
bench ceiling                   # prove the infrastructure is not the bottleneck
bench run '*' --reps 3          # every arm, interleaved
bench run spate --reps 3        # just one system; nothing else is touched
bench run --stale --reps 3      # anything whose pinned version has moved on
bench run '*' --dry-run         # print the plan without running it
```

`bench run` only ever appends. There is no code path in it that truncates a
results file.

## Licence

Code is [Apache-2.0](LICENSE). Published results in `results/` are CC-BY-4.0 —
use them, cite them, and please link back so a reader can check the provenance.
