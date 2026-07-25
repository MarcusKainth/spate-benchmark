//! Bringing up the shared infrastructure, from the environment profile.
//!
//! There is exactly **one** path to a running broker and ClickHouse, and it
//! takes its caps from the environment profile rather than from ambient
//! variables. That is not tidiness: the previous harness had the caps come from
//! environment variables, and a runner script set one pair while the driver's
//! defaults declared another and the written methodology stated a third — with
//! every recorded number silent about which had been in force. Two components
//! cannot disagree if only one of them is allowed to speak.
//!
//! Having applied the caps, this module **reads them back out of the running
//! containers' cgroups and asserts they match**. A mismatch fails the run. The
//! previous harness warned and carried on, which is how the disagreement above
//! survived long enough to reach published records.
//!
//! Infrastructure is **recreated by default**, not reused. The framework
//! repository's equivalent reuses a healthy container so repeated runs are
//! cheap, which is right for a development rig and wrong here: silently reusing
//! a warm ClickHouse of the wrong version would be a published-number defect,
//! not an inconvenience. Reuse is opt-in and sets a flag on every record it
//! produces.

use std::time::{Duration, Instant};

use crate::docker::{self, NETWORK};
use crate::environment::Environment;
use crate::report::{Flag, Infra};

/// Where the running infrastructure can be reached, from the host and from
/// inside the bench network.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Host-side Kafka bootstrap.
    pub bootstrap: String,
    /// Bootstrap as a container on [`NETWORK`] must dial it.
    pub bootstrap_internal: String,
    /// Host-side registry.
    pub registry_host: String,
    /// Host-side registry port.
    pub registry_port: u16,
    /// Registry URL for a container on [`NETWORK`].
    pub registry_internal: String,
    /// Host-side ClickHouse.
    pub ch_host: String,
    /// Host-side ClickHouse HTTP port.
    pub ch_port: u16,
    /// ClickHouse user.
    pub ch_user: String,
    /// ClickHouse password.
    pub ch_password: String,
    /// ClickHouse URL for a container on [`NETWORK`].
    pub ch_internal: String,
}

const BROKER: &str = "spate-bench-redpanda";
const CLICKHOUSE: &str = "spate-bench-clickhouse";

/// Brings up the infrastructure this environment declares.
///
/// # Errors
///
/// If a container does not become reachable, or if the caps read back from a
/// running container disagree with the profile.
///
/// # Panics
///
/// If the docker CLI itself fails.
pub fn bring_up(env: &Environment, reuse: bool) -> Result<(Endpoints, Infra, Vec<Flag>), String> {
    let mut flags = Vec::new();
    docker::ensure_network();

    let b = &env.spec.infra.broker;
    let c = &env.spec.infra.clickhouse;

    if reuse && running(BROKER) && running(CLICKHOUSE) {
        flags.push(Flag::ReusedInfra);
        eprintln!(
            "reusing the running infrastructure. Its caps are still read back and \
             asserted below, so a container started under a different envelope \
             will fail the run rather than quietly produce a number."
        );
        docker::attach_to_network(BROKER);
        docker::attach_to_network(CLICKHOUSE);
    } else {
        start_broker(b)?;
        start_clickhouse(c)?;
    }

    let endpoints = Endpoints {
        bootstrap: "localhost:9092".to_owned(),
        bootstrap_internal: format!("{BROKER}:29092"),
        registry_host: "localhost".to_owned(),
        registry_port: 18081,
        registry_internal: format!("http://{BROKER}:8081"),
        ch_host: "localhost".to_owned(),
        ch_port: 18123,
        ch_user: "default".to_owned(),
        ch_password: "bench".to_owned(),
        ch_internal: format!("http://{CLICKHOUSE}:8123"),
    };

    wait_for_registry(&endpoints)?;

    // Read back and assert. Declared-versus-applied is checked here, once, for
    // both containers — the only place in the harness that gets to decide the
    // infrastructure was what the profile said it was.
    let (broker_cpus, broker_memory) = cgroup_caps(BROKER)?;
    assert_cap(BROKER, "cpus", &b.cpus, &broker_cpus)?;
    assert_cap(BROKER, "memory", &b.memory, &broker_memory)?;

    let (ch_cpus, ch_memory) = cgroup_caps(CLICKHOUSE)?;
    assert_cap(CLICKHOUSE, "cpus", &c.cpus, &ch_cpus)?;
    assert_cap(CLICKHOUSE, "memory", &c.memory, &ch_memory)?;

    let ceiling = env.ceiling().map_or(0, |c| c.consume_msgs_per_s);
    if ceiling == 0 {
        flags.push(Flag::HeadroomUnproven);
    }

    let infra = Infra {
        digest: env.infra_digest(),
        broker: b.kind.clone(),
        broker_version: broker_version(),
        broker_image_digest: image_digest(BROKER),
        broker_cpus,
        broker_memory,
        clickhouse_version: clickhouse_version(&endpoints),
        clickhouse_image_digest: image_digest(CLICKHOUSE),
        clickhouse_cpus: ch_cpus,
        clickhouse_memory: ch_memory,
        partitions: env.spec.infra.partitions,
        registry: b.registry.clone(),
        ceiling_msgs_per_s: ceiling,
    };

    Ok((endpoints, infra, flags))
}

fn running(name: &str) -> bool {
    docker::docker_try(&["inspect", "-f", "{{.State.Running}}", name]).is_ok_and(|s| s == "true")
}

fn start_broker(b: &crate::environment::Broker) -> Result<(), String> {
    let _ = docker::docker_try(&["rm", "-f", BROKER]);
    eprintln!("starting {BROKER} ({}, --cpus={}) ...", b.image, b.cpus);

    let cpus = format!("--cpus={}", b.cpus);
    let mem = format!("--memory={}", b.memory);
    let swap = format!("--memory-swap={}", b.memory);
    // Redpanda's own memory budget, separate from the cgroup limit: left below
    // the container cap so the process reserves less than the kernel allows and
    // an overshoot surfaces as Redpanda backpressure rather than an OOM kill.
    let rp_memory = "4G";
    let advertise = format!("EXTERNAL://localhost:9092,INTERNAL://{BROKER}:29092");
    let smp = b.cpus.clone();

    let args: Vec<&str> = vec![
        "run",
        "-d",
        "--name",
        BROKER,
        "--network",
        NETWORK,
        "-p",
        "9092:9092",
        // Redpanda's built-in, Confluent-compatible Schema Registry, published
        // for the host-side prefill. Using this rather than a separate Confluent
        // container keeps a second JVM out of the measurement environment and
        // returns a CPU and a GiB to the infra budget.
        "-p",
        "18081:8081",
        &cpus,
        &mem,
        &swap,
        &b.image,
        "redpanda",
        "start",
        "--node-id",
        "0",
        "--check=false",
        // Load-bearing rather than cosmetic: by default Redpanda busy-polls one
        // core per shard, which on a co-located host burns CPU the system under
        // test needs and turns the measurement into a scheduling contest.
        "--overprovisioned",
        "--kafka-addr",
        "EXTERNAL://0.0.0.0:9092,INTERNAL://0.0.0.0:29092",
        "--advertise-kafka-addr",
        &advertise,
        "--smp",
        &smp,
        "--memory",
        rp_memory,
        "--reserve-memory",
        "0M",
    ];
    docker::docker(&args);

    let deadline = Instant::now() + Duration::from_secs(90);
    while !tcp_open("localhost", 9092) {
        if Instant::now() >= deadline {
            return Err(format!("{BROKER} did not become reachable"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // The port opens before the broker is ready to serve metadata.
    std::thread::sleep(Duration::from_secs(2));
    Ok(())
}

fn start_clickhouse(c: &crate::environment::ClickHouse) -> Result<(), String> {
    let _ = docker::docker_try(&["rm", "-f", CLICKHOUSE]);
    eprintln!("starting {CLICKHOUSE} ({}, --cpus={}) ...", c.image, c.cpus);

    let cpus = format!("--cpus={}", c.cpus);
    let mem = format!("--memory={}", c.memory);
    let swap = format!("--memory-swap={}", c.memory);
    let args: Vec<&str> = vec![
        "run",
        "-d",
        "--name",
        CLICKHOUSE,
        "--network",
        NETWORK,
        "-p",
        "18123:8123",
        "-p",
        "19000:9000",
        "-e",
        "CLICKHOUSE_PASSWORD=bench",
        "--ulimit",
        "nofile=262144:262144",
        // Deliberately no volume mount. ClickHouse writes to the container's own
        // writable layer, which is already VM-local; the VirtioFS penalty on
        // macOS applies to bind mounts from the host filesystem, and the rule
        // for this harness is never to bind-mount a measured path.
        &cpus,
        &mem,
        &swap,
        &c.image,
    ];
    docker::docker(&args);

    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        if crate::http::get("localhost", 18123, "/ping").is_ok_and(|b| b.contains("Ok")) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("{CLICKHOUSE} did not become reachable"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn wait_for_registry(e: &Endpoints) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        // `/subjects` on an empty registry returns `[]`, which is a positive
        // answer — probe for a well-formed response, not for non-emptiness.
        if crate::http::get(&e.registry_host, e.registry_port, "/subjects")
            .is_ok_and(|b| b.contains('['))
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("the schema registry did not answer".to_owned());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The cgroup v2 caps a container is actually running under, as
/// `(cpu.max, memory.max)` verbatim.
///
/// Read from inside the container rather than from `docker inspect`: inspect
/// reports what Docker was *asked* for, and the question here is what the kernel
/// is *enforcing*. Infrastructure images have a shell, so `docker exec` is
/// available — the systems under test may not, which is why their sampling goes
/// through a sidecar instead.
fn cgroup_caps(container: &str) -> Result<(String, String), String> {
    let cpu = docker::docker_try(&["exec", container, "cat", "/sys/fs/cgroup/cpu.max"])
        .map_err(|e| format!("read cpu.max from {container}: {e}"))?;
    let mem = docker::docker_try(&["exec", container, "cat", "/sys/fs/cgroup/memory.max"])
        .map_err(|e| format!("read memory.max from {container}: {e}"))?;
    Ok((cpu.trim().to_owned(), mem.trim().to_owned()))
}

/// Fails the run when the applied cap disagrees with the declared one.
fn assert_cap(container: &str, what: &str, declared: &str, applied: &str) -> Result<(), String> {
    let ok = match what {
        "cpus" => cpu_max_cores(applied)
            .is_some_and(|c| declared.parse::<f64>().is_ok_and(|d| (c - d).abs() < 0.01)),
        _ => memory_bytes(declared).is_some_and(|d| applied.parse::<u64>().is_ok_and(|a| a == d)),
    };
    if ok {
        return Ok(());
    }
    Err(format!(
        "REFUSED: {container} declares {what}={declared} but is running under \
         {what}={applied}. The envelope in the environment profile is what every \
         published number is described by, so a container running under a \
         different one cannot produce a publishable result. Recreate the \
         infrastructure (drop --reuse-infra) or correct the profile."
    ))
}

/// Cores from a cgroup v2 `cpu.max` line (`"<quota> <period>"`, or `"max …"`).
fn cpu_max_cores(cpu_max: &str) -> Option<f64> {
    let mut it = cpu_max.split_whitespace();
    let quota = it.next()?;
    let period: f64 = it.next()?.parse().ok()?;
    if quota == "max" {
        return None;
    }
    Some(quota.parse::<f64>().ok()? / period)
}

/// Parses `8g`, `512m`, `1024` into bytes.
fn memory_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last()? {
        'g' | 'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        'm' | 'M' => (&s[..s.len() - 1], 1024 * 1024),
        'k' | 'K' => (&s[..s.len() - 1], 1024),
        _ => (s, 1),
    };
    digits.trim().parse::<u64>().ok().map(|n| n * mult)
}

/// The image a container is running, by content id. A tag can be re-pushed; an
/// id cannot.
fn image_digest(container: &str) -> String {
    docker::docker_try(&["inspect", "-f", "{{.Image}}", container]).unwrap_or_default()
}

fn broker_version() -> String {
    docker::docker_try(&["exec", BROKER, "rpk", "version"])
        .map(|s| {
            s.lines()
                .find_map(|l| l.split_whitespace().last().map(str::to_owned))
                .unwrap_or_else(|| s.trim().to_owned())
        })
        .unwrap_or_else(|_| "unknown".to_owned())
}

fn clickhouse_version(e: &Endpoints) -> String {
    docker::clickhouse_sql(
        &e.ch_host,
        e.ch_port,
        &e.ch_user,
        &e.ch_password,
        "SELECT version()",
    )
    .map(|v| v.trim().to_owned())
    .unwrap_or_else(|_| "unknown".to_owned())
}

fn tcp_open(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    (host, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .is_some_and(|addr| {
            std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_max_converts_quota_and_period_to_cores() {
        assert_eq!(cpu_max_cores("800000 100000"), Some(8.0));
        assert_eq!(cpu_max_cores("400000 100000"), Some(4.0));
        // An uncapped container cannot satisfy a declared cap, and returning
        // None is what makes the assertion fail rather than pass vacuously.
        assert_eq!(cpu_max_cores("max 100000"), None);
    }

    #[test]
    fn a_mismatched_cap_is_refused() {
        // The whole point of the module: declared 8, running 4, and the run
        // stops. The previous harness printed a warning here and carried on.
        let e = assert_cap("c", "cpus", "8", "400000 100000").expect_err("must refuse");
        assert!(e.starts_with("REFUSED"), "{e}");
        assert!(assert_cap("c", "cpus", "8", "800000 100000").is_ok());
    }

    #[test]
    fn memory_caps_compare_in_bytes() {
        assert!(assert_cap("c", "memory", "8g", "8589934592").is_ok());
        assert!(assert_cap("c", "memory", "8g", "8589934591").is_err());
        // "max" is not a number, so an uncapped container fails the assertion.
        assert!(assert_cap("c", "memory", "8g", "max").is_err());
    }
}
