//! The corpus is pinned by fingerprint, not merely by round-trip.
//!
//! Every other corpus test proves the generator agrees with *itself* — that
//! `encode_batch` and the typed decoder are mutual inverses, that the flatten
//! agrees with the closed-form expectations. None of them would notice if the
//! whole corpus shifted, because they would shift with it. A published
//! comparison cannot afford that: the corpus is the thing every arm is measured
//! against, and if it changes silently then two result sets are being compared
//! across different inputs while claiming to be like-for-like.
//!
//! So these values are absolute. They were taken from the pre-extraction harness
//! (`etl-rs/benchmarks/src/comparison_data.rs` at `f41280d51165`) and asserted
//! here unchanged, which is what proves the move between repositories did not
//! alter a single byte of what is being measured.
//!
//! **If one of these fails, do not update the constant to match.** Either the
//! generator changed — in which case `workload/workload.toml` or the schema
//! moved and `dataset_version` must move with it, splitting the comparability
//! group — or something changed that should not have. Both need a human.

use spate_benchmark_harness::corpus::{Tier, encode_batch, expected, frame_confluent};

/// FNV-1a, inline. A dependency-free fingerprint: the point is stability across
/// versions of this repository, and a hash crate that changed its output would
/// be a false alarm rather than a finding.
fn fnv(seed: u64, bytes: &[u8]) -> u64 {
    let mut h = seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// Batches fingerprinted. Enough to exercise every branch of the derivations —
/// the null-region every tenth batch, the null-quality every fifth event, all
/// eight units, all four tag-array lengths — without making the test slow.
const BATCHES: u64 = 1000;

#[test]
fn encoded_datums_are_byte_identical_to_the_original_harness() {
    let mut h = FNV_OFFSET;
    let mut total = 0usize;
    for id in 0..BATCHES {
        let b = encode_batch(
            id,
            1_772_000_000_000_000_i64 + i64::try_from(id).expect("fits i64"),
        );
        total += b.len();
        h = fnv(h, &b);
    }
    assert_eq!(
        format!("{h:016x}"),
        "5c66e4254fe9b472",
        "the Avro encoding of the corpus changed"
    );
    assert_eq!(total, 4_051_124, "the corpus changed size");
}

#[test]
fn confluent_framing_is_byte_identical_to_the_original_harness() {
    // Framing is checked separately from the datum because it is what every
    // competitor actually reads off the wire: a header regression would leave
    // the datums correct and every arm unable to decode them.
    let mut h = FNV_OFFSET;
    for id in 0..BATCHES {
        h = fnv(
            h,
            &frame_confluent(42, &encode_batch(id, i64::try_from(id).expect("fits i64"))),
        );
    }
    assert_eq!(
        format!("{h:016x}"),
        "5e3ced4c090971ea",
        "the Confluent framing of the corpus changed"
    );
}

#[test]
fn closed_form_expectations_are_unchanged() {
    // These are what the correctness gates compare against, so a drift here
    // would not fail a run — it would silently redefine what "correct" means.
    let a = expected(BATCHES, Tier::A);
    assert_eq!(a.rows, 100_000);
    assert_eq!(a.value_sum, 49_950_630_000_000);
    assert_eq!(a.value_scaled_sum, 0, "tier A derives no scaled column");

    let b = expected(BATCHES, Tier::B);
    assert_eq!(b.rows, 73_500);
    assert_eq!(b.value_sum, 36_712_213_045_500);
    assert_eq!(b.value_scaled_sum, 1_903_970_758_089_327);
}

#[test]
fn the_dataset_version_matches_the_committed_workload() {
    // Pinned so that editing the schema, the DDL or the generator constants
    // without noticing is impossible: this test fails and points at the change.
    // Updating it is correct — but it must happen in the same commit as the
    // corpus change, which is the review artefact worth having.
    assert_eq!(
        spate_benchmark_harness::report::DATASET_VERSION,
        "d1-9aeec63b4931"
    );
}
