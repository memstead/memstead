//! Pipeline primitives — the inline [`Source`] of a binding and its parts.
//!
//! A pipeline is **one record**: the versioned [`crate::binding::Binding`]
//! (v2) under `projections/<mem>/<name>.json`, which alone fully defines the
//! obligation — intent, inline sources, reference mems, destination, deny
//! paths, coverage semantics, and operations. [`Source`] is the record's
//! inline source entry; *medium* and *facet* survive only as the names of a
//! source description's two halves (where it lives / which part of it),
//! never as standalone records.
//!
//! - The **medium half** of a [`Source`]: `type` / `pointer` /
//!   `change_detection` — a typed reference to a body of information.
//! - The **facet half**: `scope` (allow/deny patterns), an optional
//!   `engagement` contract, and an optional deterministic `preparation` step.
//!
//! These are operator-edited configs. The loader's job is load + validate +
//! expose read-only; nothing here fetches, transforms, or schedules.

use serde::{Deserialize, Serialize};

/// What kind of surface a [`Source`] references (its `type` field, lowercase
/// on the wire). `pdf` (and other non-text mediums) join this enum with
/// their follow-up plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediumType {
    /// A source tree of code.
    Codebase,
    /// A directory of files (non-code).
    Filesystem,
    /// Another mem's graph (reachable as the reserved id `graph` for "home").
    Graph,
    /// A git history.
    Git,
    /// Web sources.
    Web,
}

/// One inline **source** of a v2 [`crate::binding::Binding`] — the full
/// description of a body of information the pipeline reads, carrying both
/// halves the retired standalone records used to split: the *medium* half
/// (where it lives — `type` / `pointer` / `change_detection`) and the
/// *facet* half (which part of it — `scope` / `engagement` / `preparation`).
///
/// `name` is required and unique within the record: it keys per-source
/// sync/verify state (`<mem>/<binding>/<source>#synced`) exactly as facet
/// names did before the consolidation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Stable name — keys per-source sync/verify state.
    pub name: String,
    /// What kind of surface this source references (the medium half).
    #[serde(rename = "type")]
    pub medium_type: MediumType,
    /// Where the body of information lives — a path, URL, or mem id,
    /// interpreted per [`Self::medium_type`]. Opaque to this layer.
    pub pointer: String,
    /// Optional declared change-detection strategy — `none` / `git` /
    /// `mtime` / `auto`. Unset (the common case) means `auto`: the ingest
    /// resolver probes for a git work tree over [`Self::pointer`] and picks
    /// `git` or `mtime`. A graph-typed source ignores this and always uses
    /// the graph snapshot signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_detection: Option<String>,
    /// Allow/deny selection over the source (the facet half). A source with
    /// **no allow patterns is *unscoped*** — a typed refusal at run time (no
    /// strategy diffs or enumerates the whole territory; the brief reports
    /// it as unmonitored), not "everything". A source that truly wants
    /// everything writes `**/*`.
    #[serde(default)]
    pub scope: Vec<PatternEntry>,
    /// Engagement contract — verbs, tools, terminology, discipline.
    /// Free-form because the shape differs by medium type; the engine does
    /// not interpret it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engagement: Option<serde_json::Value>,
    /// Optional deterministic preparation — the identifier of a
    /// preparation registered in the engine's [`crate::preparation`]
    /// registry (today `entity-load-bearing` on graph sources and
    /// `dated-entries` on path-shaped ones). At most one per source.
    /// The edit/validate paths refuse an identifier the registry does not
    /// know ([`crate::binding::CapabilityError::PreparationUnsupported`]);
    /// a record that acquired an unknown one by hand is accepted at rest and
    /// reported unsupported at run time (the brief prints "Skipping." and
    /// exits 0) — both paths apply the one registry rule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<String>,
}

/// Whether a [`PatternEntry`] admits or excludes the matched paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatternMode {
    /// Paths matching this pattern are in reach.
    Allow,
    /// Paths matching this pattern are excluded.
    Deny,
}

/// One allow/deny glob in a [`Source`]'s scope over its medium.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternEntry {
    /// Glob pattern, interpreted relative to the source's pointer.
    pub path: String,
    /// Whether the pattern admits or excludes.
    pub mode: PatternMode,
}

/// What sets a binding's operation running — the `trigger` of a
/// [`crate::binding::BuildOperation`] / `SyncOperation` / `VerifyOperation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IngestTrigger {
    /// Repeated runs (the ingest skill loops it).
    Loop,
    /// Operator-initiated.
    Manual,
    /// Fired by an external event.
    OnEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A v2 inline source round-trips: the medium half (`type` lowercase on
    /// the wire, unset `change_detection` omitted) and the facet half
    /// (`scope` present, unset `engagement`/`preparation` omitted) in one
    /// record — the plan's wire example shape.
    #[test]
    fn source_round_trips_with_both_halves() {
        let s = Source {
            name: "source-tree".to_string(),
            medium_type: MediumType::Codebase,
            pointer: "../public".to_string(),
            change_detection: None,
            scope: vec![
                PatternEntry {
                    path: "../public/**/*.rs".to_string(),
                    mode: PatternMode::Allow,
                },
                PatternEntry {
                    path: "../public/target/**".to_string(),
                    mode: PatternMode::Deny,
                },
            ],
            engagement: None,
            preparation: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""type":"codebase""#), "got {json}");
        assert!(json.contains(r#""mode":"deny""#), "got {json}");
        for absent in ["change_detection", "engagement", "preparation"] {
            assert!(!json.contains(absent), "unset {absent} omitted: {json}");
        }
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    /// A source declaring the optional slots round-trips them.
    #[test]
    fn source_optional_slots_round_trip_when_set() {
        let s = Source {
            name: "manual-pages".to_string(),
            medium_type: MediumType::Filesystem,
            pointer: "../docs".to_string(),
            change_detection: Some("mtime".to_string()),
            scope: Vec::new(),
            engagement: Some(serde_json::json!({ "readVerb": "Read PDF" })),
            preparation: Some("pdf-to-markdown".to_string()),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains(r#""change_detection":"mtime""#), "got {json}");
        let back: Source = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    /// `IngestTrigger`'s kebab-case variants serialise as the doc names
    /// (`on-event`) — the wire forms a binding's operation `trigger` uses.
    #[test]
    fn ingest_trigger_uses_kebab_wire_forms() {
        let on_event = serde_json::to_string(&IngestTrigger::OnEvent).unwrap();
        assert_eq!(on_event, r#""on-event""#);
        let loop_ = serde_json::to_string(&IngestTrigger::Loop).unwrap();
        assert_eq!(loop_, r#""loop""#);
    }
}
