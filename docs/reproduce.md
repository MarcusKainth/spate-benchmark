---
id: reproduce
title: Reproducing this
description: How to build the arms and run the benchmark yourself.
---

Everything here runs from a clone of
[the repository](https://github.com/spate-etl/benchmark). No number on
this site comes from anywhere else.

## What you need

- Docker, with enough headroom for the infrastructure and one arm at a time.
  The reference environment gives the Docker VM 18 CPUs and 72 GiB.
- A Rust toolchain. The version is pinned in `rust-toolchain.toml` and must match
  the arm image's base — a test asserts it, because codegen moves throughput and
  a silent divergence would make the recorded toolchain wrong.

## The commands

```sh
bench list                      # systems, variants, and when each was last measured
bench validate                  # what CI checks, runnable locally
bench build '*'                 # build every entrant image
bench prefill                   # populate the topic once per corpus
bench ceiling                   # report the ceilings, and refuse if none is gateable
bench ceiling --measure --write # re-measure them against this corpus and record it
bench run '*' --reps 3          # every arm, interleaved
bench run spate --reps 3        # one system; nothing else is touched
bench run '*' --dry-run         # print the plan without running it
bench run spate --mode sustained --rate 40000    # latency; has to be asked for
```

`--dry-run` is worth using before any full sweep. It prints the exact execution
list with resolved image digests, which is how you check that "only Spate" really
means only Spate before spending hours finding out.

## Two properties worth knowing

**Runs are interleaved, not batched.** `bench run` alternates between arms rather
than completing all of one and then all of the next. This is not fastidiousness:
running arms in sequence has already manufactured a fake 30% difference in a
related project, because the machine is not in the same state at the end of a
long run as at the start.

**Nothing appends over anything.** `bench run` only ever appends, and there is no
code path in it that truncates a results file. Re-running one system produces a
diff confined to that system's file, and a number later found to be wrong is
corrected in a commit of its own.

## If you get a different number

That is useful and I would like to know. The most likely causes, in order:

1. **A different environment.** These results are from a single macOS host with
   heterogeneous cores. Add your own environment profile rather than comparing
   across; the site will refuse to draw them on one axis, which is the intended
   behaviour rather than an obstacle.
2. **A busy machine.** Run-to-run spread reached 14.5% on throughput even on a
   quiet host.
3. **A real defect in the harness.** Open an issue. A benchmark that cannot be
   reproduced is a claim, not evidence.

## No credentials required

Every arm, including Spate's, builds from a clean clone of this repository. The
framework is consumed from crates.io and pinned in `Cargo.lock`, so the arm that
is ours is exactly as reproducible as every arm that is not.

## Reproducing the cloud environment

Published runs execute on a disposable EC2 box — one on-demand `c8g.8xlarge`
(Graviton4: 32 vCPUs that are 32 physical cores, no SMT), Ubuntu 24.04 arm64,
a 500 GiB gp3 volume provisioned at 10,000 IOPS and 1,000 MiB/s so storage is
never the bottleneck being measured. Every piece of that pipeline is in this
repository:

- `infra/` — the complete AWS footprint as Terraform, including every IAM
  permission the pipeline holds;
- `.github/workflows/bench-launch.yml` — proposes a run, prints the exact arm
  list, waits for maintainer approval, launches the box;
- `.github/aws/userdata.sh.tpl` and `.github/aws/run-bench.sh` — what the box
  actually executes, versioned at the SHA the approver saw;
- `.github/workflows/bench-collect.yml` — re-validates the uploads and opens
  the results PR.

You cannot press our launch button — the approval environment and the AWS
account are ours — but nothing about the environment is privileged: the same
instance type, volume, AMI and scripts are available to any AWS account, and
`infra/README.md` is the complete standing-up procedure. A full sweep costs
roughly the instance-hours it takes at ~$1.3/hour, bounded by a 36-hour TTL.
