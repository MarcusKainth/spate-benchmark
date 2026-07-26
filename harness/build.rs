//! Generates the corpus constants and derives `dataset_version`.
//!
//! Two jobs, and they are the same job seen from two sides.
//!
//! 1. `workload/workload.toml` is the single source of truth for the generator's
//!    tunables. They are emitted as Rust constants here rather than written in
//!    both places, because a duplicate drifts and a drifted corpus constant is
//!    invisible until two result sets disagree for no stated reason.
//!
//! 2. `dataset_version` is a hash of everything that jointly determines what the
//!    data *is* — the generator's tunables, the generator's arithmetic, the Avro
//!    schema, and the target DDL. Deriving it rather than hand-maintaining it
//!    means a change to the corpus cannot be made without the version moving,
//!    which is what stops the site placing pre-change and post-change records on
//!    one axis.
//!
//! Contrast `HARNESS_VERSION`, which is hand-maintained on purpose: "did this
//! change move numbers?" is a judgement no hash can make.
//!
//! # Why each input is hashed differently
//!
//! A derived version has two failure modes, and this one had both.
//!
//! **Too narrow.** The generator's arithmetic lives in `harness/src/corpus.rs` —
//! `batch_id * 31 + seq`, `batch_id.wrapping_mul(1_000_003) + seq * 97`, the
//! `% 10` null-region rule, the tag-length rule. None of it was hashed and the
//! file was not in `rerun-if-changed`, so changing `1_000_003` to `1_000_033`
//! changed every value in the corpus while `DATASET_VERSION` stood still. New
//! records then landed in the same comparability group as old ones and were
//! medianed together with them.
//!
//! **Too broad.** Correcting a stale *comment* in `workload/clickhouse/ddl.sql`
//! moved the version from `d1-9aeec63b4931` to `d1-e396b1c7696b`. A prose fix
//! re-versioned the corpus and would have split every published record from the
//! tree — which does not make the comment get fixed, it makes it get left.
//!
//! Both failures came from one decision: hashing raw bytes uniformly. The rule
//! here is per-input instead, and it is the same rule each time — **hash each
//! file at the granularity at which its consumer can observe it.**
//!
//! | Input | Consumer | Hashed as |
//! |---|---|---|
//! | `workload.toml` | this build script | parsed values, canonically rendered |
//! | `sensor_batch.avsc` | the Schema Registry | raw bytes |
//! | `clickhouse/ddl.sql` | ClickHouse | statements with comments stripped and whitespace collapsed |
//! | `src/corpus.rs` | `rustc` | the marked generator region, comments stripped and whitespace collapsed |
//!
//! So `quality_floor = 0.2` and `quality_floor = 2e-1` are one corpus, a
//! rewrapped `CREATE TABLE` is one corpus, a rustfmt upgrade is one corpus — and
//! a changed modulus, a changed column type or a changed unit list is not.
//!
//! # The byte-hashing argument, and where it survives
//!
//! The previous version of this file justified hashing bytes with: "a consumer
//! that reads these files verbatim (the Flink arm copies the .avsc into its jar)
//! sees the bytes, so the bytes are what identifies the corpus."
//!
//! That does not survive as stated. The Flink arm *parses* those bytes, and two
//! `.avsc` files differing only in a `doc` string parse to schemas that read and
//! write byte-identical datums — Avro's own parsing canonical form drops `doc`
//! for precisely that reason. No consumer of any of these four files treats it as
//! opaque. The argument bought a demonstrated false positive (the DDL comment)
//! and never a true one, and while it was being made the file that actually
//! determines every byte on the wire was not hashed at all.
//!
//! The `.avsc` is nevertheless still hashed verbatim, for a better reason than
//! the one it was given. The registry, not the Flink arm, is what makes its text
//! load-bearing: `corpus::register_schema` POSTs the file and gets back an id, a
//! differing text is a new subject version with a new id, and that id is bytes
//! 1..5 of **every message in the corpus**. A doc-only edit to the schema really
//! does change the corpus bytes, so the version really must move. Nothing else
//! here has a consumer with that property — ClickHouse never sees a DDL comment,
//! this script never sees a TOML comment, and `rustc` never sees a Rust comment.
//!
//! A practical consequence, recorded because it looks like an oversight and is
//! not: `sensor_batch.avsc` carries a `doc` string naming the fairness contract
//! as `METHODOLOGY.md`, which is now `methodology/`. **Correcting it would move
//! `dataset_version` and orphan every committed record**, for the reason above —
//! so it is deliberately left stale until a change to the corpus has to move the
//! version anyway, and should be fixed in that commit.
//!
//! One consequence worth stating because `.gitattributes` argues from it: LF
//! pinning is now load-bearing for the `.avsc` alone. The other three are
//! normalised through `str::lines` and `split_whitespace`, so a CRLF checkout
//! cannot move their contribution — which was the near-miss `.gitattributes`
//! records for `workload.toml`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Prefix on the derived version, so the hash function can change later without
/// old identifiers becoming ambiguous.
///
/// `d1` was the uniform byte hash over three files. `d2` is the per-input
/// semantic hash over four described above. The two answer different questions
/// and a `d1` identifier must never be read as comparable with a `d2` one, which
/// is the whole job of this prefix.
const DATASET_PREFIX: &str = "d2";

/// Marker opening the corpus-defining region of `harness/src/corpus.rs`.
const CORPUS_BEGIN: &str = "// dataset-version:begin";
/// Marker closing it.
const CORPUS_END: &str = "// dataset-version:end";

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let workload = workload_dir(&manifest);

    let toml_path = workload.join("workload.toml");
    let avsc_path = workload.join("schema/sensor_batch.avsc");
    let ddl_path = workload.join("clickhouse/ddl.sql");
    // In the same crate, so cargo already rebuilds when it changes — but the
    // build script does not re-run on a source edit unless it says so, and a
    // build script that keeps emitting a stale `DATASET_VERSION` is exactly the
    // "too narrow" failure this file exists to close.
    let corpus_path = manifest.join("src/corpus.rs");

    for p in [&toml_path, &avsc_path, &ddl_path, &corpus_path] {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    println!("cargo:rerun-if-changed=build.rs");

    let toml_src = read(&toml_path);
    let avsc_src = read(&avsc_path);
    let ddl_src = read(&ddl_path);
    let corpus_src = read(&corpus_path);

    // `toml::from_str`, not `str::parse`: since toml 0.9 the `FromStr` impl on
    // `Value` parses a single value expression rather than a whole document, so
    // `parse()` rejects the file at its first comment.
    let spec: toml::Value =
        toml::from_str(&toml_src).unwrap_or_else(|e| panic!("workload.toml does not parse: {e}"));
    let generator = spec
        .get("generator")
        .unwrap_or_else(|| panic!("workload.toml has no [generator] table"));

    let events_per_batch = int(generator, "events_per_batch");
    let sensors = int(generator, "sensors");
    let names = int(generator, "names");
    let tags = int(generator, "tags");
    let base_ts_ms = int(generator, "base_ts_ms");
    let quality_floor = float(generator, "quality_floor");
    let drop_unit = string(generator, "drop_unit");
    let units: Vec<String> = generator
        .get("units")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("workload.toml: generator.units must be an array"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("workload.toml: generator.units must be strings"))
                .to_owned()
        })
        .collect();

    // The tier-B filter sentinel has to be a unit that actually occurs, or the
    // transform is a no-op and tier B silently stops testing anything.
    assert!(
        units.iter().any(|u| u == &drop_unit),
        "workload.toml: drop_unit {drop_unit:?} is not present in units {units:?}"
    );
    assert!(
        (0.0..=1.0).contains(&quality_floor),
        "workload.toml: quality_floor {quality_floor} is outside 0.0..=1.0"
    );
    assert!(
        events_per_batch > 0,
        "workload.toml: events_per_batch must be positive"
    );

    let mut out = String::new();
    let unit_list = units
        .iter()
        .map(|u| format!("{u:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        out,
        "// @generated by harness/build.rs from workload/workload.toml — do not edit."
    )
    .unwrap();
    writeln!(
        out,
        "/// Events per message, and therefore the fan-out factor."
    )
    .unwrap();
    writeln!(out, "pub const EVENTS_PER_BATCH: u32 = {events_per_batch};").unwrap();
    writeln!(out, "/// Distinct `sensor` values.").unwrap();
    writeln!(out, "pub const SENSORS: u64 = {sensors};").unwrap();
    writeln!(out, "/// Distinct metric names.").unwrap();
    writeln!(out, "pub const NAMES: u64 = {names};").unwrap();
    writeln!(out, "/// Distinct tag values.").unwrap();
    writeln!(out, "pub const TAGS: u64 = {tags};").unwrap();
    writeln!(out, "/// Units; one of them is the tier-B filter sentinel.").unwrap();
    writeln!(
        out,
        "pub const UNITS: [&str; {}] = [{unit_list}];",
        units.len()
    )
    .unwrap();
    writeln!(out, "/// The unit value tier B filters out.").unwrap();
    writeln!(out, "pub const DROP_UNIT: &str = {drop_unit:?};").unwrap();
    writeln!(
        out,
        "/// Tier B drops rows whose `quality` is non-null and below this."
    )
    .unwrap();
    writeln!(out, "pub const QUALITY_FLOOR: f64 = {quality_floor:?};").unwrap();
    writeln!(out, "/// Base event timestamp, epoch milliseconds.").unwrap();
    writeln!(out, "pub const BASE_TS_MS: i64 = {base_ts_ms};").unwrap();

    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    std::fs::write(out_dir.join("workload_consts.rs"), out).expect("write generated constants");

    // Hash each input at the granularity its consumer can observe; see the module
    // docs for why that granularity differs per input.
    let mut h = Sha256::new();
    absorb(&mut h, "workload.toml", canonical_toml(&spec).as_bytes());
    absorb(&mut h, "sensor_batch.avsc", avsc_src.as_bytes());
    absorb(
        &mut h,
        "clickhouse/ddl.sql",
        normalise_sql(&ddl_src).as_bytes(),
    );
    absorb(
        &mut h,
        "corpus.rs",
        normalise_rust(corpus_region(&corpus_src)).as_bytes(),
    );
    let digest = h.finalize();
    let short: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();

    println!("cargo:rustc-env=SPATE_BENCH_DATASET_VERSION={DATASET_PREFIX}-{short}");

    // Bake the compiler in, so `sut.toolchain` reports what actually built the
    // binary rather than whatever `rustc` happens to be on the host running the
    // driver. Those differ by design: the arm is built inside its image.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let version = std::process::Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SPATE_BENCH_RUSTC={version}");
}

/// Feed one labelled input into the digest, length-prefixed.
///
/// The label and the length are not decoration. Without them the digest is taken
/// over a bare concatenation, so moving a character from the end of one input to
/// the start of the next leaves it unchanged — a corpus change that the version
/// exists to make impossible would slip through the one mechanism meant to catch
/// it.
fn absorb(h: &mut Sha256, label: &str, bytes: &[u8]) {
    h.update(label.as_bytes());
    h.update([0u8]);
    h.update(
        u64::try_from(bytes.len())
            .expect("input length fits u64")
            .to_le_bytes(),
    );
    h.update(bytes);
}

/// The region of `corpus.rs` that determines a byte of the corpus.
///
/// Delimited by markers rather than taken as the whole file, because that file
/// also holds the Kafka producer, the prefill loop and the correctness gates —
/// none of which change what the data is. Hashing those would put the version
/// back in the "too broad" failure mode from a different direction: retuning
/// `linger.ms` would re-version a byte-identical corpus and split the tree.
///
/// # Panics
/// If either marker is missing, duplicated, out of order, or encloses almost
/// nothing. Silently narrowing the hash to an empty region is the one failure
/// this must not have, because it would look exactly like a working build.
fn corpus_region(src: &str) -> &str {
    assert_eq!(
        src.matches(CORPUS_BEGIN).count(),
        1,
        "corpus.rs must carry exactly one {CORPUS_BEGIN} marker"
    );
    assert_eq!(
        src.matches(CORPUS_END).count(),
        1,
        "corpus.rs must carry exactly one {CORPUS_END} marker"
    );
    let begin = src.find(CORPUS_BEGIN).expect("begin marker located") + CORPUS_BEGIN.len();
    let end = src.find(CORPUS_END).expect("end marker located");
    assert!(
        begin < end,
        "corpus.rs closes the dataset-version region before it opens it"
    );
    let region = &src[begin..end];
    assert!(
        region.len() > 1000,
        "the dataset-version region of corpus.rs is {} bytes, which is too little to be the \
         generator — a marker has probably been moved",
        region.len()
    );
    region
}

/// `workload.toml` reduced to the values this script reads out of it.
///
/// One line per leaf, `path`-tab-`type`-tab-`value`, sorted. Comments and layout
/// vanish, which is the point — the file's own header argues that the constants
/// live in a data file so the version tracks "what the data actually is", and a
/// comment is not that. So does spelling: `1772000000000` and `1_772_000_000_000`
/// are one number and must be one corpus.
///
/// Rendered rather than re-serialised through `toml` so that the encoding is
/// stated here in full and cannot move under a dependency upgrade. Strings are
/// emitted raw and the separators are the two characters a value may not contain,
/// which removes escaping from the definition entirely.
///
/// # Panics
/// If a string value contains a tab or a newline, or if a datetime appears — a
/// corpus keyed on a wall-clock literal would not be reproducible anyway.
fn canonical_toml(v: &toml::Value) -> String {
    let mut lines = Vec::new();
    flatten_toml("", v, &mut lines);
    lines.sort();
    lines.concat()
}

fn flatten_toml(path: &str, v: &toml::Value, out: &mut Vec<String>) {
    let child = |k: &str| {
        if path.is_empty() {
            k.to_owned()
        } else {
            format!("{path}.{k}")
        }
    };
    match v {
        toml::Value::Table(t) => {
            for (k, val) in t {
                flatten_toml(&child(k), val, out);
            }
        }
        toml::Value::Array(a) => {
            for (i, val) in a.iter().enumerate() {
                flatten_toml(&child(&i.to_string()), val, out);
            }
        }
        toml::Value::String(s) => {
            assert!(
                !s.contains(['\t', '\n']),
                "workload.toml: {path} contains a tab or newline, which the canonical form uses \
                 as separators"
            );
            out.push(format!("{path}\ts\t{s}\n"));
        }
        toml::Value::Integer(i) => out.push(format!("{path}\ti\t{i}\n")),
        toml::Value::Float(f) => out.push(format!("{path}\tf\t{f:?}\n")),
        toml::Value::Boolean(b) => out.push(format!("{path}\tb\t{b}\n")),
        toml::Value::Datetime(d) => panic!(
            "workload.toml: {path} is a datetime ({d}); the canonical form has no rendering for \
             one, and a corpus keyed on wall-clock time would not be reproducible"
        ),
    }
}

/// The DDL reduced to what ClickHouse is actually told.
///
/// Line comments go first, then whitespace collapses. This is the exact defect
/// that made a prose fix cost a dataset: ClickHouse stores a parsed table
/// definition and has never seen a `--`, so a corrected comment changes nothing
/// about where any arm's rows land.
///
/// Comments are stripped by the same rule `corpus::split_sql` uses to decide what
/// to execute, deliberately: what is hashed is then what the driver runs. It
/// shares that rule's one hazard — a `--` inside a string literal would truncate
/// the statement — and the committed DDL has no string literals outside its
/// comments.
fn normalise_sql(src: &str) -> String {
    let stripped: String = src
        .lines()
        .map(|line| line.split_once("--").map_or(line, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    collapse_whitespace(&stripped)
}

/// Rust source reduced to what the compiler acts on.
///
/// Comments go because a corrected explanation is not a corrected corpus — the
/// same rule the DDL gets, applied to the file that actually produces the bytes.
/// Whitespace collapses because rewrapping a line is not a change either, and
/// rustfmt's output moves with rustfmt's version: a toolchain upgrade must not
/// re-version the corpus.
///
/// This is a quote counter, not a Rust lexer, and it refuses rather than guesses.
/// Block comments are rejected outright, and a line whose `"` do not balance is
/// rejected too — which is what catches a raw string with an embedded quote, or a
/// `'"'` character literal, either of which would desynchronise the scan and hash
/// the wrong thing silently and forever.
///
/// # Panics
/// If the region contains a block comment, or a line the scanner cannot read.
fn normalise_rust(src: &str) -> String {
    assert!(
        !src.contains("/*"),
        "the dataset-version region of corpus.rs must not use block comments; this normaliser \
         only strips `//` line comments"
    );
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let (code, balanced) = strip_rust_line_comment(line);
        assert!(
            balanced,
            "the dataset-version region of corpus.rs has unbalanced `\"` on {line:?}; the \
             normaliser cannot tell code from comment there"
        );
        out.push_str(code);
        out.push('\n');
    }
    collapse_whitespace(&out)
}

/// Returns the code part of one line, and whether its string literals closed.
fn strip_rust_line_comment(line: &str) -> (&str, bool) {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'/' if !in_string && bytes.get(i + 1) == Some(&b'/') => return (&line[..i], true),
            _ => {}
        }
        i += 1;
    }
    (line, !in_string)
}

/// Collapse every run of whitespace to a single space, and trim.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Locates `workload/` relative to this crate, so the build works from any cwd.
fn workload_dir(manifest: &Path) -> PathBuf {
    let dir = manifest
        .parent()
        .unwrap_or_else(|| panic!("harness/ has a parent"))
        .join("workload");
    assert!(dir.is_dir(), "workload/ not found at {}", dir.display());
    dir
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn int(t: &toml::Value, key: &str) -> i64 {
    t.get(key)
        .and_then(toml::Value::as_integer)
        .unwrap_or_else(|| panic!("workload.toml: generator.{key} must be an integer"))
}

fn float(t: &toml::Value, key: &str) -> f64 {
    t.get(key)
        .and_then(toml::Value::as_float)
        .unwrap_or_else(|| panic!("workload.toml: generator.{key} must be a float"))
}

fn string(t: &toml::Value, key: &str) -> String {
    t.get(key)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("workload.toml: generator.{key} must be a string"))
        .to_owned()
}
