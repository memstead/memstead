//! Schema manifest (`schema.yaml`) — the outer envelope declaring a schema
//! package: name, version, type list, relationship vocabulary, community
//! defaults, and LLM-facing documentation.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaManifest {
    pub name: String,
    /// Semver string — parsed into `semver::Version` by the loader.
    pub version: String,
    pub description: String,
    pub when_to_use: String,
    #[serde(default)]
    pub system_message: Option<String>,
    pub types: Vec<String>,
    pub relationships: RelationshipVocabulary,
    pub community: CommunityConfig,
    /// Schema-generic writing guidance — `avoid` and `goal` prose that
    /// applies to every vault pinned to this schema. The plugin layer
    /// concatenates these with per-vault `writeGuidance.avoid_additions`
    /// / `goal_additions` (an opaque pass-through on the engine side —
    /// see `VaultConfig::write_guidance`'s contract).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_writing_guidance: Option<DefaultWritingGuidance>,
    /// Outbound cross-vault relationship vocabulary, per target schema
    /// domain. Each entry names a target schema (bare name — never a
    /// version; eligibility is name-based) and lists rel-types that may
    /// cross the boundary in that direction. Absent or `[]` means the
    /// schema declares no outbound cross-vault edges. Source-ownership
    /// only — third-party bridge schemas are not modelled; each
    /// direction is owned by exactly one schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cross_vault_relationships: Vec<CrossVaultRelationshipEntry>,
    /// Schema-level pointer naming the rel-type that body wiki-links
    /// `[[target]]` should auto-emit as engine-synthesised relations.
    /// `None` (default) means the schema is opt-out of alias synthesis
    /// — unbacked body wiki-links continue to refuse with
    /// `WIKILINK_WITHOUT_RELATION`. When set, the named rel-type must
    /// be declared in `relationships.definitions` or schema load
    /// fails with `SchemaLoadError::AliasTargetRelTypeNotDeclared`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_target_rel_type: Option<String>,
}

/// One outbound cross-vault declaration — a target schema domain
/// (named, never versioned) and the rel-types admitted in that
/// direction.
///
/// `target_types` strings within each definition live in the target
/// schema's namespace by construction — the source schema's loader
/// accepts them as opaque since the target schema may not be present
/// at source-schema load time.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CrossVaultRelationshipEntry {
    /// Bare name of the target schema — the domain identity. A version
    /// suffix (`software@1.0.0`) or range (`software@^1.0`) is rejected
    /// at schema load: cross-vault eligibility is name-based, so the
    /// declaration is satisfied by a target vault pinning *any* version
    /// of the named schema.
    pub to_schema: String,
    pub definitions: Vec<RelationshipDef>,
}

/// Schema-level writing-guidance defaults. Both fields are optional so a
/// schema can ship `avoid` without a `goal` (or vice versa). The engine
/// surfaces them via `build_schema_payload` at the top level of the
/// schema-payload JSON; resolution (concatenation with vault additions)
/// lives in the plugin layer.
#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DefaultWritingGuidance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationshipVocabulary {
    pub mode: RelationshipMode,
    pub definitions: Vec<RelationshipDef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RelationshipMode {
    Strict,
    Open,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RelationshipDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub when_to_use: Option<String>,
    pub default_weight: f32,
    /// Per-edge description posture for edges of this rel-type:
    /// `forbidden` (default) rejects any trailing description text;
    /// `optional` accepts edges with or without a description;
    /// `required` rejects edges without a description. The schema
    /// author opts a catch-all rel-type (e.g. `OTHER`) into
    /// `required` to force per-edge documentation; most rel-types
    /// keep the default `forbidden` posture so the rel-type's name
    /// is the edge's documentation.
    #[serde(default)]
    pub per_edge_description: PerEdgeDescription,
    /// When true, the engine rejects writes that would close a cycle in the
    /// subgraph restricted to edges of this relationship type. Defaults to
    /// false so existing user schemas stay opt-in. Semantically meaningless
    /// on the `_default` sentinel (never a real edge's rel_type).
    #[serde(default)]
    pub acyclic: bool,
    /// Schema-declared types whose entities may be the source of this
    /// edge. Empty (default) means shape-free — any source type admitted.
    /// The loader validates every entry against the schema's declared
    /// types list; unknown names raise `SchemaLoadError::UndeclaredType`.
    /// At write time, `memstead_relate` rejects shape violations with
    /// `INVALID_REL_SHAPE`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_types: Vec<String>,
    /// Same as `source_types` but for the target side. Empty = shape-free.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_types: Vec<String>,
    /// Per-source cardinality hint, parsed and stored on the
    /// relationship definition. Declarative only — the engine does not
    /// currently enforce it or warn when a relate pushes the source's
    /// outgoing count for this rel_type outside the declared range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinality_per_source: Option<Cardinality>,
    /// Manual-authoring posture for this rel-type. `allow` (default)
    /// admits explicit `memstead_relate` calls. `warn` lands the relation
    /// with a `RELATION_MANUAL_AUTHORING_NOT_RECOMMENDED` warning.
    /// `forbidden` refuses explicit-author calls with the typed
    /// `RELATION_MANUAL_AUTHORING_FORBIDDEN` code. The body-link →
    /// relation alias machinery (`memstead_update` / `memstead_create`'s
    /// wiki-link parser) is NOT gated — schema-emitted relations like
    /// REFERENCES synthesise unchanged.
    #[serde(default)]
    pub manual_authoring: ManualAuthoring,
}

/// Per-edge description posture declared on a `RelationshipDef`.
///
/// `Forbidden` (the default) rejects any trailing description text on
/// edges of this rel-type. `Optional` accepts both shapes. `Required`
/// rejects edges without a description — the schema author opts a
/// catch-all rel-type into this so every edge carries its own
/// rationale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PerEdgeDescription {
    #[default]
    Forbidden,
    Optional,
    Required,
}

/// Manual-authoring posture declared per `RelationshipDef`.
///
/// `Allow` (default) is the no-op posture for every rel-type a user
/// or agent may author explicitly via `memstead_relate`. `Warn` lands the
/// relation but surfaces a warning so the audit trail records the
/// drift. `Forbidden` refuses with a typed code — used for rel-types
/// the engine emits via the body-link → relation alias machinery
/// (e.g. REFERENCES), where explicit authoring duplicates work and
/// often masks the author's intent. The schema's `when_to_use` text
/// rides on the wire as recovery guidance.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ManualAuthoring {
    #[default]
    Allow,
    Warn,
    Forbidden,
}

/// Allowed cardinality ranges for `RelationshipDef::cardinality_per_source`.
/// Stringly-typed parsing rejected — typos surface at YAML load time via
/// `serde`, the warning builder gets exhaustive matches, and the wire
/// payload renders via `Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub enum Cardinality {
    #[serde(rename = "1")]
    One,
    #[serde(rename = "0..1")]
    ZeroOrOne,
    #[serde(rename = "1..N")]
    OneOrMore,
    #[serde(rename = "0..N")]
    ZeroOrMore,
}

impl std::fmt::Display for Cardinality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Cardinality::One => "1",
            Cardinality::ZeroOrOne => "0..1",
            Cardinality::OneOrMore => "1..N",
            Cardinality::ZeroOrMore => "0..N",
        };
        f.write_str(s)
    }
}

impl Cardinality {
    /// Returns `true` iff `count` falls inside the allowed range. Used by
    /// `memstead_relate` to predict whether a post-mutation outgoing count
    /// would violate the schema's intent.
    pub fn admits(&self, count: usize) -> bool {
        match self {
            Cardinality::One => count == 1,
            Cardinality::ZeroOrOne => count <= 1,
            Cardinality::OneOrMore => count >= 1,
            Cardinality::ZeroOrMore => true,
        }
    }
}

/// Community-detection (Louvain) defaults — schema-level, not per-type.
///
/// Distinct from the legacy `schemas::CommunityConfig` which was attached to
/// each `TypeDefinition`; the legacy variant has been removed.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommunityConfig {
    pub resolution: f64,
    pub seed: u32,
}
