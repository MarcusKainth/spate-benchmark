---
id: limitations
title: Limitations
description: What this benchmark does not measure, where it is weakest, and how it could mislead you.
---

Every benchmark is an argument, and every argument has a shape it cannot make.
This page is the list of things a sceptical reader would raise, written before
they raise them.

## It is one workload, chosen by the author of one of the entrants

Kafka → Avro → ClickHouse is the pipeline Spate was built for. A workload chosen
by someone else would be a stronger test. The corpus, the DDL, the schema and the
rules are all committed precisely so that someone else *can* argue with the
choice — but the choice was still ours.

## It measures throughput per unit of CPU, and almost nothing else

Not covered, and not claimed:

- **Windowing, keyed state, watermarking, event time.** Flink has all of it and
  Spate has none of it. If your problem is "aggregate over time windows keyed by
  user", this comparison is not about your problem and Flink is the answer.
- **Exactly-once.** Every arm here runs at-least-once. A system that can only
  offer exactly-once would pay for it, and that would be a real cost this
  benchmark is not measuring.
- **Failure and recovery.** Nothing here kills a broker mid-run, withdraws
  ClickHouse, or measures what each system does about it. That is a planned
  section and it is likely the most interesting one, because it is where systems
  genuinely differ.
- **Multi-node scaling.** One process, one container, one node. Flink's job graph
  and Connect's worker model exist to scale across machines; measuring them on
  one machine measures the part of them that matters least.
- **Schema evolution, backpressure from a degraded sink, mixed workloads,
  operational cost, ecosystem, hiring pool.** All real, none measured.

## The memory number is not a minimum footprint

Every arm is given far more memory than it needs, deliberately, so that no
garbage-collected runtime is penalised for an allocation *we* chose. The
consequence is that the memory figure measures what a system chooses to use when
nothing forces it to economise, not what it requires. See
[the methodology](./methodology.md) for the full argument. If you want "how small
can this run?", that is a different sweep and it has not been done.

## The host is a Mac, and that is a real weakness

Docker Desktop runs a Linux VM; a JVM inside it on arm64 is not a JVM on Linux
bare metal. The cores are heterogeneous and the hypervisor maps vCPUs across
performance and efficiency cores non-deterministically, which is an irreducible
variance source — run-to-run spread reached 14.5% on throughput.

Benchmarking guidance we consulted recommended dropping macOS results from scope
entirely. That was considered and declined, so the numbers are labelled
indicative and every record carries full environment provenance, which makes a
later bare-metal re-run an added dataset rather than a rewrite. It remains the
single most reasonable thing to attack about this suite.

## Tier B has not been run

Every current record is tier A — decode, flatten, insert. The transform tier is
specified and implemented but unmeasured.

## Most systems are not here yet

Four of the six declared entrants are unimplemented. Their blockers are written
down in [the roadmap](./roadmap.md) rather than left implicit, because "we only
measured the ones we beat" is exactly the accusation a partial comparison invites
and the only defence is to name what is missing and why.

## All benchmarks are liars

Including this one. The numbers are produced honestly and the method is published
in full, and neither of those makes a result transferable to your workload, your
data shape, your hardware or your operational constraints. Use it to form a
hypothesis, then measure your own pipeline.
