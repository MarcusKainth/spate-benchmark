---
id: roadmap
title: Roadmap
description: Which systems are measured, which are not yet, and exactly what each unmeasured one is waiting on.
---

A partial comparison invites one accusation above all others: *you only measured
the ones you beat*. The only defence is to name what is missing and what it is
waiting on, in a file that CI checks rather than in prose that quietly rots.

Each entry below comes from `[planned].blockers` in that system's
`entrant.toml`. Validation refuses a planned entrant that does not say why.

## Implemented

| System | Runtime | Arms |
|---|---|---|
| **Spate** | native (Rust) | Native and RowBinary wire formats |
| **Apache Flink 2.2.1** | JVM | RowBinaryWithNamesAndTypes, via the official ClickHouse connector |
| **Vector 0.57.0** | native (Rust) | ArrowStream and JSONEachRow wire formats |

## Not yet measured

### ClickHouse Kafka table engine

The zero-framework baseline: ClickHouse consuming the topic itself, with no
network hop between consumer and storage. It may win outright, and it is
published if it does.

It does not fit the envelope the other arms are held to, and that has to be
resolved honestly rather than fudged — the consumption happens *inside* the
ClickHouse container, so its CPU is not separable from the server's by the cgroup
sampler. Either this arm runs against a dedicated ClickHouse whose whole container
is the envelope, or it is reported on a different basis and labelled so.

### Kafka Connect + `clickhouse-kafka-connect`

Connect has no fan-out operator: one Kafka record cannot become a hundred
ClickHouse rows inside it, so the arm must land the nested array and flatten with
a materialized view. That is a legitimate real-world pattern and an interesting
result, but it moves CPU to the server where the cgroup sampler does not see it,
so this arm has to lean on ClickHouse's own profiling and say why.

**Publication is additionally gated on a licence review.** The
Confluent-distributed Connect images are under the Confluent Community Licence,
which is not OSI-approved.

### Redpanda Connect

No native ClickHouse output at all, so the arm goes through `sql_insert` with the
Go driver — a third distinct insert path, which appears in the results table
because it is not the same amount of server-side work.

Batching defaults to disabled. Running it unbatched would produce a meaninglessly
bad number, and publishing that would be exactly the failure the rules exist to
prevent: a slow competitor arm is a bug in the benchmark, not a result.

**Publication is additionally gated on a licence review** — BSL 1.1 with a
Community Licence over parts of the connector set.

## Beyond the entrant list

- **Failure modes.** Withdraw ClickHouse for sixty seconds and record what each
  system does — buffer, drop, block, or crash — and whether any rows are lost.
  Cheapest of the outstanding work and likely the most interesting result.
- **A latency curve.** Percentiles against offered load, which needs a sweep mode
  the harness does not have yet.
- **The first published sweep.** The cloud pipeline is built: an approval-gated
  pipeline launches a disposable EC2 c8g.8xlarge, runs the suite, and returns
  results as a validated pull request. What remains before numbers publish:
  measure the environment's ceilings (`bench ceiling --measure --write` via the
  pipeline's bootstrap mode) and land the first sweep.
- **Sending competitor configurations upstream** and asking whether we
  handicapped anyone, then linking whatever comes back — including "they told us
  to change X and we did".
