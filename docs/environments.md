---
id: environments
title: Environments
description: The hardware every number is tied to, and why results are never compared across machines.
---

An environment is the unit of comparability for hardware. Every record carries an
environment id, and this site never draws two environments on one axis.

That is why an environment is a committed profile with a stable id rather than a
hostname. A hostname is not a hardware disclosure — it cannot be compared across
machines and tells a reader nothing they can reproduce against. Each record also
carries a digest of the profile, so editing it later cannot retroactively
re-describe runs that already happened.

## `m5max-mbp-docker` — the reference environment

**Class: indicative.** Not authoritative, and the site renders that caveat from
this field rather than from a string match on the operating system — so when a
bare-metal environment is added, the banner disappears on its own rather than
having to be remembered.

| | |
|---|---|
| Host | Apple MacBook Pro, M5 Max |
| CPU | 18 cores — 6 performance + 12 efficiency |
| Memory | 128 GiB, with 72 GiB given to the Docker VM |
| OS | macOS, Docker Desktop Linux VM, arm64 |

### Why it is labelled indicative

Docker Desktop runs a Linux VM. A JVM inside it on arm64 is not a JVM on Linux
bare metal, and no arm here should be read as one.

The cores are heterogeneous and the hypervisor maps VM vCPUs across performance
and efficiency cores non-deterministically. `--cpuset-cpus` would pin to VM
vCPUs, not physical cores, so it does not help. This is an irreducible variance
source: run-to-run spread reached **14.5%** on throughput, which is why three
repetitions is the floor rather than a nicety and why every chart shows spread.

## The envelope

**Per system: 4 CPUs and 16 GiB of data plane.** A control plane — a Flink
JobManager, a Connect coordinator — is allocated on top, with its *measured*
consumption published alongside the arm's total rather than pre-charged against
it. Swap is disabled so memory pressure surfaces rather than hiding.

Memory is deliberately far more than any arm needs, so that no garbage-collected
runtime is penalised for an allocation we chose.
[The resource envelope](./contract/envelope.md) states what that does to the memory
number,
which is the honest cost of the choice.

**Infrastructure sits outside every arm's budget** and is identical for all of
them: Redpanda at 4 CPUs / 8 GiB, ClickHouse at 9 CPUs / 12 GiB, an 8-partition
topic. It is declared in the environment profile rather than passed on the
command line — which is the fix for a real failure, where a runner script, the
driver's defaults and the written methodology stated three different envelopes
while no record said which had been in force.

The driver applies what is declared, then **reads the caps back out of the
running containers' cgroups and asserts they match.** A mismatch fails the run
rather than warning.

## The headroom rule

Before any arm is published, a ceiling pass measures what the shared consume path
can absorb. **An arm exceeding 70% of that ceiling is infra-bound and cannot be
published as a system comparison** — above it we are measuring the broker and
ClickHouse, not the system.

Such a run is recorded with a failed status rather than discarded, so "we ran it
and it blew the limit" stays distinguishable from "we never ran it".

## Adding one

Add a profile to `environments/`, run against it, and the site will keep its
results in their own comparability group automatically. Results from hardware we
do not control are welcome and are flagged as such — no rendering logic is
needed, because a different environment id already separates them.
