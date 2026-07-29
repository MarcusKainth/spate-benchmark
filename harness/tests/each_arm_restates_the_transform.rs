//! No arm may take its transform from the oracle that marks it.
//!
//! This suite is run by the author of one of the systems in it, and the
//! correctness gate is what proves every arm did the same arithmetic. That proof
//! is only worth something if it can fail for *any* arm.
//!
//! It could not. `entrants/spate/src/rows.rs` imported `ascii_upper`,
//! `value_scaled_of`, `DROP_UNIT` and `QUALITY_FLOOR` from
//! `spate_benchmark_harness::corpus` — the module that computes the closed-form
//! expectations the gate checks. The vendor's arm and the marking scheme
//! therefore could not disagree by construction, while the Flink arm, which
//! reimplements all four in Java, could. Change the oracle's uppercase to
//! something Unicode-aware and the Spate arm follows it silently while Flink
//! fails a gate it should have passed; or, for a column the gate does not cover,
//! both pass while writing different data. The guarantee was one-sided in the
//! author's favour.
//!
//! `methodology/` settles whose job the transform is: pipeline logic — the
//! flatten, the filters and the derived columns are user code in every
//! system, and every arm writes them.
//!
//! What IS shared, deliberately, is the wire contract alone: the
//! registry-served `sensor_batch.avsc` that every arm decodes. A schema is not
//! the transform, and two hand-kept copies of it would be two things to keep in
//! step for no gain in fairness.
//!
//! # Why this is checked as source text
//!
//! The property is the **absence of a dependency edge**, and a test that
//! exercised behaviour could not see it: the two implementations agree today, and
//! that they agree is exactly the state the defect produced. A behavioural test
//! would have passed on the broken code. So the edge is checked directly, in the
//! only place it exists — the arm's own source.

use std::path::{Path, PathBuf};

/// The transform each arm must own. Not `SensorBatch`, which is the Avro wire
/// contract every arm reads off the same registry, and not the test-only
/// fixtures: a test comparing an arm against the oracle is the point.
const TRANSFORM: [&str; 4] = [
    "ascii_upper",
    "value_scaled_of",
    "DROP_UNIT",
    "QUALITY_FLOOR",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("harness/ has a parent")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Everything before `#[cfg(test)]`, which is the code that actually runs in a
/// measurement. A fixture import below that line is legitimate and must not fail
/// this test.
fn production_source(src: &str) -> &str {
    src.split_once("#[cfg(test)]").map_or(src, |(head, _)| head)
}

#[test]
fn the_spate_arm_does_not_import_its_transform_from_the_oracle() {
    for file in ["entrants/spate/src/rows.rs", "entrants/spate/src/main.rs"] {
        let src = read(file);
        let production = production_source(&src);
        for item in TRANSFORM {
            // Matched against the import, not against any mention: the arm names
            // these things — it defines them — and the defect was where they
            // came from.
            for line in production.lines().filter(|l| l.contains("use ")) {
                assert!(
                    !(line.contains("corpus") && line.contains(item)),
                    "{file} imports {item} from the oracle:\n  {}\n\nThe module that \
                     computes the gate's expectations must not also supply an arm's \
                     transform: an arm that cannot disagree with the marking scheme is \
                     not being marked. Implement it in the arm, as the Flink arm does, \
                     and let this test hold the constants to the workload.",
                    line.trim()
                );
            }
        }
    }
}

#[test]
fn every_arm_restates_the_drop_unit_the_workload_specifies() {
    // The VALUE is specification: it comes from `workload/workload.toml`, which
    // `build.rs` hashes into `dataset_version`. Restating it in each arm is what
    // makes a change to the specification fail loudly in every arm at once,
    // instead of flowing silently into whichever one imported it.
    let spec = spate_benchmark_harness::corpus::DROP_UNIT;

    let spate = read("entrants/spate/src/rows.rs");
    assert!(
        spate.contains(&format!("const DROP_UNIT: &str = {spec:?};")),
        "the Spate arm must restate DROP_UNIT as {spec:?}; the workload specifies it \
         and `build.rs` derives the constant this test compares against"
    );

    let flink = read("entrants/flink/src/main/java/dev/kainth/spatebench/flink/Rows.java");
    assert!(
        flink.contains(&format!("DROP_UNIT = {spec:?}")),
        "the Flink arm must restate DROP_UNIT as {spec:?}"
    );
}

#[test]
fn every_arm_restates_the_quality_floor_the_workload_specifies() {
    let spec = spate_benchmark_harness::corpus::QUALITY_FLOOR;

    let spate = read("entrants/spate/src/rows.rs");
    assert!(
        spate.contains(&format!("const QUALITY_FLOOR: f64 = {spec};")),
        "the Spate arm must restate QUALITY_FLOOR as {spec}"
    );

    // Java spells the same value `0.2d`. Accepted in either spelling rather than
    // pinned to one, because which suffix a Java author writes is not a fact
    // about the workload.
    let flink = read("entrants/flink/src/main/java/dev/kainth/spatebench/flink/Rows.java");
    assert!(
        flink.contains(&format!("QUALITY_FLOOR = {spec}d"))
            || flink.contains(&format!("QUALITY_FLOOR = {spec};")),
        "the Flink arm must restate QUALITY_FLOOR as {spec}"
    );
}
