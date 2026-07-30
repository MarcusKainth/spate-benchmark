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

## `c8g-8xl-ec2-docker` — the environment

**Class: authoritative.** A fresh machine per run, launched by
[the pipeline in this repository](reproduce.md#reproducing-the-cloud-environment)
and terminated when the run ends, so no state survives between measurements.

| | |
|---|---|
| Host | AWS EC2 c8g.8xlarge, on-demand |
| CPU | Graviton4 — 32 physical cores, homogeneous, no SMT |
| Memory | 64 GiB |
| Storage | EBS gp3 500 GiB, provisioned at 10,000 IOPS / 1,000 MiB/s |
| OS | Ubuntu 24.04, Docker CE, arm64 |

### Why it earns the class

Every vCPU is a dedicated physical core: Graviton has no simultaneous
multithreading and the Nitro hypervisor does not oversubscribe, so a cgroup CPU
cap means what it says. Docker here is Docker CE on Linux — containers are plain
cgroups, the same mechanism the envelope enforcement and the sampler read, with
no VM between the harness and the kernel. A JVM on this box **is** a JVM on
Linux. The storage is deliberately over-provisioned so the disk is headroom
rather than a variable, and its exact provisioning is part of the committed
profile: changing it would change what is being measured.

It is not bare metal — a `*.metal` instance would remove the hypervisor
entirely — and it is rentable by anyone, which is the point: the environment is
reproducible with an AWS account and this repository, not with access to our
hardware.

## The envelope

**Per system: 6 CPUs and 24 GiB of data plane.** A control plane — a Flink
JobManager, a Connect coordinator — is allocated on top, with its *measured*
consumption published alongside the arm's total rather than pre-charged against
it. Swap is disabled so memory pressure surfaces rather than hiding.

Memory is deliberately far more than any arm needs, so that no garbage-collected
runtime is penalised for an allocation we chose.
[The resource envelope](./contract/envelope.md) states what that does to the memory
number,
which is the honest cost of the choice.

**Infrastructure sits outside every arm's budget** and is identical for all of
them: Redpanda at 3 CPUs / 8 GiB, ClickHouse at 16 CPUs / 16 GiB, an 8-partition
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
