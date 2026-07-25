//! Bakes the framework's identity into the arm.
//!
//! The driver resolves what an arm actually ran by executing it with
//! `--version`, so the arm has to be able to answer. For a released dependency
//! that would be trivial; while the framework is an unpublished git dependency
//! the answer has to come from `Cargo.lock`, which is the only place the exact
//! revision is recorded.
//!
//! Reading the lockfile rather than, say, an environment variable is what makes
//! the reported commit *the one that was linked*. Anything a human can set by
//! hand can be set wrongly, and a benchmark record that names the wrong revision
//! is worse than one that names none.

use std::path::PathBuf;

/// The dependency whose version and revision identify the framework build.
const ANCHOR: &str = "etl-core";

fn main() {
    let lock_path = workspace_root().join("Cargo.lock");
    println!("cargo:rerun-if-changed={}", lock_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));

    let (version, source) = anchor_package(&lock);

    // `git+<url>?rev=<pin>#<resolved sha>`. The fragment is the resolved commit,
    // which is what we want: `rev` may be a short sha or a tag, the fragment
    // never is.
    let commit = source
        .rsplit_once('#')
        .map(|(_, sha)| sha.to_owned())
        .unwrap_or_else(|| "unknown".to_owned());

    // `-dev` while the framework is unpublished: the lockfile reports the
    // in-development version from its manifest, and presenting that bare would
    // imply a release that does not exist. Drop the suffix when the dependency
    // becomes a crates.io version.
    let display = if source.starts_with("git+") {
        format!("{version}-dev")
    } else {
        version
    };

    println!("cargo:rustc-env=SPATE_ARM_FRAMEWORK_VERSION={display}");
    println!(
        "cargo:rustc-env=SPATE_ARM_FRAMEWORK_COMMIT={}",
        commit.get(..12).unwrap_or(&commit)
    );

    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let toolchain = std::process::Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=SPATE_ARM_TOOLCHAIN={toolchain}");
}

/// The `version` and `source` of [`ANCHOR`] in a Cargo lockfile.
///
/// Hand-parsed rather than pulled through a TOML crate: this runs on every
/// build of the arm, the shape is fixed by cargo, and a build-dependency here
/// would be compiled inside the arm's Docker image for one lookup.
fn anchor_package(lock: &str) -> (String, String) {
    let mut version = None;
    let mut source = None;
    let mut in_anchor = false;

    for line in lock.lines() {
        let line = line.trim();
        if line == "[[package]]" {
            // Entering a new package block ends the previous one. If that was
            // the anchor and it was complete, we are done.
            if in_anchor && version.is_some() && source.is_some() {
                break;
            }
            in_anchor = false;
            version = None;
            source = None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("name = ") {
            in_anchor = rest.trim_matches('"') == ANCHOR;
            continue;
        }
        if !in_anchor {
            continue;
        }
        if let Some(rest) = line.strip_prefix("version = ") {
            version = Some(rest.trim_matches('"').to_owned());
        } else if let Some(rest) = line.strip_prefix("source = ") {
            source = Some(rest.trim_matches('"').to_owned());
        }
    }

    let version = version.unwrap_or_else(|| {
        panic!("{ANCHOR} not found in Cargo.lock — the arm cannot report what it linked")
    });
    // A path dependency has no `source`. That is a legitimate local-iteration
    // state, and it is reported honestly rather than guessed at.
    (version, source.unwrap_or_else(|| "path".to_owned()))
}

fn workspace_root() -> PathBuf {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or_else(|| panic!("entrants/spate/ has a workspace root two levels up"))
        .to_path_buf()
}
