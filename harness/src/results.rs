//! Writing results, and the one thing this module deliberately cannot do.
//!
//! **No path in this module rewrites a results file.** [`append`] is the only
//! writer, it opens with `append(true)` and no `truncate`, and there is no
//! guarded or flag-protected alternative beside it — the capability does not
//! exist. Retention of published numbers is enforced by the absence of the
//! operation rather than by anyone remembering not to use it, because the
//! previous runner script opened its results file with `>` and silently replaced
//! every earlier run on each invocation.
//!
//! A number later found to be wrong is corrected by editing the archive in a
//! commit of its own, so the change is visible in the repository's history
//! rather than in a marker the site has to render.
//!
//! Layout is `results/<env_id>/<entrant>/<YYYY-MM>.jsonl`. Two properties follow,
//! and both matter at scale:
//!
//! - **Partial re-runs are conflict-free by construction.** Re-running one
//!   system touches exactly one file, disjoint from every other system's.
//! - **No directory grows without bound.** ClickBench's result store reached
//!   31,453 files with 27,599 in a single directory, forcing an ARG_MAX
//!   workaround into its build script. Here width is bounded by the entrant
//!   count and by twelve months.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::report::Report;

/// Where records that must never be published are written instead.
///
/// Beside `results/` and not inside it, so that no part of the archive has to be
/// filtered to be trusted. See [`root_for`].
pub const TUNING_DIR: &str = "tuning";

/// The archive a record produced under `trigger` belongs in.
///
/// A tuning sweep produces dozens of real measurements that must never reach a
/// reader, and the obvious arrangement — append them to `results/` and let
/// `validate::results_are_valid` refuse the file afterwards — is worse than it
/// looks. The layout partitions by `(env_id, entrant, month)`, so those records
/// would land in the *same file* as the arm's published ones, and clearing them
/// would mean hand-deleting fifty lines out of a file containing numbers that
/// are published. This module deliberately has no capability to rewrite a
/// results file, so that hand-edit would happen in an editor, unreviewed, on the
/// one file where a slip is unrecoverable.
///
/// Two smaller consequences make the separation worth having on its own.
/// [`load_all`] reads `results/`, so `bench stale` and `bench list` would
/// otherwise report an arm as freshly measured on the strength of a tuning run —
/// the arm would then never be re-measured at the configuration it publishes.
/// And the site's loader walks `results/` too.
///
/// The validator's refusal stays, and is not made redundant by this: a record
/// can still reach `results/` by a file being moved, by a hand-written line, or
/// by a future writer that forgets to ask. Routing decides where the harness
/// puts one; the refusal decides what the archive is allowed to contain.
#[must_use]
pub fn root_for(repo_root: &Path, trigger: crate::report::Trigger) -> PathBuf {
    if trigger.bars_publication() {
        repo_root.join(TUNING_DIR)
    } else {
        repo_root.join("results")
    }
}

/// The file a record belongs in.
///
/// `ts_ms` decides the month rather than the wall clock, so a record built just
/// before midnight on the last of the month lands where its own timestamp says
/// it does.
#[must_use]
pub fn path_for(root: &Path, env_id: &str, entrant: &str, ts_ms: u64) -> PathBuf {
    root.join(env_id)
        .join(entrant)
        .join(format!("{}.jsonl", year_month(ts_ms)))
}

/// Appends one record.
///
/// # Errors
///
/// If the directory cannot be created, the file cannot be opened for append, or
/// the write fails.
pub fn append(root: &Path, report: &Report) -> std::io::Result<PathBuf> {
    let path = path_for(
        root,
        &report.run.env_id,
        &report.sut.entrant,
        report.run.ts_ms,
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = report
        .to_line()
        .map_err(|e| std::io::Error::other(format!("serialize record: {e}")))?;
    debug_assert!(!line.contains('\n'), "a record must be one line");

    // `append(true)` and no `truncate`: the only write mode this module offers.
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")?;
    Ok(path)
}

/// Reads every record under `root`.
///
/// Malformed lines are returned as errors alongside the good records rather than
/// aborting: one corrupt line in an archive should not make the other ten
/// thousand unreadable, and the caller decides whether to fail.
///
/// # Errors
///
/// Only if `root` itself cannot be walked.
pub fn load_all(root: &Path) -> std::io::Result<(Vec<Report>, Vec<String>)> {
    let mut records = Vec::new();
    let mut problems = Vec::new();
    if !root.exists() {
        return Ok((records, problems));
    }
    let mut files = Vec::new();
    collect_jsonl(root, &mut files)?;
    files.sort();
    for path in files {
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                problems.push(format!("read {}: {e}", path.display()));
                continue;
            }
        };
        for (i, line) in src.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Report>(line) {
                Ok(r) => records.push(r),
                Err(e) => problems.push(format!("{}:{}: {e}", path.display(), i + 1)),
            }
        }
    }
    Ok((records, problems))
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_jsonl(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// `YYYY-MM` from epoch milliseconds, via a plain civil-date conversion.
///
/// Hand-rolled rather than pulling in a date crate: this is the only date
/// arithmetic in the harness, and a dependency here would also be compiled into
/// the arm's Docker image.
fn year_month(ts_ms: u64) -> String {
    let days = (ts_ms / 86_400_000) as i64;
    // Howard Hinnant's civil_from_days, shifted to a 1970 epoch.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::report::Trigger;

    #[test]
    fn months_are_derived_from_the_records_own_timestamp() {
        // 2026-07-25T00:00:00Z
        assert_eq!(year_month(1_784_937_600_000), "2026-07");
        // 1970-01-01
        assert_eq!(year_month(0), "1970-01");
        // A leap day, which is where a hand-rolled conversion goes wrong.
        // 2024-02-29T12:00:00Z
        assert_eq!(year_month(1_709_208_000_000), "2024-02");
        // The last second of a year must not roll into the next.
        // 2026-12-31T23:59:59Z
        assert_eq!(year_month(1_798_761_599_000), "2026-12");
        // And the first second of the next must.
        assert_eq!(year_month(1_798_761_600_000), "2027-01");
    }

    #[test]
    fn a_run_that_must_never_be_published_is_written_beside_the_archive_and_not_into_it() {
        // The archive partitions by (env, entrant, month), so a tuning sweep
        // appended to `results/` would land in the SAME FILE as the arm's
        // published records — and this module deliberately cannot rewrite a
        // results file, so clearing them would be a hand-edit of the one file
        // where a slip is unrecoverable.
        let repo = Path::new("/repo");
        let publishable = root_for(repo, Trigger::Manual);
        assert!(
            publishable.ends_with("results"),
            "{}",
            publishable.display()
        );

        for barred in [Trigger::Tuning, Trigger::Pr] {
            let root = root_for(repo, barred);
            assert!(root.ends_with(TUNING_DIR), "{barred:?}: {}", root.display());
            // Not a subdirectory of the archive: `load_all`, `bench stale`, the
            // validator's walker and the site's loader all walk `results/`
            // recursively, so nesting it would put a tuning record back inside
            // every one of them.
            assert!(!root.starts_with(publishable.as_path()), "{barred:?}");
        }
    }

    #[test]
    fn a_partial_rerun_touches_exactly_one_path() {
        // The property the layout exists for: re-running one system cannot
        // produce a diff in another system's file, so a partial sweep is
        // conflict-free by construction rather than by convention.
        let root = Path::new("/results");
        let a = path_for(root, "env", "spate", 1_784_937_600_000);
        let b = path_for(root, "env", "flink", 1_784_937_600_000);
        assert_ne!(a, b);
        assert!(a.ends_with("env/spate/2026-07.jsonl"), "{}", a.display());
    }
}
