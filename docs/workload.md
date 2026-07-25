---
id: workload
title: The workload
description: The one pipeline every system implements — schema, target, tiers, and the deterministic corpus.
---

One workload, fixed. Every system implements this and nothing else, which is what
makes the numbers comparable at all.

Consume one Kafka topic of Confluent-framed Avro `SensorBatch` messages, decode
them, flatten each message's `events` array into one row per event, and insert
those rows into ClickHouse. One message becomes **100** rows, so the fan-out is
real work rather than a pass-through.

## The contract

- **Schema** —
  [`workload/schema/sensor_batch.avsc`](https://github.com/MarcusKainth/spate-benchmark/blob/main/workload/schema/sensor_batch.avsc).
  Two-level nesting with an inner array and nullable unions, so every
  implementation goes through a real union-decode path. Arms read this file; none
  re-declares the schema inline.
- **Target** —
  [`workload/clickhouse/ddl.sql`](https://github.com/MarcusKainth/spate-benchmark/blob/main/workload/clickhouse/ddl.sql).
  Column order is the wire contract.
- **Framing** — Confluent (`0x00` + big-endian schema id + datum) against a live
  Schema Registry, so registry-based decoding is exercised as it would be in
  production.

## Two tiers

**Tier A — transport.** Decode, flatten, insert. Column mapping only.

**Tier B — transform.** Tier A plus, in this order: drop rows whose unit is the
sentinel; drop rows whose `quality` is non-null and below the floor; coalesce a
null region to the empty string; derive a scaled value by integer division; and
uppercase the metric name **ASCII-only**.

That last constraint looks pedantic and is not. Java's `String.toUpperCase()` is
locale-dependent, so "uppercase" alone would be a different operation in
different languages and the checksum gate would fail for a reason that had
nothing to do with the systems under test.

## The corpus is a pure function of `batch_id`

Every field is derived arithmetically from the batch identifier. Three things
follow, and they are why the correctness gates can say more than "the same number
of rows arrived":

- The expected row count, checksum and per-tier filtered counts are computable in
  closed form, **without reading what any system produced**. A system that
  transforms wrongly fails as loudly as one that drops rows.
- `(batch_id, seq)` is a true row identity, so exact-distinct is an exact loss
  count and `count() - distinct` is an exact duplicate count.
- A prefilled topic can be regenerated identically months later on a different
  machine, to re-run a published arm.

The generator's constants live in
[`workload/workload.toml`](https://github.com/MarcusKainth/spate-benchmark/blob/main/workload/workload.toml),
and `dataset_version` is **derived from their content** together with the schema
and the DDL. A change to what the data is therefore cannot be made without the
version moving, which is what stops two result sets from different corpora being
placed on the same axis.

## Correctness gates block publication

An arm that fails any of these produces no published number:

- no loss — distinct row identities must equal the expected count;
- the checksum over distinct rows must match the producer's, so both systems did
  the same arithmetic;
- duplicates are **reported as a metric, not suppressed** — at-least-once systems
  may legitimately have some, and hiding that would misrepresent the guarantee.

Gating happens after a quiesce, not at the end of the measurement window. The
producer round-robins across partitions and consumers drain them independently,
so at any instant the consumed frontier is ragged: the most advanced partition
sets the maximum while slower ones leave holes below it. Gating on that snapshot
reports those holes as data loss, which is how the problem was found.
