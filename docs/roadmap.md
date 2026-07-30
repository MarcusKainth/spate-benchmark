---
id: roadmap
title: Roadmap
description: Which systems are measured, which are not yet, and exactly what each unmeasured one is waiting on.
---

A partial comparison invites one accusation above all others: *you only measured
the ones you beat*. The only defence is to name what is missing and what it is
waiting on, in the descriptors CI checks — which this page mirrors — rather than
in prose that quietly rots.

The Implemented table lists the arms whose descriptors are `active`. Each entry
under "Not yet measured" comes from `[planned].blockers` in that system's
`entrant.toml`; validation refuses a planned entrant that does not say why.

## Implemented

| System | Runtime | Arms |
|---|---|---|
| **Spate** | native (Rust) | Native and RowBinary wire formats |
| **Apache Flink 2.2.1** | JVM | RowBinaryWithNamesAndTypes, via the official ClickHouse connector |
| **Vector 0.57.0** | native (Rust) | ArrowStream and JSONEachRow wire formats |
| **Kafka Connect 4.3.1 + `clickhouse-kafka-connect` v1.4.0** | JVM | RowBinary into a Null-engine landing table, flattened by a ClickHouse materialized view — Connect has no fan-out operator, so the transform's CPU moves to the server and the arm leans on ClickHouse's own profiling, declared in `[[deviations]]` |

The Kafka Connect arm's former licence gate is **closed**: it runs on the ASF's
own `apache/kafka` image with an Apache-2.0 connector and a POM-verified
Apache-2.0 Avro converter — no Confluent-distributed image, and no
Community-Licence artefact, is present. The converter's non-Central origin is
declared as a deviation on the entrant.

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
