//! Writing results, and the one thing this module deliberately cannot do.
//!
//! **There is no truncate path here.** Not a guarded one, not a flag-protected
//! one — the capability does not exist. Retention of published numbers is
//! enforced by the absence of the operation rather than by anyone remembering
//! not to use it, because the previous runner script opened its results file
//! with `>` and silently replaced every earlier run on each invocation.
//!
//! A run later found to be wrong is **retracted, not deleted**: [`retract`]
//! appends a `superseded_by` marker naming the reason, and the site renders the
//! number struck through with that reason attached. A reader who saw a number
//! once can always find out what happened to it.
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

use crate::report::{Report, Superseded, now_ms};

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

/// Marks a record superseded, in place, without removing it.
///
/// Rewrites the one file containing `run_id`, preserving every other line
/// byte-for-byte. The rewrite is via a temporary file and a rename so an
/// interrupted retraction cannot leave a half-written archive.
///
/// # Errors
///
/// If the id is not found, or the file cannot be rewritten.
pub fn retract(root: &Path, run_id: &str, reason: &str) -> std::io::Result<PathBuf> {
    let mut files = Vec::new();
    collect_jsonl(root, &mut files)?;
    files.sort();

    for path in files {
        let src = std::fs::read_to_string(&path)?;
        let mut hit = false;
        let mut out = String::with_capacity(src.len() + 256);
        for line in src.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Report>(line) {
                Ok(mut r) if r.run_id == run_id => {
                    hit = true;
                    r.superseded_by = Some(Superseded {
                        by: None,
                        reason: reason.to_owned(),
                        ts_ms: now_ms(),
                    });
                    let l = r
                        .to_line()
                        .map_err(|e| std::io::Error::other(format!("serialize: {e}")))?;
                    out.push_str(&l);
                    out.push('\n');
                }
                // Every other line is copied verbatim rather than round-tripped
                // through the struct: re-serialising would silently rewrite
                // records this retraction has no business touching.
                _ => {
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
        if hit {
            let tmp = path.with_extension("jsonl.tmp");
            std::fs::write(&tmp, out)?;
            std::fs::rename(&tmp, &path)?;
            return Ok(path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no record with run_id {run_id}"),
    ))
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
