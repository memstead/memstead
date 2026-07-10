//! Pipeline primitives — **Medium · Facet · Projection · Ingest**.
//!
//! The four-primitive model that replaces the conflated Scope / Projection /
//! Ingest shape. The conceptual boundaries between the four
//! primitives are normative; this module is the engine-side
//! data shape that the workspace store persists and the pipeline loader
//! exposes.
//!
//! - [`Medium`] — *territory*: a passive, named, typed reference to a body of
//!   information (no selection logic, no engagement metadata, no preparation).
//! - [`Facet`] — *engagement*: how a projection reads/writes a medium —
//!   a selection (allow/deny patterns), an engagement contract, and an
//!   optional deterministic preparation step.
//! - [`Projection`] — *obligation*: maps source facets (+ optional reference
//!   mems) to a destination mem. The one place agent reasoning lives.
//! - [`Ingest`] — *schedule*: runs a projection in a mode/trigger/batch.
//!
//! These are operator-edited configs. The loader's job is load + validate +
//! expose read-only; nothing here fetches, transforms, or schedules.

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

/// A **Medium** — a passive, named, typed reference to a body of information
/// the mem acknowledges as part of its territory. Nothing more: no
/// selection, no engagement metadata, no preparation step.
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

/// A **Facet** — a named way a projection engages with a [`Medium`]: the
/// subset in reach, the engagement contract, and an optional preparation step.
///
/// The facet record is deliberately heterogeneous (a *source* facet typically
/// carries `scope` + `preparation`; a *destination* facet carries engagement
/// discipline) — forcing a uniform shape would smuggle complexity elsewhere.
/// The single-type-with-optional-fields modelling is chosen for machinery
/// simplicity (concept-doc Open Question 2); the `engagement` contract stays a
/// free-form JSON value because its shape is medium-type- and side-specific
/// (verbs, tools, terminology, discipline) and is not load-bearing for the
/// loader's structural validation.
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

/// A **Projection** — the obligation that connects source facets (and optional
/// read-only reference mems) to a single destination mem. The only place
/// agent reasoning lives; it carries no scope, preparation, or medium metadata
/// of its own (all of that lives in the facets it references).
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

/// How an [`Ingest`] run engages its projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IngestMode {
    /// Build out new coverage.
    Discovery,
    /// Improve existing coverage.
    Refinement,
    /// A single bounded pass.
    OneShot,
}

/// What sets an [`Ingest`] running.
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

/// An **Ingest** — a runnable schedule that runs a [`Projection`] in a given
/// mode, on a trigger, in batches, with optional per-run deny-path overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ingest {
    /// The projection (by name) this ingest runs.
    pub projection: String,
    /// Discovery / refinement / one-shot.
    pub mode: IngestMode,
    /// Loop / manual / on-event.
    pub trigger: IngestTrigger,
    /// How many artifacts a single run processes.
    pub batch_size: u32,
    /// Paths excluded for this ingest's runs, on top of facet scope.
    #[serde(default)]
    pub deny_paths: Vec<String>,
    /// Free-form post-run actions (e.g. a one-shot `archive_source` flag).
    /// Opaque to the engine — consumed only by the one-shot brief renderer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_actions: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// An ingest round-trips; `mode`/`trigger` use the kebab-case wire forms
    /// (`one-shot`, `on-event`) the enum renames declare.
    #[test]
    fn ingest_round_trips_with_kebab_mode_and_trigger() {
        let i = Ingest {
            projection: "macos/graph".to_string(),
            mode: IngestMode::Discovery,
            trigger: IngestTrigger::Loop,
            batch_size: 20,
            deny_paths: vec!["VISION.md".to_string(), "dev".to_string()],
            post_actions: None,
        };
        let json = serde_json::to_string(&i).unwrap();
        assert!(json.contains(r#""mode":"discovery""#), "got {json}");
        assert!(json.contains(r#""trigger":"loop""#), "got {json}");
        let back: Ingest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, i);

        // The kebab-case variants serialise as the doc names.
        let one_shot = serde_json::to_string(&IngestMode::OneShot).unwrap();
        assert_eq!(one_shot, r#""one-shot""#);
        let on_event = serde_json::to_string(&IngestTrigger::OnEvent).unwrap();
        assert_eq!(on_event, r#""on-event""#);
    }
}
