//! Pipeline primitives — the inline [`Source`] and the migrate-local legacy
//! shapes.
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
//!
//! [`Medium`] / [`Facet`] / [`Projection`] are **migrate-local** legacy
//! shapes: parsed only by `memstead projection migrate`'s conversion legs
//! (gen-1 root-folder, gen-2 four-primitive, v1 three-file). No live path
//! constructs or reads them, same as [`crate::pipeline_store::LegacyIngest`].

use serde::{Deserialize, Serialize};

/// What kind of surface a [`Medium`] references. The string forms match the
/// `type` field of the legacy `scopes/<mem>/<name>.json` records, so
/// the migration shim maps them without translation. `pdf` (and other
/// non-text mediums) join this enum with their follow-up plans.
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
/// names did before the consolidation, which is why migration preserves
/// facet names as source names byte-verbatim.
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
    /// Optional deterministic preparation step (string identifier, e.g.
    /// `pdf-to-markdown`). Unset for every text source today. A source that
    /// names a preparation the engine has no implementation for is refused
    /// at validation time — no silent skip, no crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<String>,
}

/// Legacy **Medium** (migrate-local) — the standalone territory record of the
/// retired three-file store. Parsed only by the migration legs; the live
/// model carries this content inline as a [`Source`]'s medium half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Medium {
    /// Stable name — facets and projections reference a medium by this.
    pub name: String,
    /// What kind of surface this is.
    #[serde(rename = "type")]
    pub medium_type: MediumType,
    /// Where the body of information lives — a path, URL, or mem id,
    /// interpreted per [`Self::medium_type`]. Opaque to this layer.
    pub pointer: String,
    /// An optional declared change-detection strategy for sources reading
    /// this medium — `none` / `git` / `mtime` / `auto`. Unset (the common
    /// case) means `auto`: the ingest resolver probes for a git work tree
    /// over [`Self::pointer`] and picks `git` or `mtime`. A graph-typed
    /// medium ignores this and always uses the graph snapshot signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_detection: Option<String>,
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

/// One allow/deny glob in a [`Facet`]'s selection over its medium. Mirrors the
/// `{ path, mode }` entries of the legacy scope `tree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatternEntry {
    /// Glob pattern, interpreted relative to the referenced medium's pointer.
    pub path: String,
    /// Whether the pattern admits or excludes.
    pub mode: PatternMode,
}

/// Legacy **Facet** (migrate-local) — the standalone engagement record of
/// the retired three-file store. Parsed only by the migration legs; the live
/// model carries this content inline as a [`Source`]'s facet half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facet {
    /// Stable name — projections reference a facet by this.
    pub name: String,
    /// The [`Medium`] (by name) this facet is a perspective on. A facet
    /// always references exactly one medium.
    pub medium: String,
    /// Allow/deny selection over the referenced medium. A facet with **no
    /// allow patterns is *unscoped*** — a typed refusal at run time (no
    /// strategy diffs or enumerates the whole medium; the brief reports it as
    /// unmonitored), not "whole medium". A facet that truly wants everything
    /// writes `**/*`.
    #[serde(default)]
    pub scope: Vec<PatternEntry>,
    /// Engagement contract — verbs, tools, terminology, discipline. Free-form
    /// because the shape differs by medium type and by source/destination
    /// side; the engine does not interpret it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engagement: Option<serde_json::Value>,
    /// Optional deterministic preparation step (string identifier, e.g.
    /// `pdf-to-markdown`). Unset for every text medium today. A facet that
    /// names a preparation the engine has no implementation for is accepted at
    /// rest but reported unsupported at run time — no silent skip, no crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<String>,
}

/// Legacy **Projection** (migrate-local) — the gen-2 obligation record that
/// referenced facets by name. Parsed only by the migration legs; the live
/// obligation is the v2 [`crate::binding::Binding`] with inline sources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Projection {
    /// What the projection is trying to accomplish — prose for the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Source facets (by name) the projection consumes.
    #[serde(default)]
    pub source_facets: Vec<String>,
    /// Read-only reference mems that supply cross-mem context.
    #[serde(default)]
    pub reference_mems: Vec<String>,
    /// The mem this projection writes into.
    pub destination_mem: String,
    /// Free-form projection rules (e.g. a one-shot lens `routing` string).
    /// Opaque to the engine — consumed only by the one-shot brief renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<serde_json::Value>,
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

    /// A medium round-trips and its `type` serialises to the lowercase form
    /// the legacy scope JSON used.
    #[test]
    fn medium_round_trips_with_lowercase_type() {
        let m = Medium {
            name: "source-tree".to_string(),
            medium_type: MediumType::Codebase,
            pointer: "../macos".to_string(),
            change_detection: None,
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(
            !json.contains("change_detection"),
            "unset change_detection is omitted on the wire: {json}"
        );
        assert!(json.contains(r#""type":"codebase""#), "got {json}");
        let back: Medium = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    /// A medium declaring a `change_detection` strategy round-trips with the
    /// value present; the field is the optional slot the ingest resolver
    /// reads to pick a source's change-detection strategy.
    #[test]
    fn medium_change_detection_round_trips_when_set() {
        let m = Medium {
            name: "manuals".to_string(),
            medium_type: MediumType::Filesystem,
            pointer: "../docs".to_string(),
            change_detection: Some("mtime".to_string()),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains(r#""change_detection":"mtime""#), "got {json}");
        let back: Medium = serde_json::from_str(&json).unwrap();
        assert_eq!(back.change_detection.as_deref(), Some("mtime"));
        assert_eq!(back, m);
    }

    /// A source facet with allow/deny scope and no preparation round-trips,
    /// and the unset `preparation`/`engagement` keys are omitted on the wire.
    #[test]
    fn facet_round_trips_and_omits_unset_optional_fields() {
        let f = Facet {
            name: "source-files".to_string(),
            medium: "source-tree".to_string(),
            scope: vec![
                PatternEntry {
                    path: "../macos/**/*.swift".to_string(),
                    mode: PatternMode::Allow,
                },
                PatternEntry {
                    path: "../macos/specs/**".to_string(),
                    mode: PatternMode::Deny,
                },
            ],
            engagement: None,
            preparation: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(
            !json.contains("preparation"),
            "unset preparation omitted: {json}"
        );
        assert!(
            !json.contains("engagement"),
            "unset engagement omitted: {json}"
        );
        assert!(json.contains(r#""mode":"deny""#), "got {json}");
        let back: Facet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.preparation, None);
    }

    /// A facet declaring a preparation identifier round-trips with the value
    /// present — the slot is reserved even though no implementation exists.
    #[test]
    fn facet_preparation_slot_round_trips_when_set() {
        let f = Facet {
            name: "manual-pages".to_string(),
            medium: "manuals".to_string(),
            scope: Vec::new(),
            engagement: Some(serde_json::json!({ "readVerb": "Read PDF" })),
            preparation: Some("pdf-to-markdown".to_string()),
        };
        let json = serde_json::to_string(&f).unwrap();
        let back: Facet = serde_json::from_str(&json).unwrap();
        assert_eq!(back.preparation.as_deref(), Some("pdf-to-markdown"));
        assert_eq!(back, f);
    }

    /// A projection maps source facets + reference mems to one destination.
    #[test]
    fn projection_round_trips() {
        let p = Projection {
            intent: Some("Swift macOS app source.".to_string()),
            source_facets: vec!["source-files".to_string()],
            reference_mems: vec!["engine".to_string()],
            destination_mem: "macos".to_string(),
            rules: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        let back: Projection = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.destination_mem, "macos");
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
