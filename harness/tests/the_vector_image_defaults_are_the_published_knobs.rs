//! The Vector image's hand-maintained equalities, compared by machinery.
//!
//! The Dockerfile promises "the knob defaults are kept EQUAL to the
//! descriptor's published knobs", and vector.yaml.tmpl promises "the defaults
//! after `:-` equal the published knobs" — so that a reviewer who starts the
//! container by hand gets the configuration the numbers describe (rule 7 sends
//! this image's configuration upstream). Until this test, both promises were
//! discipline: a retuned knob that missed one of the three files failed
//! nothing, and the hand-run container quietly stopped being the arm the
//! records describe. The analogous two-files-must-agree couplings (Flink's
//! gc_log, EXPECT_PARALLELISM) already have tests; this is Vector's.
//!
//! Vector-specific deliberately: the other arms have no image-ENV default
//! layer, so there is nothing to generalize over.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use spate_benchmark_harness::entrant;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("harness/ has a parent")
        .to_path_buf()
}

fn vector() -> entrant::Entrant {
    entrant::load_all(&repo_root().join("entrants"))
        .expect("descriptors valid")
        .into_iter()
        .find(|e| e.id() == "vector")
        .expect("vector entrant")
}

fn read(e: &entrant::Entrant, name: &str) -> String {
    std::fs::read_to_string(e.dir.join(name)).unwrap_or_else(|err| panic!("read {name}: {err}"))
}

/// `KEY=VALUE` pairs from every `ENV` instruction, `\` line-continuations
/// included. Values here contain no spaces or quotes, so whitespace splitting
/// is exact.
fn dockerfile_env(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut continued = false;
    for line in text.lines() {
        let t = line.trim();
        let body = if continued {
            t
        } else if let Some(rest) = t.strip_prefix("ENV ") {
            rest
        } else {
            continue;
        };
        continued = body.ends_with('\\');
        for token in body.trim_end_matches('\\').split_whitespace() {
            if let Some((k, v)) = token.split_once('=') {
                out.insert(k.to_owned(), v.to_owned());
            }
        }
    }
    out
}

/// Every `${VAR:-fallback}` in the template. Bare `${VAR}` carries no default
/// and is not collected — there is nothing there to drift.
fn tmpl_fallbacks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("${") {
        let after = &rest[i + 2..];
        let end = after
            .find('}')
            .expect("vector.yaml.tmpl has an unterminated ${…}");
        if let Some((var, fallback)) = after[..end].split_once(":-") {
            out.push((var.to_owned(), fallback.to_owned()));
        }
        rest = &after[end + 1..];
    }
    out
}

/// The entrant's `[env]` entries whose whole value is `{{knob:X}}`: variable
/// name -> knob name. This is the same table the driver substitutes through,
/// so the mapping cannot be restated here and drift.
fn knob_by_var(spec: &entrant::Spec) -> BTreeMap<&str, &str> {
    spec.env
        .iter()
        .filter_map(|(var, value)| {
            let knob = value.strip_prefix("{{knob:")?.strip_suffix("}}")?;
            Some((var.as_str(), knob))
        })
        .collect()
}

/// A knob as the container will see it — `driver::knob_text`'s rendering.
fn knob_text(v: &toml::Value) -> String {
    v.as_integer().map_or_else(
        || v.as_str().unwrap_or_default().to_owned(),
        |n| n.to_string(),
    )
}

#[test]
fn each_dockerfile_env_default_equals_the_published_knob() {
    let e = vector();
    let env = dockerfile_env(&read(&e, "Dockerfile"));
    let default = e.default_variant().expect("a default variant");

    let vars = knob_by_var(&e.spec);
    assert!(!vars.is_empty(), "vector's [env] names no knobs at all");
    for (var, knob) in vars {
        let published = default
            .knobs
            .get(knob)
            .map(knob_text)
            .unwrap_or_else(|| panic!("default variant declares no knob {knob:?}"));
        let baked = env
            .get(var)
            .unwrap_or_else(|| panic!("the Dockerfile sets no ENV default for {var}"));
        assert_eq!(
            baked, &published,
            "{var}: the Dockerfile ENV default is {baked:?} but the default variant \
             ({}) publishes {knob} = {published:?}. The Dockerfile promises the two \
             are equal, so a hand-run container is no longer the configuration the \
             numbers describe.",
            default.id
        );
    }
}

#[test]
fn each_template_fallback_equals_the_published_value() {
    let e = vector();
    let env = dockerfile_env(&read(&e, "Dockerfile"));
    let default = e.default_variant().expect("a default variant");
    let vars = knob_by_var(&e.spec);

    let fallbacks = tmpl_fallbacks(&read(&e, "vector.yaml.tmpl"));
    assert!(
        !fallbacks.is_empty(),
        "vector.yaml.tmpl has no ${{VAR:-fallback}} defaults; this test is checking nothing"
    );
    for (var, fallback) in fallbacks {
        // Each fallback's published value comes from the same source the
        // driver feeds the variable from: the knob table for `{{knob:X}}`
        // entries, the default variant's env for FORMAT, and the image ENV for
        // AUTO_OFFSET_RESET (a run-mode value whose hand-run default lives
        // only in the Dockerfile).
        let published = if let Some(knob) = vars.get(var.as_str()) {
            default
                .knobs
                .get(*knob)
                .map(knob_text)
                .unwrap_or_else(|| panic!("default variant declares no knob {knob:?}"))
        } else if var == "FORMAT" {
            default
                .env
                .get("FORMAT")
                .cloned()
                .expect("the default variant sets FORMAT")
        } else if var == "AUTO_OFFSET_RESET" {
            env.get("AUTO_OFFSET_RESET")
                .cloned()
                .expect("the Dockerfile sets AUTO_OFFSET_RESET")
        } else {
            panic!(
                "vector.yaml.tmpl gives {var} a fallback but nothing publishes a value \
                 for it; either map it in entrant.toml's [env] or drop the default"
            );
        };
        assert_eq!(
            fallback, published,
            "{var}: vector.yaml.tmpl falls back to {fallback:?} but the published \
             value is {published:?}. The template promises the `:-` defaults equal \
             the published configuration, so a hand-run container would not be the \
             arm the numbers describe."
        );
    }
}

#[test]
fn the_declared_commit_cadence_is_the_one_the_config_sets() {
    let e = vector();
    let declared = e
        .spec
        .guarantees
        .as_ref()
        .expect("vector declares [guarantees]")
        .interval_ms;

    let tmpl = read(&e, "vector.yaml.tmpl");
    let configured: u64 = tmpl
        .lines()
        .find_map(|l| l.trim().strip_prefix("commit_interval_ms:"))
        .and_then(|v| v.trim().parse().ok())
        .expect("vector.yaml.tmpl sets commit_interval_ms to an integer");
    assert_eq!(
        declared, configured,
        "entrant.toml publishes [guarantees].interval_ms = {declared} but \
         vector.yaml.tmpl sets commit_interval_ms: {configured}. The durability \
         cadence beside the numbers would not be the one the arm paid."
    );
}

#[test]
fn the_dockerfile_format_default_is_the_default_variants_format() {
    let e = vector();
    let env = dockerfile_env(&read(&e, "Dockerfile"));
    let default = e.default_variant().expect("a default variant");

    let published = default
        .env
        .get("FORMAT")
        .expect("the default variant sets FORMAT");
    let baked = env
        .get("FORMAT")
        .expect("the Dockerfile sets an ENV default for FORMAT");
    assert_eq!(
        baked, published,
        "FORMAT: the Dockerfile ENV default is {baked:?} but the default variant \
         ({}) runs {published:?}, so a hand-run container would publish a different \
         wire format from the headline arm's.",
        default.id
    );
}
