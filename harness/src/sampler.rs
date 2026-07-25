//! Framework-neutral resource measurement for the cross-framework comparison.
//!
//! The comparison publishes numbers about other people's software, so nothing
//! here may depend on the thing being measured. Every quantity this module
//! produces is read from **outside** the framework under test — from its cgroup —
//! and is therefore obtained identically whether that framework is etl-rs, a
//! JVM, a Go binary, or ClickHouse consuming a topic by itself. No `etl_*`
//! metric family, and no competitor's own instrumentation, feeds a published
//! figure.
//!
//! The framework under test always runs in a container, including etl-rs: an
//! in-process host run would get every core on the box and make the resource
//! envelope meaningless.
//!
//! ## Why a sidecar rather than `docker stats`
//!
//! `docker stats` reports a CPU *percentage* computed over an interval it
//! chooses, with no cumulative microsecond counter, and its memory column folds
//! in page cache. Neither supports a defensible CPU-per-record figure. Reading
//! cgroup v2 directly gives monotonic `usage_usec` and a page-cache-free `anon`,
//! plus `nr_throttled`/`throttled_usec`, which answer "why was it X and not 2X?"
//! with evidence instead of inference.
//!
//! The sidecar is necessary because on Docker Desktop for macOS the cgroup
//! filesystem lives inside the Linux VM and cannot be read from the host at all.
//!
//! ## Two lessons paid for during bring-up, both encoded below
//!
//! * **`memory.peak`'s reset is scoped to the file descriptor.** Writing to it
//!   resets the value only for subsequent reads through that *same* fd; a fresh
//!   open still returns the cgroup's lifetime peak. The sampler holds the fd, so
//!   its peak is a true windowed peak. The driver therefore starts the sampler at
//!   the detected steady-state boundary, which makes the sampling window the
//!   measurement window with no signalling between the two.
//! * **Killing the `docker` CLI does not stop the container.** A `timeout` on
//!   `docker run` detaches the client and leaves the container alive holding the
//!   stdout pipe open. Every container started here is therefore named and
//!   removed by name, never signalled.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::docker::{NETWORK, docker, docker_try};

/// The sampler container's name. Fixed, so an orphan from an interrupted run is
/// always cleaned up by the next one rather than accumulating.
const SAMPLER_CONTAINER: &str = "spate-bench-sampler";

/// Image used for the sampler. Chosen only because it has a Python interpreter;
/// nothing about it is under measurement.
const SAMPLER_IMAGE: &str = "python:3.12-alpine";

/// The sampler program, embedded at compile time and fed to the container on
/// stdin (`python3 -`). Passing it on stdin rather than mounting it keeps the
/// harness free of any bind mount, which on macOS would cross VirtioFS.
const SAMPLER_SRC: &str = include_str!("../../workload/sampler/sample.py");

/// Resolve a container's full 64-hex id from its name.
///
/// The cgroup directory is named after the full id, not the short form.
#[must_use]
pub fn container_id(name: &str) -> String {
    let id = docker(&["inspect", "-f", "{{.Id}}", name]);
    assert_eq!(
        id.len(),
        64,
        "expected a 64-hex container id for {name}, got {id:?}"
    );
    id
}

/// One sample row from the cgroup sampler. `-1` means the field was unreadable
/// at that instant, which is preserved rather than zeroed: a zero would read as
/// "idle" where the truth is "unknown".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Wall-clock milliseconds since the epoch, taken inside the sampler.
    pub t_ms: u64,
    /// Cumulative CPU time charged to the cgroup, microseconds.
    pub usage_usec: i64,
    /// Cumulative user-mode CPU time, microseconds.
    pub user_usec: i64,
    /// Cumulative kernel-mode CPU time, microseconds.
    pub system_usec: i64,
    /// Number of CFS periods in which the cgroup was throttled.
    pub nr_throttled: i64,
    /// Cumulative time the cgroup spent throttled, microseconds.
    pub throttled_usec: i64,
    /// Current charged memory, including page cache.
    pub mem_current: i64,
    /// Peak charged memory **since the sampler started**, via the held fd.
    pub mem_peak: i64,
    /// Anonymous memory — the page-cache-free figure the comparison headlines.
    pub anon: i64,
    /// Page-cache memory charged to the cgroup.
    pub file: i64,
    /// Kernel slab memory.
    pub slab: i64,
    /// Kernel stack memory.
    pub kernel_stack: i64,
    /// Socket buffer memory.
    pub sock: i64,
}

impl Sample {
    fn parse(line: &str) -> Option<Self> {
        let f: Vec<i64> = line
            .split(',')
            .map(|s| s.trim().parse().ok())
            .collect::<Option<_>>()?;
        if f.len() != 13 {
            return None;
        }
        Some(Self {
            t_ms: u64::try_from(f[0]).ok()?,
            usage_usec: f[1],
            user_usec: f[2],
            system_usec: f[3],
            nr_throttled: f[4],
            throttled_usec: f[5],
            mem_current: f[6],
            mem_peak: f[7],
            anon: f[8],
            file: f[9],
            slab: f[10],
            kernel_stack: f[11],
            sock: f[12],
        })
    }
}

/// A running cgroup sampler for one container.
#[derive(Debug)]
pub struct Sampler {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
    started: Instant,
    name: String,
}

impl Sampler {
    /// Start sampling `target` at `interval_s`.
    ///
    /// Call this at the point steady state is detected, not at container start:
    /// the sampler resets `memory.peak` on its own held fd at startup, so the
    /// sampling window *is* the measurement window.
    ///
    /// # Panics
    /// If the sampler container cannot be started, or its stdin/stdout cannot be
    /// captured — a silent measurement failure would be worse than a loud one.
    #[must_use]
    pub fn start(target: &str, interval_s: f64) -> Self {
        Self::start_named(target, interval_s, SAMPLER_CONTAINER)
    }

    /// Like [`start`](Self::start) but with an explicit sampler container name.
    ///
    /// Needed because an arm can be several containers — a Flink arm is a
    /// JobManager plus a TaskManager — and each needs its own sampler. The fixed
    /// name would make the second sampler evict the first.
    ///
    /// # Panics
    /// As [`start`](Self::start).
    #[must_use]
    pub fn start_named(target: &str, interval_s: f64, sampler_name: &str) -> Self {
        let id = container_id(target);
        let cgroup = format!("/cg/docker/{id}");
        // An orphan from an interrupted run would hold the name.
        let _ = docker_try(&["rm", "-f", sampler_name]);
        let sampler_container = sampler_name.to_owned();

        let interval = interval_s.to_string();
        let mut child = Command::new("docker")
            .args([
                "run",
                "--rm",
                "-i",
                "--name",
                &sampler_container,
                "--network",
                NETWORK,
                // The sampler must see the VM's cgroup tree, not its own
                // namespaced view, or the target's cgroup is invisible to it.
                "--cgroupns=host",
                // rw, because resetting `memory.peak` is a write.
                "-v",
                "/sys/fs/cgroup:/cg:rw",
                SAMPLER_IMAGE,
                "python3",
                "-",
                &cgroup,
                &interval,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn cgroup sampler");

        child
            .stdin
            .take()
            .expect("sampler stdin")
            .write_all(SAMPLER_SRC.as_bytes())
            .expect("feed sampler program");

        let stdout = child.stdout.take().expect("sampler stdout");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                sink.lock().expect("sampler line lock").push(line);
            }
        });

        Self {
            child,
            lines,
            started: Instant::now(),
            name: sampler_container,
        }
    }

    /// Stop the sampler and return everything it collected.
    ///
    /// Stops by removing the container: killing the `docker` CLI leaves the
    /// container running and holding the pipe open.
    #[must_use]
    pub fn stop(mut self) -> Samples {
        let _ = docker_try(&["rm", "-f", &self.name]);
        let _ = self.child.wait();
        let elapsed = self.started.elapsed().as_secs_f64();
        let collected = self.lines.lock().expect("sampler line lock").clone();

        let mut meta = String::new();
        let mut rows = Vec::new();
        for line in &collected {
            if let Some(rest) = line.strip_prefix('#') {
                meta = rest.trim().to_owned();
            } else if let Some(s) = Sample::parse(line) {
                rows.push(s);
            }
        }
        Samples {
            meta,
            rows,
            wall_s: elapsed,
        }
    }
}

/// Everything one sampler run collected.
#[derive(Clone, Debug)]
pub struct Samples {
    /// The sampler's header line: cgroup path, `cpu.max`, `memory.max`.
    /// Recorded so a published arm can prove the envelope was actually applied
    /// rather than merely requested.
    pub meta: String,
    /// The sample series, in order.
    pub rows: Vec<Sample>,
    /// Wall-clock seconds the sampler was alive, from the driver's clock.
    pub wall_s: f64,
}

impl Samples {
    /// Summarise the series into the cost figures the comparison publishes.
    ///
    /// Returns `None` when fewer than two samples landed: a single sample gives
    /// no CPU delta, and inventing one from a lifetime counter would silently
    /// charge the framework for its own startup.
    #[must_use]
    pub fn summarise(&self) -> Option<SutCost> {
        let (first, last) = (self.rows.first()?, self.rows.last()?);
        if self.rows.len() < 2 {
            return None;
        }
        let window_s = (last.t_ms.saturating_sub(first.t_ms)) as f64 / 1000.0;
        if window_s <= 0.0 {
            return None;
        }
        let delta = |f: fn(&Sample) -> i64| (f(last) - f(first)).max(0) as f64;
        let cpu_us = delta(|s| s.usage_usec);
        Some(SutCost {
            window_s,
            cpu_us,
            user_us: delta(|s| s.user_usec),
            system_us: delta(|s| s.system_usec),
            // Mean cores occupied over the window. Directly comparable to the
            // container's `--cpus` cap, so a value at the cap says the arm is
            // CPU-bound without needing the throttle counters to say it.
            cores_used: cpu_us / (window_s * 1_000_000.0),
            throttled_us: delta(|s| s.throttled_usec),
            nr_throttled: delta(|s| s.nr_throttled),
            // The headline footprint: page-cache-free, so a framework is not
            // charged for the kernel caching its own input.
            peak_anon_bytes: self.rows.iter().map(|s| s.anon).max().unwrap_or(-1) as f64,
            // Windowed, via the sampler's held fd — not a lifetime peak.
            peak_charged_bytes: last.mem_peak as f64,
            peak_current_bytes: self.rows.iter().map(|s| s.mem_current).max().unwrap_or(-1) as f64,
            samples: self.rows.len(),
        })
    }
}

/// Resource cost of one framework arm over one measurement window.
#[derive(Clone, Copy, Debug)]
pub struct SutCost {
    /// Length of the window, from the sampler's own timestamps.
    pub window_s: f64,
    /// CPU microseconds consumed in the window.
    pub cpu_us: f64,
    /// User-mode share of `cpu_us`.
    pub user_us: f64,
    /// Kernel-mode share of `cpu_us`.
    pub system_us: f64,
    /// Mean cores occupied (`cpu_us / window`). Compare against the `--cpus` cap.
    pub cores_used: f64,
    /// Microseconds spent throttled by the CPU cap.
    pub throttled_us: f64,
    /// CFS periods in which throttling occurred.
    pub nr_throttled: f64,
    /// Peak anonymous memory — the published footprint figure.
    pub peak_anon_bytes: f64,
    /// Peak charged memory over the window (includes page cache).
    pub peak_charged_bytes: f64,
    /// Peak `memory.current` seen in the series (includes page cache).
    pub peak_current_bytes: f64,
    /// Number of samples the summary rests on.
    pub samples: usize,
}

impl SutCost {
    /// Sum the cost of several containers into one arm's cost.
    ///
    /// This is what makes a multi-container arm measurable against a
    /// single-container one. A Flink arm is a JobManager plus a TaskManager, and
    /// the resource envelope is defined over the arm as a whole — so CPU,
    /// footprint and throttling **add**, while the window is the longest of them
    /// (they are sampled concurrently, so it is one shared window, not a sum).
    ///
    /// Reporting only the data-plane container would quietly under-report a
    /// framework that needs a control plane, and hand us a win we had not earned.
    #[must_use]
    pub fn sum(parts: &[Self]) -> Option<Self> {
        if parts.is_empty() {
            return None;
        }
        let add = |f: fn(&Self) -> f64| parts.iter().map(f).sum::<f64>();
        Some(Self {
            window_s: parts.iter().map(|p| p.window_s).fold(0.0_f64, f64::max),
            cpu_us: add(|p| p.cpu_us),
            user_us: add(|p| p.user_us),
            system_us: add(|p| p.system_us),
            cores_used: add(|p| p.cores_used),
            throttled_us: add(|p| p.throttled_us),
            nr_throttled: add(|p| p.nr_throttled),
            peak_anon_bytes: add(|p| p.peak_anon_bytes),
            peak_charged_bytes: add(|p| p.peak_charged_bytes),
            peak_current_bytes: add(|p| p.peak_current_bytes),
            samples: parts.iter().map(|p| p.samples).min().unwrap_or(0),
        })
    }

    /// CPU microseconds per row landed — the efficiency metric the comparison
    /// leads with, and the one that is meaningful across languages.
    #[must_use]
    pub fn cpu_us_per_row(&self, rows: f64) -> f64 {
        if rows <= 0.0 {
            f64::NAN
        } else {
            self.cpu_us / rows
        }
    }

    /// Whether the CPU cap was a binding constraint. Reported rather than
    /// inferred: an arm that throttled was cap-bound, which is the honest answer
    /// to "why was it X and not 2X?".
    #[must_use]
    pub fn was_throttled(&self) -> bool {
        self.nr_throttled > 0.0
    }
}

// ---------------------------------------------------------------------------
// Serialising arms
// ---------------------------------------------------------------------------

/// Path of the cross-arm advisory lock. A fixed, boring path so that a shell
/// script or a Java harness can take the same lock with `set -o noclobber`; the
/// lock is not Rust-specific and must not be.
pub const LOCK_PATH: &str = "/tmp/spate-bench-comparison.lock";

/// Exclusive right to run one arm against the shared infrastructure.
///
/// This exists because its absence already cost a measurement. Two arms ran
/// concurrently against the same Redpanda and ClickHouse, and one driver
/// `TRUNCATE`d the shared target table five times inside the other's run — so the
/// second arm's throughput numbers were unusable and its correctness had to be
/// re-verified on separate tables. Nothing about that failure was visible while it
/// was happening, which is exactly why it needs a lock rather than a convention.
///
/// Acquisition is atomic (`create_new`, i.e. `O_EXCL`). The holder writes its pid
/// and a description so a refusal can say who is running and since when.
#[derive(Debug)]
pub struct ArmLock {
    path: std::path::PathBuf,
}

impl ArmLock {
    /// Take the lock, or return the current holder's description.
    ///
    /// A lock whose recorded pid is no longer alive is treated as stale and
    /// reclaimed: a crashed run must not block the suite forever. `FORCE_UNLOCK=1`
    /// overrides a live holder, which is a deliberate foot-gun for when a holder
    /// is wedged.
    pub fn acquire(description: &str) -> Result<Self, String> {
        let path = std::path::PathBuf::from(LOCK_PATH);
        if std::env::var("FORCE_UNLOCK").is_ok_and(|v| v == "1") {
            let _ = std::fs::remove_file(&path);
        }
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = std::io::Write::write_all(
                        &mut f,
                        format!("{} {description}\n", std::process::id()).as_bytes(),
                    );
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let held = std::fs::read_to_string(&path).unwrap_or_default();
                    let holder_pid = held
                        .split_whitespace()
                        .next()
                        .and_then(|p| p.parse::<u32>().ok());
                    // A dead holder is stale; reclaim and retry once.
                    if holder_pid.is_some_and(|pid| !pid_alive(pid)) {
                        eprintln!("reclaiming stale arm lock from dead pid {holder_pid:?}");
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    return Err(format!(
                        "another arm holds {LOCK_PATH}: {}. Arms MUST run one at a \
                         time — they share one Redpanda and one ClickHouse, and the \
                         driver truncates the target table. Wait, or pass \
                         FORCE_UNLOCK=1 if that holder is wedged.",
                        held.trim()
                    ));
                }
                Err(e) => return Err(format!("could not take {LOCK_PATH}: {e}")),
            }
        }
    }
}

impl Drop for ArmLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Whether a pid is alive, via `kill -0`. Shelling out avoids a `libc`
/// dependency for one probe, and this is not on any hot path.
fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .is_ok_and(|o| o.status.success())
}

// ---------------------------------------------------------------------------
// The framework under test
// ---------------------------------------------------------------------------

/// How to launch one framework arm.
#[derive(Clone, Debug)]
pub struct SutSpec {
    /// Container name; removed before and after the run.
    pub name: String,
    /// Image to run.
    pub image: String,
    /// `--cpus` value. The whole budget, control plane included.
    pub cpus: String,
    /// `--memory` value; `--memory-swap` is set to the same, so memory pressure
    /// surfaces instead of moving into swap where we are not measuring.
    pub memory: String,
    /// Environment passed to the arm.
    pub env: Vec<(String, String)>,
    /// Command arguments after the image. Flink's image dispatches on these
    /// (`standalone-job`, `taskmanager`); a single-container arm leaves it empty.
    pub args: Vec<String>,
    /// `-v` arguments. Named volumes only — never a host bind mount, which on
    /// macOS crosses VirtioFS. Flink needs the same checkpoint volume visible in
    /// both its containers.
    pub volumes: Vec<String>,
}

/// Start a framework arm, replacing any container of the same name.
///
/// # Panics
/// If the image is missing or `docker run` is rejected — a benchmark that
/// silently measured nothing would be worse than a loud failure.
pub fn start_sut(spec: &SutSpec) {
    assert!(
        docker_try(&["image", "inspect", &spec.image]).is_ok(),
        "image {} is not built. Build the arm's Dockerfile first.",
        spec.image
    );
    let _ = docker_try(&["rm", "-f", &spec.name]);

    let cpus = format!("--cpus={}", spec.cpus);
    let mem = format!("--memory={}", spec.memory);
    // Equal to `--memory`: with swap left at its default the arm would silently
    // swap instead of feeling its cap, and the footprint figure would record a
    // limit being respected while the real cost moved somewhere unmeasured.
    let swap = format!("--memory-swap={}", spec.memory);
    let mut args: Vec<&str> = vec![
        "run",
        "-d",
        "--name",
        &spec.name,
        "--network",
        NETWORK,
        &cpus,
        &mem,
        &swap,
    ];
    let env_args: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    for e in &env_args {
        args.push("-e");
        args.push(e);
    }
    for v in &spec.volumes {
        args.push("-v");
        args.push(v);
    }
    args.push(&spec.image);
    for a in &spec.args {
        args.push(a);
    }
    docker(&args);
}

/// Start every container of a multi-container arm, in order.
///
/// Order matters for Flink: the JobManager must be up before a TaskManager can
/// register with it.
pub fn start_arm(specs: &[SutSpec]) {
    for spec in specs {
        start_sut(spec);
    }
}

/// Sample every container of an arm concurrently, one sampler each.
///
/// Returns the per-container costs in the order given, so the driver can publish
/// both the arm total and each part — the contract promises a TaskManager-only
/// figure alongside the total, so that nobody can claim we taxed Flink for its
/// JobManager, and so the control plane's real cost is a published fact rather
/// than an allocation we chose to charge.
#[must_use]
pub fn sample_arm(names: &[String], interval_s: f64) -> Vec<Sampler> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| Sampler::start_named(n, interval_s, &format!("{SAMPLER_CONTAINER}-{i}")))
        .collect()
}

/// Stop and remove a framework arm, returning its last log lines for diagnosis.
pub fn stop_sut(name: &str) -> String {
    let logs = docker_try(&["logs", "--tail", "40", name]).unwrap_or_default();
    let _ = docker_try(&["rm", "-f", name]);
    logs
}

/// Whether the arm container is still running.
#[must_use]
pub fn sut_alive(name: &str) -> bool {
    docker_try(&["inspect", "-f", "{{.State.Running}}", name]).is_ok_and(|s| s == "true")
}

// ---------------------------------------------------------------------------
// Steady state
// ---------------------------------------------------------------------------

/// Thresholds for [`detect_steady_state`].
#[derive(Clone, Copy, Debug)]
pub struct SteadyStateConfig {
    /// How many consecutive rate samples must satisfy the criteria.
    pub window: usize,
    /// Maximum coefficient of variation (stddev / mean) across the window.
    pub cv_max: f64,
    /// Maximum fractional drift across the window, from a least-squares slope:
    /// `|slope| * window_duration / mean`.
    pub slope_max: f64,
    /// Minimum mean rate. Guards against declaring a **stall** to be steady
    /// state — a pipeline producing nothing has a coefficient of variation and a
    /// slope of exactly zero, and would otherwise look like the most stable
    /// system ever measured.
    pub min_rate: f64,
}

impl Default for SteadyStateConfig {
    fn default() -> Self {
        Self {
            window: 10,
            cv_max: 0.10,
            slope_max: 0.05,
            min_rate: 1000.0,
        }
    }
}

/// Find the first index at which the trailing `window` of `(t_seconds, rate)`
/// samples looks like steady state, or `None` if it never does.
///
/// **This is a plateau detector, not a changepoint algorithm.** The benchmarking
/// literature recommends changepoint analysis (PELT, CUSUM) for exactly this
/// job, and calling this that would be an overclaim. What it does is require a
/// window to be simultaneously *flat* (low coefficient of variation), *level*
/// (small least-squares slope relative to the mean) and *non-trivial* (above
/// `min_rate`). It is deliberately conservative: it fires late rather than
/// early, because measuring a still-warming JVM is the failure it exists to
/// prevent.
///
/// The reason a fixed warmup timer is not used instead: JVM warmup is not
/// monotonic — Barrett et al., *Virtual Machine Warmup Blows Hot and Cold* —
/// so "wait 60 seconds" can land in the middle of a recompilation and quietly
/// bias a JVM arm against itself.
#[must_use]
pub fn detect_steady_state(samples: &[(f64, f64)], cfg: SteadyStateConfig) -> Option<usize> {
    if cfg.window < 2 {
        return None;
    }
    for end in cfg.window..=samples.len() {
        let w = &samples[end - cfg.window..end];
        let n = w.len() as f64;
        let mean = w.iter().map(|(_, r)| r).sum::<f64>() / n;
        if mean < cfg.min_rate {
            continue;
        }
        let var = w.iter().map(|(_, r)| (r - mean).powi(2)).sum::<f64>() / n;
        let cv = var.sqrt() / mean;
        if cv > cfg.cv_max {
            continue;
        }
        // Least-squares slope of rate against time.
        let t_mean = w.iter().map(|(t, _)| t).sum::<f64>() / n;
        let sxy: f64 = w.iter().map(|(t, r)| (t - t_mean) * (r - mean)).sum();
        let sxx: f64 = w.iter().map(|(t, _)| (t - t_mean).powi(2)).sum();
        if sxx <= 0.0 {
            continue;
        }
        let slope = sxy / sxx;
        let span = w.last().expect("non-empty window").0 - w.first().expect("non-empty window").0;
        let drift = (slope * span / mean).abs();
        if drift <= cfg.slope_max {
            return Some(end - 1);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(rates: &[f64]) -> Vec<(f64, f64)> {
        rates
            .iter()
            .enumerate()
            .map(|(i, r)| (i as f64, *r))
            .collect()
    }

    /// A ramp followed by a plateau must be detected inside the plateau, never
    /// during the ramp — that is the entire point.
    #[test]
    fn detects_steady_state_after_a_ramp() {
        let mut rates: Vec<f64> = (1..=15).map(|i| f64::from(i) * 60_000.0).collect();
        rates.extend(std::iter::repeat_n(900_000.0, 15));
        let at = detect_steady_state(&series(&rates), SteadyStateConfig::default())
            .expect("plateau detected");
        assert!(
            at >= 15,
            "detected at index {at}, which is still inside the ramp"
        );
    }

    /// A monotonic ramp that never flattens must never be declared steady.
    #[test]
    fn never_detects_on_a_pure_ramp() {
        let rates: Vec<f64> = (1..=40).map(|i| f64::from(i) * 50_000.0).collect();
        assert!(detect_steady_state(&series(&rates), SteadyStateConfig::default()).is_none());
    }

    /// The failure this guard exists for: a stalled pipeline is perfectly flat
    /// and perfectly level, and without a floor it would look like the most
    /// stable system ever measured.
    #[test]
    fn a_stall_is_not_steady_state() {
        let rates = vec![0.0; 40];
        assert!(detect_steady_state(&series(&rates), SteadyStateConfig::default()).is_none());
        // Same shape well above the floor *is* steady state, which shows the
        // rejection came from `min_rate` and not from the flatness test.
        let alive = vec![900_000.0; 40];
        assert!(detect_steady_state(&series(&alive), SteadyStateConfig::default()).is_some());
    }

    /// Noise inside the tolerance is steady; noise outside it is not.
    #[test]
    fn tolerates_bounded_noise_but_not_unbounded() {
        let calm: Vec<f64> = (0..30)
            .map(|i| 900_000.0 + if i % 2 == 0 { 9_000.0 } else { -9_000.0 })
            .collect();
        assert!(detect_steady_state(&series(&calm), SteadyStateConfig::default()).is_some());

        let wild: Vec<f64> = (0..30)
            .map(|i| 900_000.0 + if i % 2 == 0 { 450_000.0 } else { -450_000.0 })
            .collect();
        assert!(detect_steady_state(&series(&wild), SteadyStateConfig::default()).is_none());
    }

    #[test]
    fn a_window_shorter_than_two_is_rejected() {
        let cfg = SteadyStateConfig {
            window: 1,
            ..SteadyStateConfig::default()
        };
        assert!(detect_steady_state(&series(&[900_000.0; 10]), cfg).is_none());
    }

    #[test]
    fn parses_a_sampler_row() {
        let row = "1784979298378,129007508,129003510,3998,660,602643,659456,659456,\
                   135168,0,399080,16384,0";
        let s = Sample::parse(row).expect("row parses");
        assert_eq!(s.t_ms, 1_784_979_298_378);
        assert_eq!(s.usage_usec, 129_007_508);
        assert_eq!(s.anon, 135_168);
        assert_eq!(s.sock, 0);
    }

    #[test]
    fn rejects_a_row_of_the_wrong_width() {
        assert!(Sample::parse("1,2,3").is_none());
        assert!(Sample::parse("not,a,row").is_none());
    }

    /// A single sample cannot yield a CPU delta, and treating its lifetime
    /// counter as the window's usage would charge the framework for its startup.
    #[test]
    fn a_single_sample_summarises_to_nothing() {
        let one = Sample::parse("1000,5,5,0,0,0,10,10,10,0,0,0,0").expect("row parses");
        let s = Samples {
            meta: String::new(),
            rows: vec![one],
            wall_s: 1.0,
        };
        assert!(s.summarise().is_none());
    }

    #[test]
    fn summarises_cpu_as_a_delta_over_the_window() {
        let rows = vec![
            Sample::parse("1000,1000000,900000,100000,0,0,100,100,80,20,0,0,0")
                .expect("row parses"),
            // +2 CPU-seconds over 1 wall-second: two cores' worth.
            Sample::parse("2000,3000000,2700000,300000,3,500,300,400,250,50,0,0,0")
                .expect("row parses"),
        ];
        let cost = Samples {
            meta: String::new(),
            rows,
            wall_s: 1.0,
        }
        .summarise()
        .expect("two samples summarise");

        assert!((cost.window_s - 1.0).abs() < 1e-9);
        assert!((cost.cpu_us - 2_000_000.0).abs() < 1e-9);
        assert!((cost.cores_used - 2.0).abs() < 1e-9);
        // Peak anon is the max over the series, not the last value.
        assert!((cost.peak_anon_bytes - 250.0).abs() < 1e-9);
        // Charged peak comes from the held fd (the last row), which is what
        // makes it a windowed rather than lifetime figure.
        assert!((cost.peak_charged_bytes - 400.0).abs() < 1e-9);
        assert!(cost.was_throttled());
        assert!((cost.cpu_us_per_row(1_000_000.0) - 2.0).abs() < 1e-9);
    }
}
