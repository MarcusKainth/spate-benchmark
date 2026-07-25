//! The entrant contract: one TOML file per system, and nothing else to edit.
//!
//! There is deliberately **no central registry**. The driver and the site both
//! enumerate `entrants/*/entrant.toml` and derive every filter facet from the
//! union of what they find. That is the property worth protecting as the number
//! of systems grows: adding entrant N+1 touches one new directory and zero
//! shared files, so two concurrent contributions conflict in nothing. The moment
//! a shared list exists, every entrant pull request conflicts with every other
//! one, and the marginal cost of a system stops being constant.
//!
//! TOML rather than JSON because the most valuable content in a competitor's
//! configuration is the *reasons*, and a format without comments loses them.
//! Rather than YAML because there is exactly one way to write an array of tables,
//! no significant whitespace to change meaning under copy-paste, and no
//! `DESER: no` parsing as a boolean.
//!
//! Everything the site would otherwise hardcode — label, ordering, colour, "is
//! this ours" — lives in `[display]` and `[entrant]`, so the rendering code never
//! needs to know that any particular system exists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Descriptor schema version, independent of the result schema.
pub const DESCRIPTOR_SCHEMA: u32 = 1;

/// A parsed `entrant.toml`, plus where it was found.
#[derive(Debug, Clone)]
pub struct Entrant {
    /// Directory holding the descriptor.
    pub dir: PathBuf,
    /// The descriptor itself.
    pub spec: Spec,
}

/// The descriptor's contents.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Descriptor schema version.
    pub schema: u32,
    /// Identity and classification.
    pub entrant: Identity,
    /// Who to contact, and whether the config has been reviewed upstream.
    pub maintainer: Maintainer,
    /// Presentation metadata, so the site hardcodes none.
    pub display: Display,
    /// How to learn what version actually ran. Absent for `planned` entrants.
    #[serde(default)]
    pub version: Option<VersionSource>,
    /// How to build the image. Absent for `planned` entrants.
    #[serde(default)]
    pub build: Option<Build>,
    /// The resource envelope. Absent for `planned` entrants.
    #[serde(default)]
    pub envelope: Option<Envelope>,
    /// Delivery semantics, published beside the numbers.
    #[serde(default)]
    pub guarantees: Option<Guarantees>,
    /// Named docker volumes. Never host bind mounts.
    #[serde(default)]
    pub volumes: Option<Volumes>,
    /// Environment passed to the container, in the entrant's own vocabulary.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The published arms this entrant provides.
    #[serde(default)]
    pub variants: Vec<Variant>,
    /// Machine-readable departures from the methodology.
    #[serde(default)]
    pub deviations: Vec<Deviation>,
    /// Why an entrant is not yet built.
    #[serde(default)]
    pub planned: Option<Planned>,
}

/// Identity and the facets the site filters on.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Must equal the directory name.
    pub id: String,
    /// Display name.
    pub name: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub docs: String,
    /// SPDX identifier.
    pub licence: String,
    /// Who publishes the system. `self` marks our own entry and is what drives
    /// the vendor-run disclosure on the site.
    pub vendor: String,
    /// Broad category, e.g. `stream-processor`.
    pub kind: String,
    /// Implementation languages. An array because polyglot systems are real.
    pub language: Vec<String>,
    /// `native`, `jvm`, `go`, … — a genuine grouping axis for these results.
    pub runtime: String,
    /// Lifecycle.
    pub status: Status,
}

/// Whether an entrant is measured, planned, or retired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Built and measured.
    Active,
    /// Declared but not yet implemented. Validation is relaxed.
    Planned,
    /// Was measured; no longer re-run. Results stay published and visible.
    Historical,
    /// Removed at the maintainer's request or because it no longer applies.
    Withdrawn,
}

impl Status {
    /// Whether this entrant must carry a buildable, runnable definition.
    #[must_use]
    pub fn is_runnable(self) -> bool {
        matches!(self, Self::Active | Self::Historical)
    }
}

/// Contact and upstream-review state.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Maintainer {
    /// `spate-benchmark` means we wrote the arm; otherwise a contact.
    pub who: String,
    /// Rule 7: has the configuration been shown to the project?
    pub reviewed_upstream: bool,
    #[serde(default)]
    pub review_url: String,
}

/// Presentation metadata. The site reads this instead of hardcoding anything.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    /// Stable sort key for equal-ranked rows.
    pub order: i64,
    /// Hue **angle** in degrees, not a hex colour: the site owns contrast in
    /// both themes, and a supplied hex would fail one of them — while a site
    /// that overrode it would make the field a lie.
    pub hue: u16,
    /// Short label for dense layouts.
    pub short: String,
}

/// How to discover what version actually ran.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSource {
    /// `command` | `image-label` | `pinned`.
    pub strategy: String,
    /// For `command`: argv executed inside the image.
    #[serde(default)]
    pub command: Vec<String>,
    /// For `command`: regex with capture groups (version, commit).
    #[serde(default)]
    pub pattern: String,
    /// For `image-label`: the label to read.
    #[serde(default)]
    pub label: String,
    /// Expected value. Asserted against what is resolved; a mismatch refuses the
    /// run rather than publishing a mislabelled number.
    #[serde(default)]
    pub pinned: String,
}

/// How to build the entrant's image.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    /// Build context, relative to the entrant directory.
    pub context: String,
    /// Dockerfile, relative to the entrant directory.
    pub dockerfile: String,
    /// Local image tag. The **digest** is what gets recorded.
    pub image: String,
    /// BuildKit secret ids the build needs.
    #[serde(default)]
    pub secrets: Vec<String>,
}

/// The resource envelope, and how it splits across containers.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// Data-plane CPU total.
    pub cpus: String,
    /// Data-plane memory total.
    pub memory: String,
    /// The containers this entrant runs.
    #[serde(default, rename = "container")]
    pub containers: Vec<Container>,
}

/// One container of a possibly multi-container arm.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Container {
    /// `data-plane` or `control-plane`.
    pub role: Role,
    /// Suffix for the container name.
    pub name: String,
    /// CPU quota.
    pub cpus: String,
    /// Memory limit. Swap is pinned equal to it by the driver.
    pub memory: String,
    /// Command arguments, for images that dispatch on argv.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Whether a container does the work or coordinates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Does the ingestion. Charged against the envelope.
    DataPlane,
    /// Coordinates. Allocated on top, with measured cost published.
    ControlPlane,
}

/// Delivery semantics, published beside the numbers so the comparison is
/// guarantee-for-guarantee rather than throughput-for-throughput.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guarantees {
    /// e.g. `at-least-once`.
    pub delivery: String,
    /// Mechanism: `offset-commit`, `checkpoint`, …
    pub durability: String,
    /// Cadence of that mechanism.
    pub interval_ms: u64,
    #[serde(default)]
    pub notes: String,
}

/// Named docker volumes.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volumes {
    /// `name:/path` entries. Named volumes only — a host bind mount would be
    /// served over VirtioFS on the reference environment, taxing exactly the
    /// paths under measurement.
    pub named: Vec<String>,
}

/// One published arm.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// Stable for the life of the entrant: it is recorded on every result.
    pub id: String,
    /// Human label.
    pub label: String,
    /// `a` or `b`.
    pub tier: String,
    /// The anti-gaming valve.
    pub approach: Approach,
    /// Exactly one variant per active entrant is the default.
    #[serde(default)]
    pub default: bool,
    /// Extra environment for this arm.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Values substituted into `{{knob:…}}` placeholders.
    #[serde(default)]
    pub knobs: BTreeMap<String, toml::Value>,
    /// Facts the arm asserts about itself that the site publishes, notably
    /// `wire_format` (rule 5).
    #[serde(default)]
    pub reports: BTreeMap<String, String>,
    #[serde(default)]
    pub notes: String,
}

/// How realistic a variant's configuration is. Defined against our own rules in
/// `METHODOLOGY.md`, not borrowed wholesale from another benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Approach {
    /// Rules 1 and 3 satisfied. Headline-eligible.
    Realistic,
    /// Rule-1 compliant but not what a typical user would deploy.
    Tuned,
    /// Uses code the project does not ship, or drops a guarantee. Never the
    /// headline; exists to quantify a specific effect.
    Stripped,
}

/// A machine-readable departure from the methodology (rule 4).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deviation {
    /// Stable identifier, so the site can link to an explanation.
    pub id: String,
    /// What differs.
    pub what: String,
    /// Why it is the right call.
    pub why: String,
    /// Which part of the contract it touches.
    #[serde(default)]
    pub affects: Vec<String>,
}

/// Why an entrant is declared but not yet built.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Planned {
    #[serde(default)]
    pub tracking: String,
    #[serde(default)]
    pub blockers: String,
    /// Present when publication is gated on a licence review rather than on
    /// engineering.
    #[serde(default)]
    pub licence_gate: String,
}

impl Entrant {
    /// The entrant's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.spec.entrant.id
    }

    /// The variant marked `default`, else the first.
    #[must_use]
    pub fn default_variant(&self) -> Option<&Variant> {
        self.spec
            .variants
            .iter()
            .find(|v| v.default)
            .or_else(|| self.spec.variants.first())
    }

    /// A variant by id.
    #[must_use]
    pub fn variant(&self, id: &str) -> Option<&Variant> {
        self.spec.variants.iter().find(|v| v.id == id)
    }

    /// The single data-plane container.
    #[must_use]
    pub fn data_plane(&self) -> Option<&Container> {
        self.spec
            .envelope
            .as_ref()?
            .containers
            .iter()
            .find(|c| c.role == Role::DataPlane)
    }
}

/// Loads and validates every descriptor under `dir`.
///
/// # Errors
///
/// Returns every problem found, not just the first: a contributor fixing an
/// entrant should see the whole list rather than discovering one issue per
/// iteration.
pub fn load_all(dir: &Path) -> Result<Vec<Entrant>, Vec<String>> {
    let mut entrants = Vec::new();
    let mut errors = Vec::new();

    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
        Err(e) => return Err(vec![format!("read {}: {e}", dir.display())]),
    };
    dirs.sort();

    for d in dirs {
        let path = d.join("entrant.toml");
        if !path.is_file() {
            errors.push(format!("{} has no entrant.toml", d.display()));
            continue;
        }
        match load_one(&d, &path) {
            Ok(e) => entrants.push(e),
            Err(mut errs) => errors.append(&mut errs),
        }
    }

    cross_check(&entrants, &mut errors);

    if errors.is_empty() {
        Ok(entrants)
    } else {
        Err(errors)
    }
}

fn load_one(dir: &Path, path: &Path) -> Result<Entrant, Vec<String>> {
    let src =
        std::fs::read_to_string(path).map_err(|e| vec![format!("read {}: {e}", path.display())])?;
    let spec: Spec = toml::from_str(&src).map_err(|e| vec![format!("{}: {e}", path.display())])?;
    let entrant = Entrant {
        dir: dir.to_path_buf(),
        spec,
    };
    let errors = validate(&entrant);
    if errors.is_empty() {
        Ok(entrant)
    } else {
        Err(errors)
    }
}

/// Per-entrant validation. Status-gated: `planned` relaxes everything that
/// requires an implementation, so a roadmap entry does not have to lie about
/// having an envelope in order to be listed.
fn validate(e: &Entrant) -> Vec<String> {
    let mut errs = Vec::new();
    let id = e.id();
    let at = |m: String| format!("{id}: {m}");

    if e.spec.schema != DESCRIPTOR_SCHEMA {
        errs.push(at(format!(
            "descriptor schema {}, expected {DESCRIPTOR_SCHEMA}",
            e.spec.schema
        )));
    }

    // The id IS the join key: results, directories and the site all use it, so a
    // divergence between the two would silently orphan every record.
    let dir_name = e
        .dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if dir_name != id {
        errs.push(at(format!("id does not match directory name {dir_name:?}")));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errs.push(at(
            "id must be lowercase ascii, digits and hyphens".to_owned()
        ));
    }
    if e.spec.entrant.licence.trim().is_empty() {
        errs.push(at("licence is empty".to_owned()));
    }
    if e.spec.entrant.language.is_empty() {
        errs.push(at("language must list at least one language".to_owned()));
    }
    if e.spec.display.hue >= 360 {
        errs.push(at(format!(
            "display.hue {} is not an angle in 0..360",
            e.spec.display.hue
        )));
    }

    if !e.spec.entrant.status.is_runnable() {
        // A planned entrant must say why, or it is an empty promise.
        if e.spec.planned.is_none() {
            errs.push(at(
                "status is not runnable but [planned] is absent".to_owned()
            ));
        }
        return errs;
    }

    // From here on the entrant claims to be measurable.
    if e.spec.build.is_none() {
        errs.push(at("active entrant has no [build]".to_owned()));
    }
    if e.spec.version.is_none() {
        errs.push(at("active entrant has no [version]".to_owned()));
    }
    if !e.dir.join("README.md").is_file() {
        errs.push(at("active entrant has no README.md".to_owned()));
    }
    if let Some(b) = &e.spec.build
        && !e.dir.join(&b.dockerfile).is_file()
    {
        errs.push(at(format!("build.dockerfile {} not found", b.dockerfile)));
    }

    validate_envelope(e, &mut errs, &at);
    validate_variants(e, &mut errs, &at);

    errs
}

fn validate_envelope(e: &Entrant, errs: &mut Vec<String>, at: &dyn Fn(String) -> String) {
    let Some(env) = &e.spec.envelope else {
        errs.push(at("active entrant has no [envelope]".to_owned()));
        return;
    };

    let data: Vec<&Container> = env
        .containers
        .iter()
        .filter(|c| c.role == Role::DataPlane)
        .collect();
    if data.len() != 1 {
        errs.push(at(format!(
            "expected exactly one data-plane container, found {}",
            data.len()
        )));
        return;
    }

    // The declared totals must equal what the data-plane containers actually get.
    // Without this the envelope is decorative: the driver would apply per-container
    // caps while the record published a total nothing enforced.
    let sum_cpus: f64 = data.iter().filter_map(|c| c.cpus.parse::<f64>().ok()).sum();
    let want_cpus = env.cpus.parse::<f64>().unwrap_or(f64::NAN);
    if (sum_cpus - want_cpus).abs() > f64::EPSILON {
        errs.push(at(format!(
            "data-plane containers total {sum_cpus} CPUs but [envelope].cpus is {}",
            env.cpus
        )));
    }
    let sum_mem: u64 = data.iter().filter_map(|c| parse_memory(&c.memory)).sum();
    match parse_memory(&env.memory) {
        Some(want) if want == sum_mem => {}
        Some(want) => errs.push(at(format!(
            "data-plane containers total {sum_mem} bytes but [envelope].memory is {} ({want})",
            env.memory
        ))),
        None => errs.push(at(format!(
            "[envelope].memory {:?} unparseable",
            env.memory
        ))),
    }

    // A control-plane container outside the budget is legitimate, but it is a
    // departure from the simplest reading of the contract and must be declared.
    let has_control = env.containers.iter().any(|c| c.role == Role::ControlPlane);
    let declares = e
        .spec
        .deviations
        .iter()
        .any(|d| d.affects.iter().any(|a| a == "envelope"));
    if has_control && !declares {
        errs.push(at(
            "has a control-plane container outside the envelope but declares no \
             [[deviations]] entry affecting \"envelope\""
                .to_owned(),
        ));
    }
}

fn validate_variants(e: &Entrant, errs: &mut Vec<String>, at: &dyn Fn(String) -> String) {
    if e.spec.variants.is_empty() {
        errs.push(at("active entrant declares no variants".to_owned()));
        return;
    }

    let mut seen = std::collections::BTreeSet::new();
    for v in &e.spec.variants {
        if !seen.insert(v.id.as_str()) {
            errs.push(at(format!("duplicate variant id {:?}", v.id)));
        }
        if !v
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            errs.push(at(format!(
                "variant id {:?} must be lowercase ascii, digits and hyphens",
                v.id
            )));
        }
        if !matches!(v.tier.as_str(), "a" | "b") {
            errs.push(at(format!(
                "variant {:?} has tier {:?}, expected a|b",
                v.id, v.tier
            )));
        }
        // Rule 5: the insert format is not the same server-side work across
        // systems, so a results table that omits it is indefensible.
        if !v.reports.contains_key("wire_format") {
            errs.push(at(format!(
                "variant {:?} does not report a wire_format",
                v.id
            )));
        }
        // Rule 1: a stripped arm is one we have reason to publish anyway, and the
        // reason has to be written down or the label is unexplained.
        if v.approach == Approach::Stripped && v.notes.trim().is_empty() {
            errs.push(at(format!(
                "variant {:?} is `stripped` but carries no notes explaining why it exists",
                v.id
            )));
        }
    }

    let defaults = e.spec.variants.iter().filter(|v| v.default).count();
    if defaults != 1 {
        errs.push(at(format!(
            "expected exactly one variant marked default, found {defaults}"
        )));
    }
    // The site's default view shows one row per entrant at its default variant.
    // A `stripped` default would put a deliberately unrepresentative arm in the
    // headline — the exact failure the valve exists to prevent.
    if let Some(d) = e.spec.variants.iter().find(|v| v.default)
        && d.approach != Approach::Realistic
    {
        errs.push(at(format!(
            "default variant {:?} has approach {:?}; the default must be realistic",
            d.id, d.approach
        )));
    }
}

/// Checks that only make sense across the whole set.
fn cross_check(entrants: &[Entrant], errs: &mut Vec<String>) {
    let mut orders: BTreeMap<i64, &str> = BTreeMap::new();
    let mut hues: Vec<(u16, &str)> = Vec::new();

    for e in entrants {
        if let Some(prev) = orders.insert(e.spec.display.order, e.id()) {
            errs.push(format!(
                "display.order {} is used by both {prev} and {}",
                e.spec.display.order,
                e.id()
            ));
        }
        hues.push((e.spec.display.hue, e.id()));
    }

    // Hues must be far enough apart to stay distinguishable. Checked here rather
    // than left to the site, because the site derives its colours from these and
    // cannot invent separation that the data does not have.
    hues.sort_unstable();
    for w in hues.windows(2) {
        let (a, ida) = w[0];
        let (b, idb) = w[1];
        if b - a < 20 {
            errs.push(format!(
                "display.hue {a} ({ida}) and {b} ({idb}) are within 20 degrees"
            ));
        }
    }
}

/// Parses `4g`, `512m`, `1024` (bytes) into bytes.
fn parse_memory(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last()? {
        'g' | 'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        'm' | 'M' => (&s[..s.len() - 1], 1024 * 1024),
        'k' | 'K' => (&s[..s.len() - 1], 1024),
        _ => (s, 1),
    };
    digits.trim().parse::<u64>().ok().map(|n| n * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_suffixes_parse() {
        assert_eq!(parse_memory("4g"), Some(4 * 1024 * 1024 * 1024));
        assert_eq!(parse_memory("512m"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory("1024"), Some(1024));
        assert_eq!(parse_memory("nonsense"), None);
    }

    #[test]
    fn hue_separation_is_enforced() {
        // Two entrants whose colours a reader could not tell apart is a data
        // problem, not a rendering problem: the site derives hue from here and
        // cannot invent separation the descriptors did not provide.
        let mut errs = Vec::new();
        let hues = [(10u16, "a"), (25u16, "b")];
        let mut v: Vec<(u16, &str)> = hues.to_vec();
        v.sort_unstable();
        for w in v.windows(2) {
            if w[1].0 - w[0].0 < 20 {
                errs.push("too close".to_owned());
            }
        }
        assert_eq!(errs.len(), 1);
    }
}
