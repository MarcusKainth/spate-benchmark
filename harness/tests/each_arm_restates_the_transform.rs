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
//!
//! # One table, every arm
//!
//! The checks are driven by [`ARMS`] rather than written out per entrant,
//! because the failure mode of the per-entrant shape was demonstrated by this
//! very file: it named spate and flink literally, so a third arm's transform
//! would simply have gone unchecked until somebody remembered this test existed.
//! An arm is added by adding a row, and
//! [`every_active_arm_has_a_row_in_this_table`] is what makes forgetting the row
//! loud.
//!
//! Config-only arms are rows too. A VRL program, a materialized view's SQL and
//! a Java class restate the constants in different spellings, so each row states
//! the exact substrings its artifact must contain — derived from the oracle's
//! value, never written as a second literal here.

use std::path::{Path, PathBuf};

use spate_benchmark_harness::{corpus, entrant};

/// The oracle items no Rust arm may import. Not `SensorBatch`, which is the
/// Avro wire contract every arm reads off the same registry, and not the
/// test-only fixtures: a test comparing an arm against the oracle is the point.
const TRANSFORM: [&str; 4] = [
    "ascii_upper",
    "value_scaled_of",
    "DROP_UNIT",
    "QUALITY_FLOOR",
];

/// One transform artifact and how it must restate the workload's constants.
struct Arm {
    /// The entrant the artifact belongs to. Every active entrant must appear on
    /// at least one row.
    entrant: &'static str,
    /// Repo-relative file holding (part of) the transform.
    file: &'static str,
    /// The exact substrings this file must contain, derived from the oracle's
    /// value. More than one entry means "any of these spellings", because how a
    /// language spells a literal is not a fact about the workload. Empty means
    /// this file carries no constants (it is listed for the oracle check only);
    /// [`every_row_in_this_table_checks_something`] is what stops that escape
    /// from producing a row that checks nothing at all.
    drop_unit: fn(&str) -> Vec<String>,
    /// As `drop_unit`, for the quality floor.
    quality_floor: fn(f64) -> Vec<String>,
    /// Whether the file is Rust that could reach the oracle through `use`.
    /// Only a Rust arm links against the harness crate; a Java, SQL or VRL
    /// artifact has no import edge to check.
    rust_oracle_check: bool,
}

/// Every arm's transform artifacts. A new arm adds its row(s) here in the PR
/// that activates it; [`every_active_arm_has_a_row_in_this_table`] is what makes
/// omitting them a failure rather than a gap.
const ARMS: [Arm; 6] = [
    Arm {
        entrant: "spate",
        file: "entrants/spate/src/rows.rs",
        drop_unit: |spec| vec![format!("const DROP_UNIT: &str = {spec:?};")],
        quality_floor: |spec| vec![format!("const QUALITY_FLOOR: f64 = {spec};")],
        rust_oracle_check: true,
    },
    Arm {
        // The pipeline entry point must not import the oracle either; the
        // constants themselves live in rows.rs.
        entrant: "spate",
        file: "entrants/spate/src/main.rs",
        drop_unit: |_| Vec::new(),
        quality_floor: |_| Vec::new(),
        rust_oracle_check: true,
    },
    Arm {
        entrant: "flink",
        file: "entrants/flink/src/main/java/dev/kainth/spatebench/flink/Rows.java",
        drop_unit: |spec| vec![format!("DROP_UNIT = {spec:?}")],
        // Java spells the same value `0.2d`. Accepted in either spelling rather
        // than pinned to one, because which suffix a Java author writes is not
        // a fact about the workload.
        quality_floor: |spec| {
            vec![
                format!("QUALITY_FLOOR = {spec}d"),
                format!("QUALITY_FLOOR = {spec};"),
            ]
        },
        rust_oracle_check: false,
    },
    Arm {
        // A config-only arm: the transform is a VRL program, compiled by Vector
        // from this committed file. Both needles are bounded on the right by a
        // delimiter the VRL text supplies — the sentinel's closing quote, the
        // floor's closing paren — so a constant that merely EXTENDS the spec's
        // spelling (a 0.25 floor against a 0.2 spec) cannot ride a prefix
        // match. The file's comments spell neither needle, deliberately: the
        // match is substring over the whole file, and prose containing one
        // would hold this test green while the code drifted.
        // Not Rust — VRL has no import edge to the oracle to check.
        entrant: "vector",
        file: "entrants/vector/transform.vrl",
        drop_unit: |spec| vec![format!("!= {spec:?}")],
        quality_floor: |spec| vec![format!(">= {spec})")],
        rust_oracle_check: false,
    },
    Arm {
        // A config-only arm: the transform is the materialized view's SQL,
        // applied by the per-entrant DDL hook. SQL spells the filters as
        // predicates, so the substrings held to the oracle are the comparison
        // operator plus the literal.
        entrant: "kafka-connect",
        file: "entrants/kafka-connect/clickhouse/arm.sql",
        drop_unit: |spec| vec![format!("!= '{spec}'")],
        quality_floor: |spec| vec![format!(">= {spec}")],
        rust_oracle_check: false,
    },
    Arm {
        // A config-only arm: the transform is a materialized view's SQL, and
        // the constants appear as ClickHouse literals — the filter predicates
        // in the MV's WHERE clause.
        entrant: "clickhouse-kafka-engine",
        file: "entrants/clickhouse-kafka-engine/initdb/10_ddl.sql",
        drop_unit: |spec| vec![format!("e.unit != '{spec}'")],
        quality_floor: |spec| vec![format!("e.quality < {spec}")],
        rust_oracle_check: false,
    },
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
fn no_rust_arm_imports_its_transform_from_the_oracle() {
    for arm in ARMS.iter().filter(|a| a.rust_oracle_check) {
        let src = read(arm.file);
        let production = production_source(&src);
        for item in TRANSFORM {
            // Matched against the import, not against any mention: the arm names
            // these things — it defines them — and the defect was where they
            // came from.
            for line in production.lines().filter(|l| l.contains("use ")) {
                assert!(
                    !(line.contains("corpus") && line.contains(item)),
                    "{} imports {item} from the oracle:\n  {}\n\nThe module that \
                     computes the gate's expectations must not also supply an arm's \
                     transform: an arm that cannot disagree with the marking scheme is \
                     not being marked. Implement it in the arm, as the Flink arm does, \
                     and let this test hold the constants to the workload.",
                    arm.file,
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
    let spec = corpus::DROP_UNIT;
    for arm in &ARMS {
        let wanted = (arm.drop_unit)(spec);
        if wanted.is_empty() {
            continue;
        }
        let src = read(arm.file);
        assert!(
            wanted.iter().any(|w| src.contains(w)),
            "the {} arm must restate the drop unit in {} as one of {wanted:?}; the \
             workload specifies it and `build.rs` derives the constant this test \
             compares against",
            arm.entrant,
            arm.file
        );
    }
}

#[test]
fn every_arm_restates_the_quality_floor_the_workload_specifies() {
    let spec = corpus::QUALITY_FLOOR;
    for arm in &ARMS {
        let wanted = (arm.quality_floor)(spec);
        if wanted.is_empty() {
            continue;
        }
        let src = read(arm.file);
        assert!(
            wanted.iter().any(|w| src.contains(w)),
            "the {} arm must restate the quality floor in {} as one of {wanted:?}",
            arm.entrant,
            arm.file
        );
    }
}

/// The rule that keeps the completeness rule honest: having a row is only
/// evidence if the row does something. The empty-vec escape on the extractors
/// is legitimate — `entrants/spate/src/main.rs` carries no constants and is
/// listed for the oracle check alone — but it composes into a trap: a row with
/// empty extractors AND `rust_oracle_check: false` satisfies
/// [`every_active_arm_has_a_row_in_this_table`] while asserting nothing at
/// all. So every row must exercise at least one of the three checks.
#[test]
fn every_row_in_this_table_checks_something() {
    // The extractors are called with the real oracle values, exactly as the
    // restatement tests call them, so "empty" here means empty for the specs
    // that actually run — not for some probe input.
    for arm in &ARMS {
        let checks_a_constant = !(arm.drop_unit)(corpus::DROP_UNIT).is_empty()
            || !(arm.quality_floor)(corpus::QUALITY_FLOOR).is_empty();
        assert!(
            checks_a_constant || arm.rust_oracle_check,
            "the {} row for {} has empty drop_unit and quality_floor extractors \
             and rust_oracle_check: false — it checks nothing. A row that checks \
             nothing is worse than a missing row: a missing row fails \
             every_active_arm_has_a_row_in_this_table, but this one passes it and \
             reads as coverage. Give the row a constant to restate or an oracle \
             edge to check, or delete it.",
            arm.entrant,
            arm.file
        );
    }
}

/// The rule that makes the table complete: an arm cannot become `active`
/// without a row here, so a new system's transform is checked from its first
/// green build rather than from whenever somebody remembers this file.
#[test]
fn every_active_arm_has_a_row_in_this_table() {
    let entrants = entrant::load_all(&repo_root().join("entrants")).expect("descriptors are valid");
    for e in entrants {
        if e.spec.entrant.status != entrant::Status::Active {
            continue;
        }
        assert!(
            ARMS.iter().any(|a| a.entrant == e.id()),
            "{} is active but has no row in ARMS: its transform restatement is \
             unchecked. Add a row naming the file that restates DROP_UNIT and \
             QUALITY_FLOOR (a VRL program and a materialized view's SQL are rows \
             too), in the same PR that activates the arm.",
            e.id()
        );
    }
}
