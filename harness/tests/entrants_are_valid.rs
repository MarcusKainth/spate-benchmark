//! Every committed descriptor parses and satisfies the contract.
//!
//! This is the gate that keeps the entrant contract from becoming decoration.
//! Without it a descriptor could declare a 4 CPU envelope while the driver
//! started containers totalling five, or claim a wire format no arm writes, and
//! nothing would notice until a reader did.
//!
//! The last two tests check things that live *outside* the descriptor but which
//! it asserts: the Flink JVM's own sizing against its declared container, and
//! the Rust toolchain pin against the arm's image. Both are cases where two
//! files have to agree and neither is obviously the source of truth — exactly
//! the shape that drifts silently.

use std::path::{Path, PathBuf};

use spate_benchmark_harness::entrant::{self, Approach, Role, Status};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("harness/ has a parent")
        .to_path_buf()
}

fn entrants_dir() -> PathBuf {
    repo_root().join("entrants")
}

#[test]
fn every_descriptor_parses_and_validates() {
    match entrant::load_all(&entrants_dir()) {
        Ok(entrants) => {
            assert!(!entrants.is_empty(), "no entrants found");
        }
        Err(errors) => panic!(
            "{} descriptor problem(s):\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        ),
    }
}

#[test]
fn the_vendor_entry_is_unique_and_present() {
    // Exactly one entrant may claim `vendor = "self"`. The site renders its
    // conflict-of-interest disclosure from that field, so two would be
    // meaningless and zero would silently drop the disclosure entirely.
    let entrants = entrant::load_all(&entrants_dir()).expect("descriptors valid");
    let ours: Vec<&str> = entrants
        .iter()
        .filter(|e| e.spec.entrant.vendor == "self")
        .map(entrant::Entrant::id)
        .collect();
    assert_eq!(ours, ["spate"], "expected exactly one vendor-run entrant");
}

#[test]
fn every_active_entrant_has_a_realistic_default() {
    let entrants = entrant::load_all(&entrants_dir()).expect("descriptors valid");
    for e in entrants
        .iter()
        .filter(|e| e.spec.entrant.status.is_runnable())
    {
        let d = e.default_variant().expect("a default variant");
        assert_eq!(
            d.approach,
            Approach::Realistic,
            "{}: default variant {} must be realistic",
            e.id(),
            d.id
        );
    }
}

#[test]
fn planned_entrants_explain_themselves() {
    // A roadmap that says only "later" is a promise, not a plan. Anything not yet
    // measured has to say what is blocking it, so the gap is legible to a reader
    // deciding whether the omission is convenient for us.
    let entrants = entrant::load_all(&entrants_dir()).expect("descriptors valid");
    let planned: Vec<_> = entrants
        .iter()
        .filter(|e| e.spec.entrant.status == Status::Planned)
        .collect();
    assert!(!planned.is_empty(), "expected the roadmap to be non-empty");
    for e in planned {
        let p = e.spec.planned.as_ref().expect("[planned] present");
        assert!(
            p.blockers.trim().len() > 40,
            "{}: [planned].blockers is too thin to be informative",
            e.id()
        );
    }
}

#[test]
fn flink_jvm_sizing_fits_its_declared_container() {
    // Defect this exists to prevent, found in the extracted harness: config.yaml
    // sized the TaskManager JVM for a 3 GiB container while the driver started it
    // with 4 GiB, leaving ~1.1 GiB of Flink's own allowance unused and
    // undisclosed. That handicapped a competitor in a comparison we publish,
    // which is the direction of error this benchmark can least afford.
    let entrants = entrant::load_all(&entrants_dir()).expect("descriptors valid");
    let flink = entrants
        .iter()
        .find(|e| e.id() == "flink")
        .expect("flink entrant");

    let config = std::fs::read_to_string(flink.dir.join("config.yaml")).expect("read config.yaml");
    let sizes = process_sizes(&config);
    assert_eq!(
        sizes.len(),
        2,
        "expected a JobManager and a TaskManager size"
    );

    let envelope = flink.spec.envelope.as_ref().expect("envelope");
    for container in &envelope.containers {
        let limit = mib(&container.memory).expect("container memory parses");
        // Which JVM belongs to which container is decided by ordering in the
        // file: jobmanager first, then taskmanager, matching Flink's own layout.
        let jvm = match container.role {
            Role::ControlPlane => sizes[0],
            Role::DataPlane => sizes[1],
        };
        assert!(
            jvm <= limit,
            "flink {}: JVM process.size {jvm}m exceeds the container's {limit}m",
            container.name
        );
        // Slack is required — the JVM's accounting does not cover everything in
        // the container — but a large gap means Flink is being denied memory it
        // was allocated, which is the defect above.
        let slack = limit - jvm;
        assert!(
            slack <= limit / 8,
            "flink {}: JVM process.size {jvm}m leaves {slack}m of its {limit}m \
             container unused; Flink is being handicapped",
            container.name
        );
    }
}

#[test]
fn a_jvm_containers_declared_gc_log_is_where_its_configuration_sends_it() {
    // The descriptor's `gc_log` is what the harness copies out after a run; the
    // arm's own configuration is what decides where the JVM writes. Neither
    // file is obviously the source of truth, which is the shape that drifts
    // silently — and a drift here does not fail anything, it just records no GC
    // figures (or another JVM's) for an arm that produced them.
    let entrants = entrant::load_all(&entrants_dir()).expect("descriptors valid");
    let mut seen_a_declaration = false;
    for e in entrants.iter().filter(|e| {
        e.spec.entrant.runtime == "jvm" && e.spec.entrant.status == entrant::Status::Active
    }) {
        let envelope = e
            .spec
            .envelope
            .as_ref()
            .expect("active JVM arm has envelope");
        // Every JVM container declares one, and no two containers of one arm
        // share a path — a shared path would read one JVM's pauses as the
        // other's.
        let mut paths = std::collections::BTreeSet::new();
        for c in &envelope.containers {
            let gc_log = c.gc_log.as_deref().unwrap_or_else(|| {
                panic!(
                    "{}: container {:?} declares no gc_log; a JVM arm without one \
                     publishes no GC figures at all",
                    e.id(),
                    c.name
                )
            });
            assert!(
                paths.insert(gc_log),
                "{}: two containers declare gc_log {gc_log:?}; the JVMs write \
                 separate logs or one arm's pauses are the other's",
                e.id()
            );
            seen_a_declaration = true;
            // The path must appear in the entrant's own configuration — the
            // file that actually aims the JVM's -Xlog — somewhere under its
            // directory. Searched rather than parsed, because each runtime
            // spells its options differently (Flink's config.yaml, Connect's
            // Dockerfile KAFKA_OPTS) and a parser per runtime would be the
            // per-entrant harness knowledge this field exists to remove.
            let mentioned = configuration_files(&e.dir)
                .iter()
                .any(|text| text.contains(gc_log));
            assert!(
                mentioned,
                "{}: gc_log {gc_log:?} appears in no configuration file under {}; \
                 the descriptor names a path nothing writes to",
                e.id(),
                e.dir.display()
            );
        }
    }
    assert!(
        seen_a_declaration,
        "no active JVM arm declared a gc_log; this test is checking nothing"
    );
}

/// Every plausibly-configuration file directly under an entrant's directory,
/// read as text. Flat rather than recursive: the files that aim a JVM's flags
/// live at the top of the entrant, and a recursive walk would read source trees.
fn configuration_files(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_file()
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push(text);
            }
        }
    }
    out
}

#[test]
fn the_flink_images_parallelism_matches_the_number_it_asserts_about_itself() {
    // Two files inside one image have to agree, and neither is obviously the
    // source of truth — the shape that drifts silently.
    //
    // `ComparisonJob` refuses to submit unless the parallelism the cluster
    // resolved equals `EXPECT_PARALLELISM`. That check is what stops a
    // parallelism sweep recording values it never ran at, and it only works if
    // the image's own default asserts the truth about itself: a container run by
    // hand gets `config.yaml`'s `parallelism.default` and the Dockerfile's
    // `EXPECT_PARALLELISM`, with no driver to set either. If those two disagree,
    // the image refuses to start every job, and the first person to meet it will
    // be told that FLINK_PROPERTIES is broken when it is not.
    //
    // The descriptor's `parallelism` knob is deliberately NOT tied to these. It
    // is what the driver applies, and requiring it to match the image would mean
    // an image rebuild every time the published configuration changed — which is
    // exactly the coupling making the knob reachable was for.
    let entrants = entrant::load_all(&entrants_dir()).expect("descriptors valid");
    let flink = entrants
        .iter()
        .find(|e| e.id() == "flink")
        .expect("flink entrant");

    let config = std::fs::read_to_string(flink.dir.join("config.yaml")).expect("read config.yaml");
    let mut lines = config.lines().skip_while(|l| l.trim() != "parallelism:");
    lines.next().expect("config.yaml declares parallelism");
    let default: u32 = lines
        .find_map(|l| l.trim().strip_prefix("default:"))
        .and_then(|v| v.trim().parse().ok())
        .expect("config.yaml declares parallelism.default as an integer");

    let dockerfile =
        std::fs::read_to_string(flink.dir.join("Dockerfile")).expect("read Dockerfile");
    let expected: u32 = dockerfile
        .lines()
        .find_map(|l| l.trim().strip_prefix("EXPECT_PARALLELISM="))
        .and_then(|v| v.trim_end_matches(" \\").trim().parse().ok())
        .expect("the Dockerfile sets EXPECT_PARALLELISM to an integer");

    assert_eq!(
        default, expected,
        "entrants/flink/config.yaml sets parallelism.default={default} but the \
         Dockerfile sets EXPECT_PARALLELISM={expected}. ComparisonJob compares the \
         two at job submission, so a hand-run container would refuse every job and \
         report it as a configuration-override failure that has not happened."
    );
}

#[test]
fn the_rust_toolchain_pin_matches_the_arm_image() {
    // Two files have to agree and neither is obviously authoritative: the
    // toolchain that runs the host gates, and the one inside the image that
    // actually builds the measured binary. Codegen moves throughput, so a silent
    // divergence would make the recorded toolchain wrong.
    let root = repo_root();
    let pin =
        std::fs::read_to_string(root.join("rust-toolchain.toml")).expect("rust-toolchain.toml");
    let channel = pin
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = "))
        .map(|v| v.trim().trim_matches('"').to_owned())
        .expect("channel in rust-toolchain.toml");

    let dockerfile =
        std::fs::read_to_string(root.join("entrants/spate/Dockerfile")).expect("arm Dockerfile");
    let from = dockerfile
        .lines()
        .find_map(|l| l.trim().strip_prefix("FROM rust:"))
        .map(|v| v.split('-').next().unwrap_or_default().to_owned())
        .expect("FROM rust:<version> in the arm Dockerfile");

    assert_eq!(
        channel, from,
        "rust-toolchain.toml pins {channel} but entrants/spate/Dockerfile builds on {from}"
    );
}

/// The `size:` values under each `process:` key, in file order, as MiB.
fn process_sizes(config: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut lines = config.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != "process:" {
            continue;
        }
        for next in lines.by_ref() {
            let t = next.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            if let Some(v) = t.strip_prefix("size:")
                && let Some(m) = mib(v.trim())
            {
                out.push(m);
            }
            break;
        }
    }
    out
}

/// `3900m`, `4g`, `960m` as MiB.
///
/// Delegates rather than parsing, because this used to be the fourth copy of the
/// suffix parser and the one that had diverged furthest: it returned MiB where
/// the others returned bytes, and rejected suffixes the descriptors are allowed
/// to use. A test whose own arithmetic disagrees with the code under test is not
/// a check.
fn mib(s: &str) -> Option<u64> {
    entrant::parse_memory(s).map(|bytes| bytes / (1024 * 1024))
}
