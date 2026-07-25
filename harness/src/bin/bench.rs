//! `bench` — the benchmark driver.
//!
//! Subcommands and flags rather than environment variables, deliberately. The
//! previous harness took its configuration from the environment, and that is
//! precisely how a runner script, the driver's own defaults and the written
//! methodology came to state three different infrastructure envelopes while
//! every recorded number stayed silent about which had been in force.
//!
//! Two properties of this tool are load-bearing rather than convenient:
//!
//! - **`--dry-run` prints the exact execution list.** A full sweep costs hours,
//!   so "which arms will this actually run?" has to be answerable before
//!   spending them rather than inferred afterwards from what appeared.
//! - **Nothing here can truncate a results file.** See `results.rs`: the
//!   capability does not exist, so retention is not a matter of remembering.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use spate_benchmark_harness::driver::{self, Mode, RunOptions};
use spate_benchmark_harness::entrant::{self, Entrant, Status};
use spate_benchmark_harness::environment::Environment;
use spate_benchmark_harness::report::{DATASET_VERSION, HARNESS_VERSION, Trigger};
use spate_benchmark_harness::results;
use spate_benchmark_harness::select::{self, Selector};

/// Exit code for a refusal: the run was attempted and declined to produce a
/// number. Distinct from 1 (a usage error) so a sweep script can tell "this arm
/// is invalid" from "you called me wrong".
const EXIT_REFUSED: u8 = 3;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        usage();
        return ExitCode::from(1);
    };
    let rest = &args[1..];

    let root = repo_root();

    let result = match cmd {
        "list" => cmd_list(&root, rest),
        "validate" => cmd_validate(&root),
        "build" => cmd_build(&root, rest),
        "stale" => cmd_stale(&root),
        "retract" => cmd_retract(&root, rest),
        "prefill" => cmd_prefill(&root, rest),
        "ceiling" => cmd_ceiling(&root, rest),
        "run" => cmd_run(&root, rest),
        "-h" | "--help" | "help" => {
            usage();
            return ExitCode::SUCCESS;
        }
        other => Err(format!("unknown subcommand {other:?}. Try `bench help`.")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("bench: {msg}");
            ExitCode::from(if msg.starts_with("REFUSED") {
                EXIT_REFUSED
            } else {
                1
            })
        }
    }
}

fn usage() {
    println!(
        "\
bench — the Spate Benchmark driver

  bench list [--json]            systems, variants, and when each was last measured
  bench validate                 what CI checks, runnable locally
  bench build <selector>...      build the selected entrants' images
  bench stale                    arms whose measurement has fallen behind
  bench retract <run_id> --reason <text>
  bench prefill                  populate the topic once per corpus
  bench ceiling                  prove the infrastructure is not the bottleneck
  bench run <selector>... [--reps N] [--dry-run] [--env <id>]

selectors:
  <entrant>[:<variant>[:<version>]]   '*' means any, '@<tag>' overrides the image

  spate                          every variant of one system
  spate:tier-a-rowbinary         one arm
  '*'                            everything runnable
  flink@spate-bench-flink:2.3.0  a specific image — how a new version is measured

`bench run` only ever appends. There is no code path in it that truncates a
results file."
    );
}

/// Flag parsing. Deliberately hand-rolled and strict: an unrecognised flag is an
/// error rather than something ignored, because a typo'd `--reps` that silently
/// ran once instead of three times would produce a result whose repetition count
/// nobody questioned.
fn opts_from(args: &[String], root: &Path) -> Result<RunOptions, String> {
    let mut o = RunOptions {
        reps: 3,
        mode: Mode::Drain,
        env_id: default_env(root)?,
        trigger: Trigger::Manual,
        dry_run: false,
        reuse_infra: false,
        fail_fast: false,
        topic: "comparison-sensor-batches".to_owned(),
        batches: 1_500_000,
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if !a.starts_with('-') {
            i += 1;
            continue;
        }
        let mut value = || -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", args[i - 1]))
        };
        match a.as_str() {
            "--reps" => o.reps = value()?.parse().map_err(|e| format!("--reps: {e}"))?,
            "--env" => o.env_id = value()?,
            "--topic" => o.topic = value()?,
            "--batches" => o.batches = value()?.parse().map_err(|e| format!("--batches: {e}"))?,
            "--trigger" => {
                o.trigger = match value()?.as_str() {
                    "nightly" => Trigger::Nightly,
                    "manual" => Trigger::Manual,
                    "pr" => Trigger::Pr,
                    "release" => Trigger::Release,
                    other => return Err(format!("unknown --trigger {other:?}")),
                }
            }
            "--dry-run" => o.dry_run = true,
            "--reuse-infra" => o.reuse_infra = true,
            "--fail-fast" => o.fail_fast = true,
            other => return Err(format!("unknown flag {other:?}. Try `bench help`.")),
        }
        i += 1;
    }
    Ok(o)
}

/// The environment to use when none is named.
///
/// Refuses when there is more than one rather than picking: an ambient default
/// that silently selects hardware is exactly the class of thing this harness
/// exists to remove.
fn default_env(root: &Path) -> Result<String, String> {
    let dir = root.join("environments");
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .map_err(|e| format!("read environments: {e}"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
        .collect();
    ids.sort();
    match ids.len() {
        1 => Ok(ids.remove(0)),
        0 => Err("no environment profiles in environments/".to_owned()),
        _ => Err(format!(
            "several environments exist ({}); name one with --env. Guessing which \
             hardware a number describes is not something this tool does.",
            ids.join(", ")
        )),
    }
}

fn cmd_prefill(root: &Path, args: &[String]) -> Result<(), String> {
    driver::prefill(root, &opts_from(args, root)?)
}

fn cmd_ceiling(root: &Path, args: &[String]) -> Result<(), String> {
    let opts = opts_from(args, root)?;
    let env = Environment::load(&root.join("environments"), &opts.env_id)?;
    let ceiling = env.ceiling()?;
    println!(
        "{}: proven consume ceiling {} msgs/s",
        env.spec.id, ceiling.consume_msgs_per_s
    );
    println!(
        "an arm above {:.0}% of that is infra-bound and is recorded as such rather \
         than published",
        spate_benchmark_harness::environment::HEADROOM_LIMIT * 100.0
    );
    println!(
        "\nRe-measuring the ceiling needs the raw consume rig, which has not been \
         ported into this repository yet. The committed figure was measured on \
         this environment's broker at its declared cap; see \
         environments/ceilings/ for how, and treat it as provenance rather than \
         something this command produced."
    );
    Ok(())
}

fn cmd_run(root: &Path, args: &[String]) -> Result<(), String> {
    let entrants = load_entrants(root)?;
    let selectors = parse_selectors(args)?;
    let arms = select::expand(&entrants, &selectors)?;
    let opts = opts_from(args, root)?;
    driver::run(root, &arms, &opts)
}

/// The repository root, from this binary's compile-time location.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("harness/ has a parent")
        .to_path_buf()
}

fn load_entrants(root: &Path) -> Result<Vec<Entrant>, String> {
    entrant::load_all(&root.join("entrants")).map_err(|errs| {
        format!(
            "{} descriptor problem(s):\n  - {}",
            errs.len(),
            errs.join("\n  - ")
        )
    })
}

fn cmd_validate(root: &Path) -> Result<(), String> {
    let entrants = load_entrants(root)?;
    println!("entrants: {} descriptor(s) valid", entrants.len());

    // Every environment must load, since a record naming one that does not parse
    // could never be rendered.
    let env_dir = root.join("environments");
    let mut envs = 0usize;
    for e in std::fs::read_dir(&env_dir).map_err(|e| format!("read environments: {e}"))? {
        let path = e.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|x| x == "toml") {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("bad environment filename")?;
            let env = Environment::load(&env_dir, id)?;
            env.ceiling()?;
            envs += 1;
        }
    }
    println!("environments: {envs} profile(s) valid, each with a measured ceiling");

    let (records, problems) =
        results::load_all(&root.join("results")).map_err(|e| format!("read results: {e}"))?;
    if !problems.is_empty() {
        return Err(format!(
            "{} malformed result line(s):\n  - {}",
            problems.len(),
            problems.join("\n  - ")
        ));
    }
    println!("results: {} record(s) parse", records.len());
    println!("harness v{HARNESS_VERSION}, dataset {DATASET_VERSION}");
    Ok(())
}

fn cmd_list(root: &Path, args: &[String]) -> Result<(), String> {
    let entrants = load_entrants(root)?;
    let (records, _) = results::load_all(&root.join("results")).unwrap_or_default();

    if args.iter().any(|a| a == "--json") {
        let rows: Vec<serde_json::Value> = entrants
            .iter()
            .map(|e| {
                serde_json::json!({
                    "id": e.id(),
                    "name": e.spec.entrant.name,
                    "status": format!("{:?}", e.spec.entrant.status).to_lowercase(),
                    "runtime": e.spec.entrant.runtime,
                    "licence": e.spec.entrant.licence,
                    "vendor": e.spec.entrant.vendor,
                    "variants": e.spec.variants.iter().map(|v| &v.id).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    for e in &entrants {
        let status = format!("{:?}", e.spec.entrant.status).to_lowercase();
        let ours = if e.spec.entrant.vendor == "self" {
            "  [vendor-run]"
        } else {
            ""
        };
        println!(
            "{:<24} {:<11} {:<7} {}{ours}",
            e.id(),
            status,
            e.spec.entrant.runtime,
            e.spec.entrant.name
        );

        if e.spec.entrant.status != Status::Planned {
            for v in &e.spec.variants {
                let last = records
                    .iter()
                    .filter(|r| r.sut.entrant == *e.id() && r.sut.variant_id == v.id)
                    .map(|r| r.run.ts_ms)
                    .max();
                let when = last.map_or_else(
                    || "never measured".to_owned(),
                    |ts| format!("last {}", iso_day(ts)),
                );
                let approach = format!("{:?}", v.approach).to_lowercase();
                let default = if v.default { " (default)" } else { "" };
                println!("    {:<28} {approach:<10} {when}{default}", v.id);
            }
        } else if let Some(p) = &e.spec.planned {
            let first = p.blockers.trim().lines().next().unwrap_or("");
            println!("    blocked: {first}");
        }
    }
    Ok(())
}

fn cmd_build(root: &Path, args: &[String]) -> Result<(), String> {
    let entrants = load_entrants(root)?;
    let selectors = parse_selectors(args)?;
    let arms = select::expand(&entrants, &selectors)?;

    // One image per entrant, not per arm: variants differ by environment, not by
    // build. Building the same image once per variant would multiply a slow
    // docker build by the variant count for no difference in the result.
    let mut seen = std::collections::BTreeSet::new();
    for arm in &arms {
        if !seen.insert(arm.entrant.id()) {
            continue;
        }
        let e = arm.entrant;
        let build = e
            .spec
            .build
            .as_ref()
            .ok_or_else(|| format!("{}: no [build] section", e.id()))?;

        let context = e.dir.join(&build.context);
        let dockerfile = e.dir.join(&build.dockerfile);
        let dockerfile_rel = dockerfile
            .strip_prefix(&context)
            .map_err(|_| format!("{}: dockerfile is outside the build context", e.id()))?;

        let mut argv: Vec<String> = vec![
            "build".into(),
            "-f".into(),
            dockerfile_rel.display().to_string(),
            "-t".into(),
            build.image.clone(),
        ];
        for s in &build.secrets {
            // The private framework dependency. A secret rather than a build ARG:
            // an ARG is baked into image history, and this repository's images
            // must never carry a credential.
            argv.push("--secret".into());
            argv.push(format!("id={s},src={}", credential_path(s)?));
        }
        argv.push(".".into());

        println!("building {} -> {}", e.id(), build.image);
        let status = std::process::Command::new("docker")
            .args(&argv)
            .current_dir(&context)
            .status()
            .map_err(|err| format!("docker: {err}"))?;
        if !status.success() {
            return Err(format!("{}: docker build failed", e.id()));
        }
    }
    Ok(())
}

/// Where a named build secret's material lives on this machine.
fn credential_path(id: &str) -> Result<String, String> {
    match id {
        "gitcred" => {
            let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_owned())?;
            let p = format!("{home}/.git-credentials");
            if Path::new(&p).is_file() {
                Ok(p)
            } else {
                Err(format!(
                    "the Spate arm needs a credential for the private framework \
                     repository, and {p} does not exist. Run `gh auth setup-git` \
                     (or `git config --global credential.helper store` and \
                     authenticate once). This disappears when the framework \
                     publishes to crates.io."
                ))
            }
        }
        other => Err(format!("unknown build secret {other:?}")),
    }
}

fn cmd_stale(root: &Path) -> Result<(), String> {
    let entrants = load_entrants(root)?;
    let (records, _) = results::load_all(&root.join("results")).unwrap_or_default();

    let mut any = false;
    for e in entrants
        .iter()
        .filter(|e| e.spec.entrant.status.is_runnable())
    {
        for v in &e.spec.variants {
            let latest = records
                .iter()
                .filter(|r| r.sut.entrant == *e.id() && r.sut.variant_id == v.id)
                .max_by_key(|r| r.run.ts_ms);
            match latest {
                None => {
                    any = true;
                    println!("{}:{} — never measured", e.id(), v.id);
                }
                // A record produced under a superseded protocol is stale in the
                // way that matters most: it cannot be drawn on the same axis as
                // anything current, so it is invisible rather than merely old.
                Some(r) if r.run.harness_version != HARNESS_VERSION => {
                    any = true;
                    println!(
                        "{}:{} — harness v{} (current v{HARNESS_VERSION}); not comparable",
                        e.id(),
                        v.id,
                        r.run.harness_version
                    );
                }
                Some(r) if r.run.dataset_version != DATASET_VERSION => {
                    any = true;
                    println!(
                        "{}:{} — dataset {} (current {DATASET_VERSION}); not comparable",
                        e.id(),
                        v.id,
                        r.run.dataset_version
                    );
                }
                Some(_) => {}
            }
        }
    }
    if !any {
        println!("every runnable arm has a current measurement");
    }
    Ok(())
}

fn cmd_retract(root: &Path, args: &[String]) -> Result<(), String> {
    let run_id = args
        .first()
        .ok_or("usage: bench retract <run_id> --reason <text>")?;
    let reason = args
        .iter()
        .position(|a| a == "--reason")
        .and_then(|i| args.get(i + 1))
        .ok_or("a retraction must state a reason: --reason <text>")?;

    let path = results::retract(&root.join("results"), run_id, reason)
        .map_err(|e| format!("retract {run_id}: {e}"))?;
    println!("retracted {run_id} in {}", path.display());
    println!(
        "The record is still present and will still be shown, struck through, with \
         the reason attached. Commit this change so the retraction is part of the \
         published history."
    );
    Ok(())
}

fn parse_selectors(args: &[String]) -> Result<Vec<Selector>, String> {
    let raw: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if raw.is_empty() {
        return Err("no selector given. Use '*' for everything.".to_owned());
    }
    raw.iter().map(|s| Selector::parse(s)).collect()
}

/// `YYYY-MM-DD` from epoch milliseconds.
fn iso_day(ts_ms: u64) -> String {
    let days = (ts_ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
