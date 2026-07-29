# The resource envelope

Part of [the fairness contract](README.md). What each system is given, what
the infrastructure around it is given, and the headroom rule that decides
whether a number describes the system or the rig.

**4 CPUs and 16 GiB of _data plane_ per system.** A system's control plane — a
Flink JobManager, a Connect worker's coordinator — is allocated **on top** of that
budget, and its **measured** consumption is published alongside the arm's total
rather than pre-charged against it.

This is a deviation from the more obvious rule ("4 CPU / 16 GiB total, control
plane included"), it is deliberate, and it is disclosed here and on the site
because it favours the multi-process arms:

> Charging a whole JobManager against a single TaskManager is an artefact of
> running one TaskManager. In production one JobManager serves an entire cluster,
> so a per-TM share of it is a rounding error. Measurement bears this out — across
> the published Flink runs the JobManager consumed **0.066 to 0.088 cores**, so
> charging it a full core would have taxed Flink roughly 13× its real cost, and
> the resulting "win" would have been an artefact of our own accounting.

Every arm therefore publishes two figures: the arm total, and
`data_plane_cores_used` / `data_plane_peak_anon_bytes` for the data plane alone. A
reader who disagrees with this rule can apply the stricter one from the published
numbers; a reader given only a blended total could not.

Each entrant declares its containers with roles in `[[envelope.container]]`, and
validation asserts that exactly one is `data-plane` and that the data-plane
containers sum to the declared `[envelope]` totals. The driver applies exactly
what is declared, then **reads the caps back out of the running containers' cgroups
and asserts they match**. A mismatch fails the run; it does not warn.

Swap is disabled (`--memory-swap` equals `--memory`) so memory pressure surfaces
instead of hiding in a swapfile.

## Why memory is generous, and what that does to the memory number

CPU is the scarce resource here and memory is not: one arm runs at a time, and
no arm in this workload needs more than a couple of gigabytes to do its job. So
every arm gets 16 GiB — several times what any of them will touch.

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

Every arm publishes `peak_anon` and `memory.peak`. JVM arms are specified to
publish configured versus actually-used heap as well, so that the gap between
allocation and use is visible rather than implied; that part is not yet measured
and is marked as such in the table below.

Infrastructure sits **outside** that budget and is identical for every arm, and is
declared per environment rather than passed on the command line: Redpanda
(4 CPUs, 8 GiB) and ClickHouse (9 CPUs, 12 GiB) in the committed environment
profile.

Those two numbers are the output of a measured search rather than a guess: the
broker's cap was swept against its own cgroup counters until it was no longer the
constraint, and ClickHouse was given the cores that freed. The host bounds the
total: the arm's 4 CPUs, the driver and the sampler must still fit beside the
infrastructure.

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
are measuring ClickHouse, not the system. If arms hit the ceiling, an envelope
moves until they are engine-bound.

**Which envelope moves is a diagnosis, not a preference.** Shrinking the arms is
right when the arms are too big for the rig around them. When the infrastructure
is the thing at its cap, shrinking every arm makes the comparison smaller for all
of them and leaves the fault in place. Read the cgroup counters on both sides
first, and move whichever is at its cap. Such a run is recorded with
`status: infra_bound` rather than discarded, so "we ran it and it blew the limit"
is distinguishable from "we never ran it".
