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
    /// Knob combinations the arm cannot run, declared so a sweep is refused
    /// before it starts a container.
    #[serde(default, rename = "constraints")]
    pub constraints: Vec<Constraint>,
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

/// The `[version].strategy` values `driver::resolve_sut` implements.
///
/// Checked at validation rather than left to fail at run time. A descriptor
/// naming a strategy nobody wrote resolves no version at all, and the only
/// symptom is a refusal after the image has been built and the sweep has
/// started — which is the most expensive moment to learn about a typo.
///
/// `image-label` used to be listed here and was never implemented.
/// `entrants/flink/entrant.toml` records why it should not be: `flink:2.2.1`
/// carries no Flink version label, only `org.opencontainers.image.version:
/// 24.04` for the base operating system, and a descriptor trusting that label
/// would have recorded every Flink result as version 24.04 entirely plausibly.
/// `pinned` was listed too, and is not a strategy but an assertion — a version a
/// human typed into a descriptor is not evidence of what ran.
pub const VERSION_STRATEGIES: [&str; 1] = ["command"];

/// How to discover what version actually ran.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionSource {
    /// How the version is resolved. One of [`VERSION_STRATEGIES`].
    pub strategy: String,
    /// For `command`: argv executed inside the image.
    #[serde(default)]
    pub command: Vec<String>,
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

/// The delivery guarantee every arm is expected to run under.
///
/// `methodology/`: "Every arm runs **at-least-once**." Validation refuses any
/// other value, because an exactly-once arm sitting in the same chart as five
/// at-least-once arms is paying for a stronger guarantee than they are and the
/// numbers are not on one axis.
pub const DELIVERY_CONTRACT: &str = "at-least-once";

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
    /// Components this variant selects that the entrant's project does not ship:
    /// code we wrote, a patched build, a fork.
    ///
    /// Declaring one binds the variant to [`Approach::Stripped`], and that
    /// binding is the whole point of the field. Before it, the only thing keeping
    /// the Flink arm that runs our own `ReusingAvroDeserializationSchema` out of
    /// the headline set was the word `stripped` on one hand-written line;
    /// editing that one word to `realistic` passed every check in the repository
    /// and put hand-written-decoder numbers into the default view of the site.
    /// `methodology/` calls this valve "not hypothetical", and a valve that one
    /// word defeats is not one.
    ///
    /// Naming the component rather than setting a boolean is deliberate: the
    /// declaration is rendered, so a reader can see *what* the arm substituted,
    /// and deleting it to dodge the rule is a visible removal of a disclosure
    /// rather than a one-word edit.
    #[serde(default)]
    pub unshipped: Vec<String>,
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
}

/// How realistic a variant's configuration is. Defined against our own rules in
/// `methodology/`, not borrowed wholesale from another benchmark.
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

/// A knob combination the arm cannot run.
///
/// # Why this exists in the descriptor rather than in the driver
///
/// A tuning sweep walks a product of knob values, and some cells of that product
/// are not runnable at all. The Flink arm's sink is the case that forced this:
/// `AsyncSinkWriter` requires its buffered-request bound to be **strictly
/// greater** than its batch size, so `buffered_rows = max_rows` is not a slow
/// configuration, it is a job that refuses to start. Discovering that after two
/// containers, a job submission and a JVM start-up is minutes per cell across
/// dozens of cells, and the failure arrives as a stack trace in a container log
/// rather than as a sentence about the sweep.
///
/// The alternative was to teach `driver` about Flink's sink, which would put one
/// system's internals into the code that measures every system — the thing
/// [`Spec`] exists so that nobody has to do. The entrant states its own rule and
/// the driver applies it without knowing what it means.
///
/// # Only one relation, deliberately
///
/// `knob` must be strictly greater than `exceeds`, and there is no second form.
/// A general expression language here would be a configuration DSL: unreviewable,
/// and a place for a rule to be written that nobody can evaluate by reading it.
/// One relation covers the case that exists, and a second real case is a second
/// field with its own name and its own doc comment, added by someone who has one.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Constraint {
    /// The knob that must be the larger.
    pub knob: String,
    /// The knob it must strictly exceed.
    pub exceeds: String,
    /// What breaks when it does not. Quoted verbatim in the refusal, so the
    /// operator of a sweep is told why the cell is impossible rather than merely
    /// that it is.
    pub why: String,
}

/// Every way a set of knob values breaks the entrant's declared constraints.
///
/// Every problem, never the first, for the reason `validate::results_are_valid`
/// reports every problem: a sweep operator fixing a cell should see the whole
/// list rather than discovering one bound per attempt.
///
/// A knob a constraint names and the values do not carry is itself a violation.
/// The tempting reading is "not applicable, skip it", and it is wrong here: an
/// unset knob means the image's own `ENV` default applies, which is a value the
/// descriptor never stated and the record would not report — so the constraint
/// would be evaluated against a number nobody can see, or not at all.
#[must_use]
pub fn knob_violations(
    constraints: &[Constraint],
    knobs: &BTreeMap<String, toml::Value>,
) -> Vec<String> {
    let mut out = Vec::new();
    for c in constraints {
        let value = |name: &str| knobs.get(name).and_then(toml::Value::as_integer);
        match (value(&c.knob), value(&c.exceeds)) {
            (Some(a), Some(b)) if a > b => {}
            (Some(a), Some(b)) => out.push(format!(
                "{} = {a} must strictly exceed {} = {b}. {}",
                c.knob,
                c.exceeds,
                // Reflowed onto one line. `why` is a TOML multi-line string, so
                // it arrives hard-wrapped at the width of the descriptor, and a
                // refusal printed with those breaks in the middle of an indented
                // list reads as two problems rather than one.
                c.why.split_whitespace().collect::<Vec<_>>().join(" ")
            )),
            _ => out.push(format!(
                "the constraint \"{} must exceed {}\" cannot be checked: one of them \
                 is unset or is not an integer here (knobs: {}). An unset knob leaves \
                 the image's own default in force, which is a value this descriptor \
                 never states and no record reports.",
                c.knob,
                c.exceeds,
                if knobs.is_empty() {
                    "none".to_owned()
                } else {
                    knobs
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )),
        }
    }
    out
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
    if !e.dir.join("README.md").is_file() {
        errs.push(at("active entrant has no README.md".to_owned()));
    }
    if let Some(b) = &e.spec.build
        && !e.dir.join(&b.dockerfile).is_file()
    {
        errs.push(at(format!("build.dockerfile {} not found", b.dockerfile)));
    }

    validate_version(e, &mut errs, &at);
    validate_guarantees(e, &mut errs, &at);
    validate_envelope(e, &mut errs, &at);
    validate_variants(e, &mut errs, &at);
    validate_constraints(e, &mut errs, &at);

    errs
}

/// Checks the declared constraints, and checks every committed variant against
/// them.
///
/// Both halves are needed and they catch different mistakes. The first catches a
/// constraint that names a knob nothing sets, which would be a rule about
/// nothing. The second catches a *committed* variant that violates its own
/// entrant's rule — a descriptor that cannot run, which would otherwise be
/// discovered by whoever next ran the arm rather than by whoever wrote it.
fn validate_constraints(e: &Entrant, errs: &mut Vec<String>, at: &dyn Fn(String) -> String) {
    for c in &e.spec.constraints {
        for name in [&c.knob, &c.exceeds] {
            if name.trim().is_empty() {
                errs.push(at(
                    "a [[constraints]] entry names an empty knob; a constraint over \
                     nothing is a rule that can never fire"
                        .to_owned(),
                ));
            }
        }
        if c.knob == c.exceeds {
            errs.push(at(format!(
                "constraint {:?} must exceed itself, which nothing can satisfy",
                c.knob
            )));
        }
        if c.why.trim().is_empty() {
            errs.push(at(format!(
                "constraint \"{} must exceed {}\" does not say what breaks when it \
                 does not. The text is quoted verbatim into the refusal a sweep sees, \
                 and a refusal that only says `no` costs whoever hits it the \
                 investigation this field exists to have already done.",
                c.knob, c.exceeds
            )));
        }
    }

    for v in &e.spec.variants {
        for why in knob_violations(&e.spec.constraints, &v.knobs) {
            errs.push(at(format!("variant {:?}: {why}", v.id)));
        }
    }
}

/// Checks that the version can actually be resolved by the code that resolves it.
///
/// The failure this moves earlier: an unimplemented strategy, or a `command`
/// strategy with no argv, resolves nothing at all, and `driver::resolve_sut`
/// refuses the run — after the image has been built and the sweep has started.
/// Every fact needed to catch it is in the descriptor.
fn validate_version(e: &Entrant, errs: &mut Vec<String>, at: &dyn Fn(String) -> String) {
    let Some(v) = &e.spec.version else {
        errs.push(at("active entrant has no [version]".to_owned()));
        return;
    };
    if !VERSION_STRATEGIES.contains(&v.strategy.as_str()) {
        errs.push(at(format!(
            "[version].strategy {:?} is not implemented. Known: {}",
            v.strategy,
            VERSION_STRATEGIES.join(", ")
        )));
    }
    if v.strategy == "command" && v.command.is_empty() {
        errs.push(at(
            "[version].strategy is \"command\" but [version].command is empty, so \
             nothing would be run and no version resolved"
                .to_owned(),
        ));
    }
}

/// Checks the delivery guarantee against the one the methodology fixes.
///
/// Every arm is compared guarantee-for-guarantee, so an arm declaring something
/// other than [`DELIVERY_CONTRACT`] is paying for something different from every
/// other arm on the same chart and cannot be drawn beside them.
fn validate_guarantees(e: &Entrant, errs: &mut Vec<String>, at: &dyn Fn(String) -> String) {
    let Some(g) = &e.spec.guarantees else {
        errs.push(at(
            "active entrant has no [guarantees]. Delivery semantics are published \
             beside the numbers so the comparison is guarantee-for-guarantee; an arm \
             that declares none cannot be compared at all."
                .to_owned(),
        ));
        return;
    };
    if g.delivery != DELIVERY_CONTRACT {
        errs.push(at(format!(
            "declares delivery {:?} rather than {DELIVERY_CONTRACT:?}. Every arm is \
             compared guarantee-for-guarantee, so an arm paying for a different \
             guarantee is not on the same axis as the rest.",
            g.delivery
        )));
    }
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
        // Rule 1's valve. A variant that selects code the project does not ship
        // and is labelled `realistic` is the direction the rule exists to stop,
        // and `unshipped` is what makes that direction detectable at all:
        // without a declaration there is nothing in any file the harness reads
        // that distinguishes "runs the shipped deserializer" from "runs ours".
        for u in &v.unshipped {
            if u.trim().is_empty() {
                errs.push(at(format!(
                    "variant {:?} lists an empty `unshipped` entry; name the component",
                    v.id
                )));
            }
        }
        if !v.unshipped.is_empty() && v.approach != Approach::Stripped {
            errs.push(at(format!(
                "variant {:?} declares unshipped {:?} but has approach {:?}. Code the \
                 project does not ship is `stripped` under rule 1 and is never the \
                 headline. If the arm no longer uses it, remove the declaration in \
                 the same change — the two move together or the label is decoration.",
                v.id, v.unshipped, v.approach
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

/// Parses a declared memory limit — `16g`, `512m`, `1048576k`, `1024` (bytes) —
/// into bytes.
///
/// Public, and the **only** copy, because there were four and they had drifted.
/// This module and `infra` accepted `k`; the copy in `driver` did not, and the
/// copy in `tests/entrants_are_valid.rs` returned MiB and rejected a bare byte
/// count. The `driver` copy is the one that asserts an arm's *applied* cgroup
/// cap, which the methodology says fails a run rather than warning, so the
/// drift landed on the strictest check: a descriptor declaring
/// `memory = "1048576k"` validated here, was applied by Docker exactly as asked,
/// and then failed the cap assertion with a message reporting that the container
/// ran under a limit which was in fact the one requested.
///
/// It lives here because descriptors are where these strings originate.
#[must_use]
pub fn parse_memory(s: &str) -> Option<u64> {
    let s = s.trim();
    // Only ASCII suffixes are sliced off, so `s.len() - 1` is always a character
    // boundary — a trailing multi-byte character falls through to the bare-count
    // arm and fails to parse.
    let (digits, mult) = match s.chars().last()? {
        'g' | 'G' => (&s[..s.len() - 1], 1024 * 1024 * 1024),
        'm' | 'M' => (&s[..s.len() - 1], 1024 * 1024),
        'k' | 'K' => (&s[..s.len() - 1], 1024),
        _ => (s, 1),
    };
    // Checked rather than wrapping: an absurd declaration has to be unparseable,
    // because a wrapped product would be a plausible small number that could
    // compare equal to an unrelated cap and pass the assertion it exists to fail.
    digits.trim().parse::<u64>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(id: &str, approach: Approach, unshipped: &[&str]) -> Variant {
        Variant {
            id: id.to_owned(),
            label: id.to_owned(),
            tier: "a".to_owned(),
            approach,
            default: false,
            env: BTreeMap::new(),
            knobs: BTreeMap::new(),
            reports: BTreeMap::from([("wire_format".to_owned(), "native".to_owned())]),
            unshipped: unshipped.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn entrant_with(variants: Vec<Variant>) -> Entrant {
        Entrant {
            dir: PathBuf::from("entrants/probe"),
            spec: Spec {
                schema: DESCRIPTOR_SCHEMA,
                entrant: Identity {
                    id: "probe".to_owned(),
                    name: "Probe".to_owned(),
                    homepage: String::new(),
                    repo: String::new(),
                    docs: String::new(),
                    licence: "Apache-2.0".to_owned(),
                    vendor: "probe".to_owned(),
                    kind: "stream-processor".to_owned(),
                    language: vec!["rust".to_owned()],
                    runtime: "native".to_owned(),
                    status: Status::Active,
                },
                maintainer: Maintainer {
                    who: "spate-benchmark".to_owned(),
                    reviewed_upstream: false,
                    review_url: String::new(),
                },
                display: Display {
                    order: 1,
                    hue: 0,
                    short: "Probe".to_owned(),
                },
                version: None,
                build: None,
                envelope: None,
                guarantees: None,
                volumes: None,
                env: BTreeMap::new(),
                variants,
                constraints: Vec::new(),
                planned: None,
            },
        }
    }

    fn errors_from(variants: Vec<Variant>) -> Vec<String> {
        let e = entrant_with(variants);
        let mut errs = Vec::new();
        let at = |m: String| format!("probe: {m}");
        validate_variants(&e, &mut errs, &at);
        errs
    }

    /// The Flink sink's rule, in the shape the descriptor states it.
    fn buffered_exceeds_batch() -> Constraint {
        Constraint {
            knob: "buffered_rows".to_owned(),
            exceeds: "max_rows".to_owned(),
            why: "AsyncSinkWriter refuses to construct otherwise.".to_owned(),
        }
    }

    fn knobs(pairs: &[(&str, i64)]) -> BTreeMap<String, toml::Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), toml::Value::Integer(*v)))
            .collect()
    }

    #[test]
    fn every_memory_suffix_a_descriptor_may_use_parses_to_bytes() {
        assert_eq!(parse_memory("4g"), Some(4 * 1024 * 1024 * 1024));
        assert_eq!(parse_memory("512m"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory("1024"), Some(1024));
        assert_eq!(parse_memory("nonsense"), None);
        assert_eq!(parse_memory(" 8G "), Some(8 * 1024 * 1024 * 1024));
    }

    #[test]
    fn a_kibibyte_suffix_parses_because_one_copy_of_this_used_to_reject_it() {
        // The drift this closes. `driver`'s copy had no `k` arm, so a descriptor
        // declaring `1048576k` validated here, was applied by Docker as exactly
        // one gibibyte, and then failed the cap assertion claiming the container
        // was running under a limit that was precisely the one requested.
        assert_eq!(parse_memory("1048576k"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory("1048576K"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory("1g"), parse_memory("1048576k"));
    }

    #[test]
    fn an_overflowing_declaration_is_unparseable_rather_than_wrapped() {
        // A wrapped product is the dangerous outcome: it is a plausible small
        // number that could compare equal to some unrelated cap and pass the
        // assertion this parser feeds.
        assert_eq!(parse_memory("99999999999999999999g"), None);
        assert_eq!(parse_memory("18014398509481985k"), None);
    }

    #[test]
    fn a_variant_selecting_unshipped_code_must_be_labelled_stripped() {
        // The valve's missing converse. `entrants/flink/entrant.toml` marks the
        // arm running our own `ReusingAvroDeserializationSchema` as `stripped` on
        // one hand-written line; before this rule, editing that word to
        // `realistic` passed every check and moved hand-written-decoder numbers
        // into the site's default view.
        let errs = errors_from(vec![
            variant("shipped", Approach::Realistic, &[]),
            variant(
                "ours",
                Approach::Realistic,
                &["ReusingAvroDeserializationSchema"],
            ),
        ]);
        assert!(
            errs.iter().any(|e| e.contains("declares unshipped")),
            "{errs:?}"
        );

        // Labelled correctly, the same descriptor raises nothing about approach.
        let errs = errors_from(vec![
            variant("shipped", Approach::Realistic, &[]),
            variant(
                "ours",
                Approach::Stripped,
                &["ReusingAvroDeserializationSchema"],
            ),
        ]);
        assert!(
            !errs.iter().any(|e| e.contains("declares unshipped")),
            "{errs:?}"
        );
    }

    #[test]
    fn an_unshipped_declaration_must_name_the_component() {
        // `unshipped = [""]` would otherwise satisfy the rule above while
        // disclosing nothing, which is the shape a reader cannot act on.
        let errs = errors_from(vec![variant("ours", Approach::Stripped, &[" "])]);
        assert!(
            errs.iter().any(|e| e.contains("empty `unshipped` entry")),
            "{errs:?}"
        );
    }

    #[test]
    fn a_knob_combination_the_arm_cannot_run_is_a_violation_quoting_the_reason() {
        // The cell a sweep will reach for first: raise the batch size to match
        // the single-process arms' and leave the buffer where it was. Flink's
        // AsyncSinkWriter refuses to construct, minutes into the cell, with a
        // message naming neither knob.
        let v = knob_violations(
            &[buffered_exceeds_batch()],
            &knobs(&[("max_rows", 262_144), ("buffered_rows", 50_000)]),
        );
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("262144"), "{}", v[0]);
        assert!(v[0].contains("50000"), "{}", v[0]);
        assert!(
            v[0].contains("AsyncSinkWriter"),
            "the entrant's own reason: {}",
            v[0]
        );

        // Strictly greater, not greater-or-equal: equality is exactly the case
        // the sink rejects, and an off-by-one here would let the sweep spend a
        // cell proving it.
        assert_eq!(
            knob_violations(
                &[buffered_exceeds_batch()],
                &knobs(&[("max_rows", 50_000), ("buffered_rows", 50_000)]),
            )
            .len(),
            1
        );
        assert!(
            knob_violations(
                &[buffered_exceeds_batch()],
                &knobs(&[("max_rows", 50_000), ("buffered_rows", 50_001)]),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_constraint_over_a_knob_the_variant_does_not_set_is_a_violation_and_not_a_pass() {
        // "Not applicable, skip it" is the tempting reading and the wrong one:
        // an unset knob leaves the image's own ENV default in force, so the
        // constraint would be evaluated against a number the descriptor never
        // states and no record reports.
        let v = knob_violations(&[buffered_exceeds_batch()], &knobs(&[("max_rows", 25_000)]));
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v[0].contains("cannot be checked"), "{}", v[0]);
        assert!(v[0].contains("max_rows"), "{}", v[0]);
    }

    #[test]
    fn a_committed_variant_that_breaks_its_own_entrants_constraint_fails_validation() {
        // A descriptor that cannot run must fail where it is written rather than
        // where it is next used.
        let mut e = entrant_with(vec![variant("only", Approach::Realistic, &[])]);
        e.spec.constraints = vec![buffered_exceeds_batch()];
        e.spec.variants[0].knobs = knobs(&[("max_rows", 25_000), ("buffered_rows", 25_000)]);

        let mut errs = Vec::new();
        let at = |m: String| format!("probe: {m}");
        validate_constraints(&e, &mut errs, &at);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].contains("variant \"only\""), "{}", errs[0]);
    }

    #[test]
    fn a_constraint_that_explains_nothing_is_rejected() {
        // The `why` is quoted verbatim into the refusal a sweep operator reads,
        // so an empty one costs whoever hits it the investigation the field
        // exists to have already done.
        let mut e = entrant_with(vec![variant("only", Approach::Realistic, &[])]);
        e.spec.constraints = vec![Constraint {
            knob: "buffered_rows".to_owned(),
            exceeds: "buffered_rows".to_owned(),
            why: "  ".to_owned(),
        }];
        e.spec.variants[0].knobs = knobs(&[("buffered_rows", 1)]);

        let mut errs = Vec::new();
        let at = |m: String| format!("probe: {m}");
        validate_constraints(&e, &mut errs, &at);
        assert!(
            errs.iter().any(|x| x.contains("must exceed itself")),
            "{errs:?}"
        );
        assert!(
            errs.iter().any(|x| x.contains("does not say what breaks")),
            "{errs:?}"
        );
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
