//! The Spate arm of the streaming ETL benchmark.
//!
//! Runs the real framework, in a container, under the same cgroup caps every
//! other arm is held to. An in-process host run would get all of the host's
//! cores and invalidate the resource envelope — measuring our own framework
//! outside the constraints we hold competitors to would void the comparison
//! before it started.
//!
//! It reports nothing about itself. `metrics.exporter` is `none`, and every
//! published figure comes from outside: ClickHouse for throughput and latency,
//! cgroup v2 for CPU and memory.

fn main() {
    println!("spate-arm: not yet implemented");
}
