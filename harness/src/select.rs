//! The selector grammar: which arms a command applies to.
//!
//! `<entrant>[:<variant>[:<version>]]`, with `*` accepted in any position and an
//! optional `@<image-tag>` override.
//!
//! This exists because the partial re-run is the operation that matters most
//! here. Re-running one system must not require re-running any other, and must
//! not touch any other system's results — so the unit of selection has to be
//! finer than "everything" and coarser than a hand-written list of container
//! commands.
//!
//! ```text
//! spate                          every variant of one entrant
//! spate:tier-a-rowbinary         one variant
//! *                              every active entrant
//! *:*:2.2.1                      every arm at one version
//! flink@spate-bench-flink:2.3.0  a specific image, e.g. a new version of an entrant
//! ```

use std::fmt;

use crate::entrant::{Entrant, Variant};

/// One parsed selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    /// Entrant id, or `*`.
    pub entrant: String,
    /// Variant id, or `*`.
    pub variant: String,
    /// Version, or `*`. Matched against the resolved version at run time.
    pub version: String,
    /// Explicit image tag, overriding the descriptor's.
    pub image: Option<String>,
}

impl Selector {
    /// Parses one selector string.
    ///
    /// # Errors
    ///
    /// If the selector has too many colon-separated parts, or any part is empty.
    /// An empty part is rejected rather than treated as `*`: `spate::` almost
    /// certainly means a shell variable failed to expand, and silently widening
    /// it to every variant would run far more than the caller asked for.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (body, image) = match s.split_once('@') {
            Some((b, i)) if !i.is_empty() => (b, Some(i.to_owned())),
            Some((_, _)) => return Err(format!("{s:?}: empty image tag after '@'")),
            None => (s, None),
        };

        let parts: Vec<&str> = body.split(':').collect();
        if parts.len() > 3 {
            return Err(format!(
                "{s:?}: expected <entrant>[:<variant>[:<version>]], got {} parts",
                parts.len()
            ));
        }
        for p in &parts {
            if p.is_empty() {
                return Err(format!(
                    "{s:?}: empty component. Use '*' to mean 'any' — an empty one is \
                     usually an unexpanded shell variable, and widening it silently \
                     would run more than you asked for."
                ));
            }
        }

        Ok(Self {
            entrant: parts[0].to_owned(),
            variant: parts.get(1).unwrap_or(&"*").to_owned().to_owned(),
            version: parts.get(2).unwrap_or(&"*").to_owned().to_owned(),
            image,
        })
    }

    /// Whether this selector names the given entrant.
    #[must_use]
    pub fn matches_entrant(&self, id: &str) -> bool {
        self.entrant == "*" || self.entrant == id
    }

    /// Whether this selector names the given variant.
    #[must_use]
    pub fn matches_variant(&self, id: &str) -> bool {
        self.variant == "*" || self.variant == id
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.entrant, self.variant, self.version)?;
        if let Some(i) = &self.image {
            write!(f, "@{i}")?;
        }
        Ok(())
    }
}

/// One arm a command will act on.
#[derive(Debug, Clone)]
pub struct Arm<'a> {
    /// The entrant.
    pub entrant: &'a Entrant,
    /// The variant.
    pub variant: &'a Variant,
    /// Image override from the selector, if any.
    pub image: Option<String>,
}

/// Expands selectors against the loaded entrants.
///
/// Only runnable entrants are considered: a `planned` entry has no build and no
/// variants, and `*` silently including one would fail at `docker build` with a
/// confusing error rather than at selection with a clear one.
///
/// # Errors
///
/// If a selector names an entrant or variant that does not exist, or matches
/// nothing. A selector that matches nothing is an error rather than a no-op:
/// `bench run spate:tier-a-natve` is a typo, and quietly running zero arms would
/// look like success.
pub fn expand<'a>(entrants: &'a [Entrant], selectors: &[Selector]) -> Result<Vec<Arm<'a>>, String> {
    let mut out: Vec<Arm<'a>> = Vec::new();

    for sel in selectors {
        if sel.entrant != "*" && !entrants.iter().any(|e| e.id() == sel.entrant) {
            return Err(format!(
                "no entrant {:?}. Known: {}",
                sel.entrant,
                entrants
                    .iter()
                    .map(Entrant::id)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        let mut hits = 0usize;
        for e in entrants {
            if !e.spec.entrant.status.is_runnable() || !sel.matches_entrant(e.id()) {
                continue;
            }
            for v in &e.spec.variants {
                if !sel.matches_variant(&v.id) {
                    continue;
                }
                hits += 1;
                // Deduplicate: overlapping selectors ("*" plus "spate") must not
                // run an arm twice and record two results for one intent.
                if out
                    .iter()
                    .any(|a| a.entrant.id() == e.id() && a.variant.id == v.id)
                {
                    continue;
                }
                out.push(Arm {
                    entrant: e,
                    variant: v,
                    image: sel.image.clone(),
                });
            }
        }

        if hits == 0 {
            return Err(format!("selector {sel} matched no runnable arm"));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_three_positions() {
        let s = Selector::parse("spate:tier-a:0.1.0").expect("parses");
        assert_eq!(s.entrant, "spate");
        assert_eq!(s.variant, "tier-a");
        assert_eq!(s.version, "0.1.0");
        assert_eq!(s.image, None);
    }

    #[test]
    fn omitted_positions_default_to_any() {
        let s = Selector::parse("spate").expect("parses");
        assert_eq!((s.variant.as_str(), s.version.as_str()), ("*", "*"));
    }

    #[test]
    fn an_image_override_is_split_off_first() {
        // The shape that runs a NEW VERSION of an existing entrant, which is the
        // second axis this benchmark grows along.
        let s = Selector::parse("flink@spate-bench-flink:2.3.0").expect("parses");
        assert_eq!(s.entrant, "flink");
        assert_eq!(s.image.as_deref(), Some("spate-bench-flink:2.3.0"));
        // The colon inside the image tag must NOT be read as a variant.
        assert_eq!(s.variant, "*");
    }

    #[test]
    fn an_empty_component_is_rejected_rather_than_widened() {
        // `spate::` is almost always an unexpanded shell variable. Treating it as
        // "every variant" would run far more than the caller asked for, on a
        // machine where a sweep costs hours.
        let e = Selector::parse("spate::").expect_err("must reject");
        assert!(e.contains("empty component"), "{e}");
        assert!(Selector::parse("@tag").is_err());
        assert!(Selector::parse("a:b:c:d").is_err());
    }

    #[test]
    fn wildcards_match_anything() {
        let s = Selector::parse("*").expect("parses");
        assert!(s.matches_entrant("anything"));
        assert!(s.matches_variant("anything"));
    }

    #[test]
    fn display_round_trips_through_parse() {
        for raw in ["spate:tier-a:0.1.0", "*:*:*", "flink@img:1.2"] {
            let s = Selector::parse(raw).expect("parses");
            let again = Selector::parse(&s.to_string()).expect("reparses");
            assert_eq!(s, again, "{raw}");
        }
    }
}
