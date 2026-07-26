//! `HARNESS_VERSION` and the table that explains it cannot move apart.
//!
//! `harness_version` is hand-maintained rather than derived, deliberately: "did
//! this change move numbers?" is a judgement, and a content hash would answer yes
//! to every typo fix and shatter every comparability group in the archive. The
//! price of that judgement is that the constant carries no explanation of its
//! own. The table under `### Harness versions` in
//! `methodology/comparability.md` is the explanation, and that file says CI
//! asserts the two stay in step.
//!
//! This is that assertion. Without it the failure is silent and permanent: a bump
//! that lands without its row splits every existing result set — the site refuses
//! to draw records of different protocol versions on one axis — while leaving no
//! record anywhere of what changed. A reader then sees half the archive go
//! "not comparable" and has nothing to read that says why, and the person who
//! bumped it is the only one who ever knew. The table is not documentation of the
//! constant; it is the only place the reason exists.
//!
//! The reverse direction matters too, which is why contiguity is checked rather
//! than just the maximum: a row added for a version the constant never reached
//! describes a protocol nothing was ever measured under.

use std::path::{Path, PathBuf};

use spate_benchmark_harness::report::HARNESS_VERSION;

/// The heading the table lives under. Matched case-insensitively on a trimmed
/// line, so ordinary prose editing does not break the parse — but matched, not
/// searched for loosely: if this heading cannot be found the test fails rather
/// than passing over a document whose table has been deleted.
const HEADING: &str = "### Harness versions";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("harness/ has a parent")
        .to_path_buf()
}

fn methodology() -> String {
    let path = repo_root().join("methodology").join("comparability.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The version numbers in the table under [`HEADING`], in file order.
///
/// Tolerant of the things prose editing changes — cell padding, a different date
/// format, extra columns, the alignment markers in the separator row — and
/// intolerant of the one thing that would make this test worthless, which is
/// finding nothing and reporting success.
fn table_versions(doc: &str) -> Vec<u32> {
    let mut lines = doc
        .lines()
        .skip_while(|l| !l.trim().eq_ignore_ascii_case(HEADING));
    assert!(
        lines.next().is_some(),
        "methodology/comparability.md has no `{HEADING}` heading. That section is the record of \
         what each measurement protocol version changed, and this test cannot \
         check HARNESS_VERSION against a table that is not there. Restore it \
         rather than removing this test."
    );

    let mut versions = Vec::new();
    let mut in_table = false;
    for line in lines {
        let row = line.trim();
        // The next heading ends the section, whether or not a table was found.
        if row.starts_with('#') {
            break;
        }
        if !row.starts_with('|') {
            // Prose sits between the heading and the table; a blank line after
            // the table ends it.
            if in_table {
                break;
            }
            continue;
        }
        in_table = true;

        let cell = row
            .trim_matches('|')
            .split('|')
            .next()
            .unwrap_or_default()
            .trim();
        // The separator row, including any `:---:` alignment markers.
        if !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':') {
            continue;
        }
        if cell.eq_ignore_ascii_case("version") {
            continue;
        }
        match cell.parse::<u32>() {
            Ok(v) => versions.push(v),
            Err(e) => panic!(
                "the `{HEADING}` table in methodology/comparability.md has a row whose first cell \
                 is {cell:?}, which is not a version number ({e}). The first column \
                 is the protocol version and is compared against HARNESS_VERSION in \
                 harness/src/report.rs, so it has to be the bare integer."
            ),
        }
    }

    assert!(
        !versions.is_empty(),
        "found no version rows under `{HEADING}` in methodology/comparability.md. The table is \
         the only record of what each protocol version changed; an empty one means \
         it has been removed or reformatted past recognition."
    );
    versions
}

#[test]
fn the_methodology_table_records_the_current_harness_version() {
    let versions = table_versions(&methodology());
    let newest = *versions.iter().max().expect("a non-empty table");
    assert_eq!(
        newest, HARNESS_VERSION,
        "harness/src/report.rs sets HARNESS_VERSION = {HARNESS_VERSION}, but the \
         newest row under `{HEADING}` in methodology/comparability.md is {newest} (the table \
         lists {versions:?}).\n\
         \n\
         If the constant was just bumped: add its row to that table in the same \
         commit, giving the number, the date, and what about the protocol moved \
         the numbers. A bump splits every existing result set — records of \
         different protocol versions are never drawn on one axis — and that row is \
         the only place a reader can find out why half the archive stopped being \
         comparable.\n\
         \n\
         If the table is ahead instead, the row describes a protocol nothing has \
         ever been measured under: either bump the constant or drop the row."
    );
}

#[test]
fn every_protocol_version_has_exactly_one_row() {
    let versions = table_versions(&methodology());
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    let expected: Vec<u32> = (1..=u32::try_from(versions.len()).expect("a short table")).collect();
    assert_eq!(
        sorted,
        expected,
        "the `{HEADING}` table in methodology/comparability.md lists {versions:?}, which is not \
         the contiguous run 1..={} it has to be. A gap means a bump landed without \
         its row and the change it was made for is now unrecorded; a repeat means \
         two protocol changes are claiming one comparability group, and every \
         record written under either is indistinguishable from the other.",
        versions.len()
    );
}
