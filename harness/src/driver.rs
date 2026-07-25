//! The measurement protocol.
//!
//! Everything here exists to answer one objection: *you measured your own
//! framework with your own instrumentation*. Nothing a system reports about
//! itself reaches a published number. Throughput is a row count in ClickHouse,
//! CPU and memory are cgroup counters read by a sidecar, and the envelope is
//! read back out of the kernel rather than trusted from the request.
//!
//! The protocol is a sequence of refusals as much as a sequence of measurements.
//! An arm that exits, that produces too few samples, that outruns the
//! infrastructure's proven ceiling, or that loses or corrupts rows, produces a
//! record with a failed status and **no metrics** — never a plausible-looking
//! number. Getting this wrong is not a crash; it is a publishable-looking figure
//! that is quietly false, which is the only outcome this suite genuinely cannot
//! afford.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::corpus::{self, Tier};
use crate::entrant::{Container, Role, Variant};
use crate::environment::{Environment, HEADROOM_LIMIT};
use crate::infra::{self, Endpoints};
use crate::report::{Flag, Infra, Kind, Metric, Report, RunMeta, Status, Sut, Trigger};
use crate::results;
use crate::sampler::{self, ArmLock, SutCost, SutSpec};
use crate::select::Arm;

/// How the arm is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Replay a prefilled topic to exhaustion and time the whole drain.
    ///
    /// The headline throughput measurement, and the only one this host can make
    /// honestly. Under sustained load the generator, the broker, ClickHouse and
    /// the arm together oversubscribe the machine, and the contention shows up
    /// as the arm's number: widening egress concurrency from 2 to 32 appeared to
    /// change throughput not at all, which reads exactly like "egress does not
    /// matter" and was host contention. Measured in drain it gives 3.25M → 4.81M
    /// rows/s.
    ///
    /// A full drain also removes the two things a windowed measurement has to
    /// get right and silently fails at: sizing a window, and detecting steady
    /// state inside it. There is no window — the drain is the measurement.
    Drain,
}

/// Everything `bench run` was asked to do.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Repetitions per arm.
    pub reps: u32,
    /// How the arm is loaded.
    pub mode: Mode,
    /// Environment id.
    pub env_id: String,
    /// What caused the run.
    pub trigger: Trigger,
    /// Print the plan and stop.
    pub dry_run: bool,
    /// Reuse running infrastructure rather than recreating it.
    pub reuse_infra: bool,
    /// Abandon the sweep on the first refusal. Off by default: one bad arm must
    /// not cost a thirty-hour sweep.
    pub fail_fast: bool,
    /// Topic to consume.
    pub topic: String,
    /// Corpus size, in messages.
    pub batches: u64,
}

/// Seconds a drain may take before it is abandoned.
const DRAIN_MAX_S: u64 = 1800;
/// Seconds to wait for the pipeline to settle before gating.
const QUIESCE_MAX_S: u64 = 900;
/// Sampler interval.
const SAMPLE_INTERVAL_S: f64 = 1.0;
/// Batches the correctness gate examines, counted down from the top of the range.
///
/// Bounded because exact-distinct needs a hash set proportional to cardinality,
/// and running it over the full 150M-row corpus asked ClickHouse for 10.45 GiB
/// against a 10.8 GiB limit and was killed — taking a completed, valid
/// measurement down with it.
///
/// The slice is taken from the TOP of the range because that is the part
/// produced during and after the measurement window, and it is still ten million
/// rows: a system that drops, duplicates or mis-transforms does so
/// systematically rather than once. The window is recorded in the record's note,
/// so the gate is visibly a sample rather than silently one.
const GATE_MAX_BATCHES: u64 = 100_000;

/// Prepares infrastructure, schema, target tables and corpus.
///
/// # Errors
///
/// If infrastructure cannot be brought up, or the corpus does not verify.
pub fn prefill(root: &Path, opts: &RunOptions) -> Result<(), String> {
    let env = Environment::load(&root.join("environments"), &opts.env_id)?;
    let (ep, _infra, _flags) = infra::bring_up(&env, opts.reuse_infra)?;

    let schema_id = corpus::register_schema(&ep.registry_host, ep.registry_port);
    eprintln!("registered {} as schema id {schema_id}", corpus::SUBJECT);

    for stmt in corpus::ddl_statements() {
        crate::docker::clickhouse_sql(&ep.ch_host, ep.ch_port, &ep.ch_user, &ep.ch_password, &stmt)
            .map_err(|e| format!("DDL failed: {e}"))?;
    }
    eprintln!("target tables applied");

    let report = corpus::prefill(
        &ep.bootstrap,
        &opts.topic,
        env.spec.infra.partitions,
        opts.batches,
        schema_id,
    );
    eprintln!("prefill: {} messages on {}", report.batches, opts.topic);

    // Re-read the bytes actually sitting in Kafka and re-derive every field from
    // `batch_id`. The round-trip unit tests only prove the encoder and decoder
    // agree with each other; this proves the wire matches the contract, which is
    // what every competitor arm actually reads.
    let verified = corpus::verify_corpus(&ep.bootstrap, &opts.topic, schema_id, 64);
    eprintln!("verified {verified} messages against the contract");

    for tier in [Tier::A, Tier::B] {
        let e = corpus::expected(opts.batches, tier);
        eprintln!("expected tier {}: {} rows", tier.name(), e.rows);
    }
    Ok(())
}

/// Runs the selected arms and appends one record per repetition.
///
/// # Errors
///
/// If setup fails. Individual arm refusals are recorded and reported, and do not
/// stop the sweep unless `fail_fast` is set.
pub fn run(root: &Path, arms: &[Arm<'_>], opts: &RunOptions) -> Result<(), String> {
    let env = Environment::load(&root.join("environments"), &opts.env_id)?;

    // The plan, printed before anything is spent. A full sweep costs hours, so
    // "which arms will this actually run?" has to be answerable in advance
    // rather than inferred afterwards from what appeared.
    eprintln!(
        "plan: {} arm(s) x {} rep(s) = {} run(s), interleaved, on {} [{}]",
        arms.len(),
        opts.reps,
        arms.len() * opts.reps as usize,
        env.spec.id,
        format!("{:?}", env.spec.class).to_lowercase()
    );
    for a in arms {
        eprintln!(
            "  {}:{}  tier {}  {}",
            a.entrant.id(),
            a.variant.id,
            a.variant.tier,
            a.variant
                .reports
                .get("wire_format")
                .map_or("-", String::as_str)
        );
    }
    if opts.dry_run {
        eprintln!("dry run: nothing was started");
        return Ok(());
    }

    // One arm at a time, across the whole host. Two arms sharing this machine
    // would each measure the other.
    let _lock = ArmLock::acquire("bench run").map_err(|e| format!("REFUSED: {e}"))?;

    let (ep, infra, mut base_flags) = infra::bring_up(&env, opts.reuse_infra)?;
    if !env.is_publishable() {
        base_flags.push(Flag::ThirdPartyHardware);
    }

    let schema_id = corpus::register_schema(&ep.registry_host, ep.registry_port);
    for stmt in corpus::ddl_statements() {
        crate::docker::clickhouse_sql(&ep.ch_host, ep.ch_port, &ep.ch_user, &ep.ch_password, &stmt)
            .map_err(|e| format!("DDL failed: {e}"))?;
    }
    let _ = schema_id;

    let corpus_rows_a = corpus::expected(opts.batches, Tier::A).rows;
    let corpus_rows_b = corpus::expected(opts.batches, Tier::B).rows;

    let mut refusals = Vec::new();
    let mut emitted = 0usize;

    // Interleaved, not batched. Running all of one arm and then all of another
    // has already manufactured a fake 30% difference in a related project: the
    // machine is not in the same state at the end of a long run as at the start,
    // and batching aliases that drift onto whichever arm went last.
    for rep in 1..=opts.reps {
        for arm in arms {
            let expected_rows = match arm.variant.tier.as_str() {
                "b" => corpus_rows_b,
                _ => corpus_rows_a,
            };
            eprintln!(
                "\n=== rep {rep}/{} — {}:{} ===",
                opts.reps,
                arm.entrant.id(),
                arm.variant.id
            );
            match measure(
                root,
                &env,
                &ep,
                &infra,
                arm,
                opts,
                rep,
                expected_rows,
                &base_flags,
            ) {
                Ok(Some(path)) => {
                    emitted += 1;
                    eprintln!("recorded in {}", path.display());
                }
                Ok(None) => {}
                Err(why) => {
                    eprintln!("REFUSED {}:{}: {why}", arm.entrant.id(), arm.variant.id);
                    refusals.push(format!("{}:{} — {why}", arm.entrant.id(), arm.variant.id));
                    if opts.fail_fast {
                        return Err(format!("stopping after the first refusal: {why}"));
                    }
                }
            }
        }
    }

    eprintln!(
        "\n{emitted} record(s) written, {} refusal(s)",
        refusals.len()
    );
    for r in &refusals {
        eprintln!("  {r}");
    }
    Ok(())
}

/// One repetition of one arm.
#[expect(
    clippy::too_many_arguments,
    reason = "splitting this would hide the order of operations, which is the part that has to be right"
)]
fn measure(
    root: &Path,
    env: &Environment,
    ep: &Endpoints,
    infra: &Infra,
    arm: &Arm<'_>,
    opts: &RunOptions,
    rep: u32,
    expected_rows: u64,
    base_flags: &[Flag],
) -> Result<Option<std::path::PathBuf>, String> {
    let tier = match arm.variant.tier.as_str() {
        "b" => Tier::B,
        _ => Tier::A,
    };
    let image = arm
        .image
        .clone()
        .or_else(|| arm.entrant.spec.build.as_ref().map(|b| b.image.clone()))
        .ok_or("no image for this entrant")?;

    // Resolve what is about to run BEFORE running it. A digest that cannot be
    // read is a refusal, not an optional field: version strings can be
    // re-pushed under the same tag, and a record that cannot say what produced
    // it is not evidence.
    let sut = resolve_sut(arm, &image)?;

    // A clean table per repetition. Without this the gate would see the previous
    // repetition's rows and the row delta would be meaningless.
    crate::docker::clickhouse_sql(
        &ep.ch_host,
        ep.ch_port,
        &ep.ch_user,
        &ep.ch_password,
        &format!("TRUNCATE TABLE {}", tier.table()),
    )
    .map_err(|e| format!("truncate failed: {e}"))?;

    let specs = build_specs(arm, ep, opts, &image)?;
    let names: Vec<String> = specs.iter().map(|s| s.name.clone()).collect();
    for s in &specs {
        eprintln!(
            "  container {} ({}, --cpus={}, --memory={}) {}",
            s.name,
            s.image,
            s.cpus,
            s.memory,
            s.args.join(" ")
        );
    }

    let rows_now = || -> u64 {
        crate::docker::clickhouse_sql(
            &ep.ch_host,
            ep.ch_port,
            &ep.ch_user,
            &ep.ch_password,
            &format!("SELECT count() FROM {}", tier.table()),
        )
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0)
    };

    sampler::start_arm(&specs);
    let samplers = sampler::sample_arm(&names, SAMPLE_INTERVAL_S);

    // Drain: run until the corpus is exhausted. The window is the drain.
    let t0 = Instant::now();
    let mut rows;
    let mut first_row_at: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if !names.iter().any(|n| sampler::sut_alive(n)) {
            let logs = stop_all(&names);
            return Err(format!("the arm exited during the drain. Logs:\n{logs}"));
        }
        rows = rows_now();
        if first_row_at.is_none() && rows > 0 {
            first_row_at = Some(Instant::now());
        }
        if rows >= expected_rows {
            break;
        }
        if t0.elapsed() > Duration::from_secs(DRAIN_MAX_S) {
            let logs = stop_all(&names);
            return Err(format!(
                "the drain did not finish within {DRAIN_MAX_S}s ({rows} of {expected_rows} rows). \
                 Logs:\n{logs}"
            ));
        }
    }

    let costs: Vec<(String, crate::sampler::Samples)> = samplers
        .into_iter()
        .zip(&names)
        .map(|(s, n)| (n.clone(), s.stop()))
        .collect();

    // Defect this closes: the sampler already reads `cpu.max` and `memory.max`
    // back out of the arm's cgroup — the literal proof the envelope was applied
    // — and the previous harness discarded it in `summarise()`. Asserting it
    // here is what makes the methodology's claim true for the arms as well as
    // for the infrastructure.
    for (name, s) in &costs {
        let declared = arm
            .entrant
            .spec
            .envelope
            .as_ref()
            .and_then(|e| e.containers.iter().find(|c| name.ends_with(&c.name)));
        if let Some(c) = declared {
            assert_arm_caps(name, c, &s.meta)?;
        }
    }

    let parts: Vec<(String, Option<SutCost>)> = costs
        .iter()
        .map(|(n, s)| (n.clone(), s.summarise()))
        .collect();
    for (name, c) in &parts {
        if let Some(c) = c {
            eprintln!(
                "  {name}: {:.2} cores, peak anon {:.1} MB{}",
                c.cores_used,
                c.peak_anon_bytes / 1e6,
                if c.was_throttled() { " THROTTLED" } else { "" }
            );
        }
    }

    let data_plane_name = arm
        .entrant
        .data_plane()
        .map(|c| format!("spate-bench-sut-{}", c.name));
    let data_plane_cost = parts
        .iter()
        .find(|(n, _)| Some(n) == data_plane_name.as_ref())
        .and_then(|(_, c)| *c);
    let collected: Vec<SutCost> = parts.iter().filter_map(|(_, c)| *c).collect();

    // The producer round-robins across partitions and consumers drain them
    // independently, so at any instant the consumed frontier is RAGGED: the most
    // advanced partition sets the maximum while slower ones leave holes below it.
    // Gating on that snapshot reports those holes as data loss, which is how the
    // problem was originally found. The metrics still come from the drain above;
    // only the gate sees these extra rows, which is exactly right — the question
    // the gate asks is "did everything produced eventually arrive?".
    quiesce(&rows_now);

    let logs = stop_all(&names);
    let Some(cost) = SutCost::sum(&collected) else {
        return Err(format!(
            "the cgroup sampler produced fewer than two samples, so there is no CPU \
             delta and no measurement. Logs:\n{logs}"
        ));
    };

    let window_s = first_row_at.map_or_else(
        || t0.elapsed().as_secs_f64(),
        |t| t.elapsed().as_secs_f64() + 1.0,
    );
    #[expect(
        clippy::cast_precision_loss,
        reason = "row counts stay far below f64's exact range"
    )]
    let rows_f = rows as f64;
    let rows_per_s = if window_s > 0.0 {
        rows_f / window_s
    } else {
        0.0
    };

    let mut flags = base_flags.to_vec();
    if cost.was_throttled() {
        flags.push(Flag::CpuCapThrottled);
    }

    // The headroom rule, enforced rather than checked by hand. Above the limit we
    // are measuring the shared consume path and not the system, so the record is
    // emitted with a failed status and no metrics rather than published.
    let mut status = Status::Ok;
    let mut note = format!("{} sampler samples", cost.samples);
    if infra.ceiling_msgs_per_s > 0 {
        #[expect(clippy::cast_precision_loss, reason = "ceilings are small integers")]
        let ceiling_rows = infra.ceiling_msgs_per_s as f64 * f64::from(corpus::EVENTS_PER_BATCH);
        let share = rows_per_s / ceiling_rows;
        eprintln!(
            "  headroom: {:.0}% of the proven consume ceiling",
            share * 100.0
        );
        if share > HEADROOM_LIMIT {
            status = Status::InfraBound;
            note.push_str(&format!(
                "; INFRA-BOUND at {:.0}% of the ceiling",
                share * 100.0
            ));
        }
    }

    // Correctness gates. An arm that loses rows is faster for the wrong reason,
    // and one that computes different values did different work.
    let gates = corpus::run_gates(
        &ep.ch_host,
        ep.ch_port,
        &ep.ch_user,
        &ep.ch_password,
        tier,
        GATE_MAX_BATCHES,
    )
    .map_err(|e| format!("{e}\nLogs:\n{logs}"))?;
    if let Some(why) = gates.failure() {
        return Err(format!("correctness gate failed: {why}\nLogs:\n{logs}"));
    }
    note.push_str(&format!("; gate window {GATE_MAX_BATCHES} batches"));

    let mut report = Report::new(
        "kafka_avro_clickhouse",
        Kind::Measurement,
        status,
        sut,
        RunMeta::new(&env.spec.id, &env.digest, opts.trigger, infra.clone()),
    )
    .rep(rep, opts.reps)
    .variant("tier", arm.variant.tier.clone())
    .variant(
        "approach",
        format!("{:?}", arm.variant.approach).to_lowercase(),
    )
    .variant("mode", "drain")
    .variant("partitions", i64::from(env.spec.infra.partitions))
    .variant("batches", i64::try_from(opts.batches).unwrap_or(i64::MAX));

    for (k, v) in &arm.variant.reports {
        report = report.variant(k.clone(), v.clone());
    }
    for (k, v) in &arm.variant.knobs {
        if let Some(n) = v.as_integer() {
            report = report.variant(k.clone(), n);
        } else if let Some(s) = v.as_str() {
            report = report.variant(k.clone(), s.to_owned());
        }
    }

    if status.carries_metrics() {
        report = report
            .metric("rows_per_s", Metric::maximize(rows_per_s, "records/s"))
            .metric(
                "cpu_us_per_row",
                Metric::minimize(cost.cpu_us_per_row(rows_f), "us"),
            )
            .metric("cores_used", Metric::minimize(cost.cores_used, "cores"))
            .metric(
                "rows_per_s_per_core",
                Metric::maximize(
                    if cost.cores_used > 0.0 {
                        rows_per_s / cost.cores_used
                    } else {
                        0.0
                    },
                    "records/s",
                ),
            )
            .metric("peak_anon_bytes", Metric::bytes(cost.peak_anon_bytes))
            .metric("peak_charged_bytes", Metric::bytes(cost.peak_charged_bytes))
            .metric("throttled_us", Metric::minimize(cost.throttled_us, "us"))
            .metric(
                "duplicate_rows",
                // Reported, never suppressed: these are at-least-once systems and
                // some duplication is legitimate. Hiding it would misrepresent the
                // guarantee being compared.
                #[expect(clippy::cast_precision_loss, reason = "counts stay small")]
                Metric::minimize(gates.duplicates as f64, "rows"),
            );
        // The contract promises a data-plane figure alongside the total, so that
        // nobody can claim we taxed a multi-process system for its control plane.
        if let Some(dp) = data_plane_cost {
            report = report
                .metric(
                    "data_plane_cores_used",
                    Metric::minimize(dp.cores_used, "cores"),
                )
                .metric(
                    "data_plane_peak_anon_bytes",
                    Metric::bytes(dp.peak_anon_bytes),
                );
        }
    }

    for f in flags {
        report = report.flag(f);
    }
    report = report.note(note);

    eprintln!(
        "  {rows} rows in {window_s:.1}s = {rows_per_s:.0} rows/s; {:.2} cores; {:.3} us/row",
        cost.cores_used,
        cost.cpu_us_per_row(rows_f)
    );

    let path = results::append(&root.join("results"), &report)
        .map_err(|e| format!("append record: {e}"))?;
    Ok(Some(path))
}

/// Waits for the pipeline to settle so the gate sees a complete frontier.
fn quiesce(rows_now: &dyn Fn() -> u64) {
    let mut stable = 0u32;
    let mut prev = rows_now();
    let deadline = Instant::now() + Duration::from_secs(QUIESCE_MAX_S);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let n = rows_now();
        if n == prev {
            stable += 1;
            // Three consecutive unchanged polls: one is not enough, because a
            // batch in flight can straddle a single poll interval.
            if stable >= 3 {
                break;
            }
        } else {
            stable = 0;
        }
        prev = n;
        if Instant::now() > deadline {
            eprintln!(
                "WARNING: still draining after {QUIESCE_MAX_S}s; the gate may read a \
                 ragged frontier as loss."
            );
            break;
        }
    }
    eprintln!("  quiesced at {prev} rows");
}

fn stop_all(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("--- {n} ---\n{}", sampler::stop_sut(n)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads what an arm actually is, by running it.
fn resolve_sut(arm: &Arm<'_>, image: &str) -> Result<Sut, String> {
    let digest =
        crate::docker::docker_try(&["image", "inspect", "-f", "{{.Id}}", image]).map_err(|e| {
            format!(
                "cannot read the image digest for {image}: {e}. The image must be built \
                 before it can be measured (`bench build {}`), and a run whose digest \
                 cannot be read is refused rather than published without one.",
                arm.entrant.id()
            )
        })?;

    let (version, commit, toolchain) = match arm.entrant.spec.version.as_ref() {
        Some(v) if v.strategy == "command" && !v.command.is_empty() => {
            let mut argv: Vec<&str> = vec!["run", "--rm", "--entrypoint", &v.command[0], image];
            for a in &v.command[1..] {
                argv.push(a);
            }
            let out = crate::docker::docker_try(&argv)
                .map_err(|e| format!("version command failed for {image}: {e}"))?;
            parse_version(&out)
        }
        _ => (None, None, None),
    };

    // A pinned version is asserted, not assumed. A base-image bump that moves the
    // version silently would otherwise publish the old label against new code.
    if let Some(v) = arm.entrant.spec.version.as_ref()
        && !v.pinned.is_empty()
        && let Some(found) = version.as_deref()
        && found != v.pinned
    {
        return Err(format!(
            "REFUSED: {} declares version {:?} but the image reports {found:?}. \
             Update the descriptor in the same change that moved the image.",
            arm.entrant.id(),
            v.pinned
        ));
    }

    if version.is_none() && commit.is_none() {
        return Err(format!(
            "REFUSED: could not resolve a version or commit for {}. Every published \
             number has to say what produced it.",
            arm.entrant.id()
        ));
    }

    Ok(Sut {
        entrant: arm.entrant.id().to_owned(),
        variant_id: arm.variant.id.clone(),
        version,
        commit,
        image_digest: digest,
        image: image.to_owned(),
        toolchain,
    })
}

/// Extracts `(version, commit, toolchain)` from an arm's `--version` output.
///
/// The contract is a line containing a version-like token, optionally followed
/// by a parenthesised commit, and optionally a `toolchain:` line. Deliberately
/// not a regex: a regex engine is a dependency compiled into every arm image for
/// one parse, and the shape is fixed by us rather than discovered.
fn parse_version(out: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut version = None;
    let mut commit = None;
    let mut toolchain = None;

    for line in out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("toolchain:") {
            toolchain = Some(rest.trim().to_owned());
            continue;
        }
        if version.is_none() {
            version = line
                .split_whitespace()
                .find(|t| {
                    t.starts_with(|c: char| c.is_ascii_digit())
                        && t.contains('.')
                        && t.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                })
                .map(str::to_owned);
        }
        if commit.is_none()
            && let Some(open) = line.find('(')
            && let Some(close) = line[open..].find(')')
        {
            let inner = &line[open + 1..open + close];
            if inner.len() >= 7 && inner.chars().all(|c| c.is_ascii_hexdigit()) {
                commit = Some(inner.to_owned());
            }
        }
    }
    (version, commit, toolchain)
}

/// Builds the container specs for one arm from its descriptor.
fn build_specs(
    arm: &Arm<'_>,
    ep: &Endpoints,
    opts: &RunOptions,
    image: &str,
) -> Result<Vec<SutSpec>, String> {
    let envelope = arm
        .entrant
        .spec
        .envelope
        .as_ref()
        .ok_or("entrant has no envelope")?;

    // A FRESH consumer group per run, not a stable one per arm.
    //
    // Drain replays the prefilled corpus from offset zero, which `earliest` only
    // does for a group with no committed offsets. A stable group id would commit
    // at the end of rep 1, and rep 2 would then resume at the tail, consume
    // nothing, and sit there until the drain deadline — reporting a timeout for
    // an arm that is working perfectly.
    let group_id = format!(
        "comparison-{}-{}-{}",
        arm.entrant.id(),
        arm.variant.id,
        uuid::Uuid::now_v7().simple()
    );
    let container_names: BTreeMap<&str, String> = envelope
        .containers
        .iter()
        .map(|c| (c.name.as_str(), format!("spate-bench-sut-{}", c.name)))
        .collect();

    let volumes: Vec<String> = arm
        .entrant
        .spec
        .volumes
        .as_ref()
        .map(|v| v.named.clone())
        .unwrap_or_default();

    let mut specs = Vec::new();
    for c in &envelope.containers {
        let name = container_names[c.name.as_str()].clone();
        let mut env: Vec<(String, String)> = Vec::new();

        // The entrant's own vocabulary, with the driver-owned values substituted.
        // Sending one system's variable names to another would leave it on its
        // defaults while the record claimed knob values that were never applied —
        // a silent misreport rather than a visible failure.
        for (k, raw) in arm.entrant.spec.env.iter().chain(arm.variant.env.iter()) {
            let v = substitute(raw, arm.variant, ep, opts, &group_id, &container_names)?;
            env.push((k.clone(), v));
        }

        specs.push(SutSpec {
            name,
            image: image.to_owned(),
            cpus: c.cpus.clone(),
            memory: c.memory.clone(),
            env,
            args: c.args.clone(),
            volumes: volumes.clone(),
        });
    }

    // Control plane first: a TaskManager that starts before its JobManager spends
    // its first seconds retrying a connection, which lands inside the measurement.
    specs.sort_by_key(|s| {
        let role = envelope
            .containers
            .iter()
            .find(|c| s.name.ends_with(&c.name))
            .map(|c| c.role);
        u8::from(role != Some(Role::ControlPlane))
    });
    Ok(specs)
}

fn substitute(
    raw: &str,
    variant: &Variant,
    ep: &Endpoints,
    opts: &RunOptions,
    group_id: &str,
    containers: &BTreeMap<&str, String>,
) -> Result<String, String> {
    let mut out = raw.to_owned();
    let simple = [
        ("{{broker_internal}}", ep.bootstrap_internal.clone()),
        ("{{registry_internal}}", ep.registry_internal.clone()),
        ("{{clickhouse_internal}}", ep.ch_internal.clone()),
        ("{{topic}}", opts.topic.clone()),
        ("{{group_id}}", group_id.to_owned()),
        ("{{tier}}", variant.tier.clone()),
        // Drain replays a prefilled corpus from the beginning.
        ("{{offset_reset}}", "earliest".to_owned()),
    ];
    for (k, v) in simple {
        out = out.replace(k, &v);
    }

    for (k, v) in &variant.knobs {
        let text = v.as_integer().map_or_else(
            || v.as_str().unwrap_or_default().to_owned(),
            |n| n.to_string(),
        );
        out = out.replace(&format!("{{{{knob:{k}}}}}"), &text);
    }
    for (name, container) in containers {
        out = out.replace(&format!("{{{{container:{name}}}}}"), container);
    }

    // An unresolved placeholder would reach the container verbatim and be read as
    // a literal, so the arm would run misconfigured while the record claimed the
    // intended value. Fail instead.
    if out.contains("{{") {
        return Err(format!(
            "REFUSED: unresolved placeholder in {raw:?} (produced {out:?}). A \
             placeholder that reaches the container is read as a literal, so the \
             arm would run misconfigured while the record claimed otherwise."
        ));
    }
    Ok(out)
}

/// Asserts an arm's applied cgroup caps against what its descriptor declares.
fn assert_arm_caps(name: &str, declared: &Container, meta: &str) -> Result<(), String> {
    // `# cgroup=… cpu.max=<quota>/<period> memory.max=<bytes> …`
    let field =
        |key: &str| -> Option<&str> { meta.split_whitespace().find_map(|t| t.strip_prefix(key)) };
    let cpu = field("cpu.max=").ok_or_else(|| format!("{name}: sampler reported no cpu.max"))?;
    let mem =
        field("memory.max=").ok_or_else(|| format!("{name}: sampler reported no memory.max"))?;

    let cores = {
        let mut it = cpu.split('/');
        let q = it.next().unwrap_or_default();
        let p: f64 = it.next().unwrap_or("0").parse().unwrap_or(0.0);
        if q == "max" || p <= 0.0 {
            None
        } else {
            q.parse::<f64>().ok().map(|q| q / p)
        }
    };
    let want_cores: f64 = declared.cpus.parse().unwrap_or(f64::NAN);
    if !cores.is_some_and(|c| (c - want_cores).abs() < 0.01) {
        return Err(format!(
            "REFUSED: {name} declares cpus={} but is running under cpu.max={cpu}. \
             The envelope is what every published number is described by.",
            declared.cpus
        ));
    }

    let want_bytes = memory_bytes(&declared.memory);
    if want_bytes != mem.parse::<u64>().ok() {
        return Err(format!(
            "REFUSED: {name} declares memory={} but is running under memory.max={mem}.",
            declared.memory
        ));
    }
    Ok(())
}

fn memory_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last()? {
        'g' | 'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        'm' | 'M' => (&s[..s.len() - 1], 1024 * 1024),
        _ => (s, 1),
    };
    digits.trim().parse::<u64>().ok().map(|n| n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_arms_version_line() {
        let (v, c, t) =
            parse_version("spate-arm 0.1.0-dev (f41280d51165)\ntoolchain: rustc 1.97.1");
        assert_eq!(v.as_deref(), Some("0.1.0-dev"));
        assert_eq!(c.as_deref(), Some("f41280d51165"));
        assert_eq!(t.as_deref(), Some("rustc 1.97.1"));
    }

    #[test]
    fn parses_a_bare_version() {
        let (v, c, _) = parse_version("2.2.1\n");
        assert_eq!(v.as_deref(), Some("2.2.1"));
        assert_eq!(c, None);
    }

    #[test]
    fn a_non_hex_parenthetical_is_not_a_commit() {
        // "(build 42)" is not a commit, and recording it as one would put a
        // fabricated provenance field on a published record.
        let (_, c, _) = parse_version("thing 1.2.3 (not a sha)");
        assert_eq!(c, None);
    }

    #[test]
    fn an_arm_cap_mismatch_is_refused() {
        let declared = Container {
            role: Role::DataPlane,
            name: "sut".to_owned(),
            cpus: "4".to_owned(),
            memory: "16g".to_owned(),
            args: vec![],
        };
        let good = "# cgroup=/x cpu.max=400000/100000 memory.max=17179869184 x=1";
        assert!(assert_arm_caps("sut", &declared, good).is_ok());

        // The evidence the previous harness threw away: this is the sampler
        // proving the arm did NOT get the envelope it is described by.
        let bad = "# cgroup=/x cpu.max=200000/100000 memory.max=17179869184 x=1";
        let e = assert_arm_caps("sut", &declared, bad).expect_err("must refuse");
        assert!(e.starts_with("REFUSED"), "{e}");

        let uncapped = "# cgroup=/x cpu.max=max/100000 memory.max=max x=1";
        assert!(assert_arm_caps("sut", &declared, uncapped).is_err());
    }
}
