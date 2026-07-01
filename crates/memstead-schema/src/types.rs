//! Schema-as-artifact type format.
//!
//! Serde-based, YAML-authorable type definitions.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Complete type definition — serde/schemars-based, authored in YAML.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeDefinition {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    #[serde(default)]
    pub boundaries: Vec<String>,
    #[serde(default)]
    pub examples: Vec<TypeExample>,
    #[serde(default)]
    pub system_message: Option<String>,
    pub sections: Vec<SectionDef>,
    pub metadata_fields: Vec<MetadataFieldDef>,
    pub title_weight: f32,
    pub text_fields: Vec<String>,
    pub hierarchy_relationship: String,
    #[serde(default)]
    pub edge_weight_overrides: IndexMap<String, f32>,
    pub propagating_relationships: Vec<String>,
    pub updatable_fields: Vec<String>,
    pub health_required_fields: Vec<String>,
    pub staleness_threshold_days: u32,
    pub write_rules: Vec<String>,
    /// Outgoing-edge invariants the schema asserts for this type.
    /// Each entry names a list of relationship names plus a cardinality
    /// constraint. The engine evaluates these on every `memstead_create` /
    /// `memstead_update` (post-application of inline `relations:` / patches)
    /// and surfaces unsatisfied blocks as a single
    /// `MISSING_REQUIRED_OUTGOING` warning per entity. Tier-2 (warn,
    /// never block). Empty default — types without `required_outgoing`
    /// keep current behaviour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_outgoing: Vec<RequiredOutgoing>,
    // Populated by loader from schema-level defaults merged with
    // edge_weight_overrides. Skipped during serialization so the on-disk
    // form round-trips.
    #[serde(skip)]
    pub edge_weights: IndexMap<String, f32>,
}

/// One outgoing-edge requirement block on a type definition. Lists one
/// or more relationship names and a cardinality constraint they must
/// jointly satisfy. The schema author groups multiple alternative
/// relationships into a single block when "any of these" satisfies the
/// rule (e.g. `[CHOSEN, REJECTED]` together with `at_least_one` would
/// require at least one outgoing edge across both names — but the
/// planning schema lists each as its own block instead, so each block
/// gets its own warning entry).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequiredOutgoing {
    /// Edge names the rule applies to. Loader validates each against
    /// the schema's declared relationship vocabulary; unknown names
    /// raise `SchemaLoadError::UndeclaredRelationship`.
    pub relationships: Vec<String>,
    pub cardinality: RequiredCardinality,
}

/// Required-cardinality variants. `AtLeastOne` is the only variant
/// shipped initially; `ExactlyOne` is the obvious next variant but is
/// not yet wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequiredCardinality {
    AtLeastOne,
}

impl std::fmt::Display for RequiredCardinality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RequiredCardinality::AtLeastOne => "at_least_one",
        })
    }
}

impl RequiredOutgoing {
    /// Returns `true` iff `outgoing_count` of edges across the block's
    /// `relationships` list satisfies the declared cardinality.
    pub fn admits(&self, outgoing_count: usize) -> bool {
        match self.cardinality {
            RequiredCardinality::AtLeastOne => outgoing_count >= 1,
        }
    }
}

impl TypeDefinition {
    /// Look up an edge weight by relationship name. Falls back to `_default`
    /// when the relationship is unknown; returns `1.0` only if `_default`
    /// itself is missing (the loader normally guarantees it exists).
    pub fn edge_weight(&self, rel: &str) -> f32 {
        if let Some(&w) = self.edge_weights.get(rel) {
            return w;
        }
        if let Some(&w) = self.edge_weights.get("_default") {
            return w;
        }
        1.0
    }

    pub fn section(&self, key: &str) -> Option<&SectionDef> {
        self.sections.iter().find(|s| s.key == key)
    }

    pub fn catch_all_section(&self) -> Option<&SectionDef> {
        self.sections.iter().find(|s| s.catch_all)
    }

    pub fn metadata_field(&self, key: &str) -> Option<&MetadataFieldDef> {
        self.metadata_fields.iter().find(|f| f.key == key)
    }

    /// Closest declared metadata-field key for a typo, used by the CRUD layer
    /// to build a "did you mean ..." hint when rejecting an unknown key.
    pub fn suggest_metadata_field(&self, key: &str) -> Option<String> {
        crate::schema::closest_match(key, self.metadata_fields.iter().map(|f| f.key.as_str()))
    }

    /// Closest declared section key for a typo. Used by the CRUD layer to
    /// build the `UNKNOWN_SECTION` envelope's `suggestion` field on inbound
    /// create/update writes.
    pub fn suggest_section(&self, key: &str) -> Option<String> {
        crate::schema::closest_match(key, self.sections.iter().map(|s| s.key.as_str()))
    }

    /// Required sections in declaration order.
    pub fn required_sections(&self) -> impl Iterator<Item = &SectionDef> {
        self.sections.iter().filter(|s| s.required)
    }

    /// Optional sections in declaration order.
    pub fn optional_sections(&self) -> impl Iterator<Item = &SectionDef> {
        self.sections.iter().filter(|s| !s.required)
    }

    /// System message as a string — empty if unset.
    pub fn system_message_str(&self) -> &str {
        self.system_message.as_deref().unwrap_or("")
    }
}

/// Inline few-shot example — concrete entity content matching this type.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeExample {
    pub title: String,
    pub sections: IndexMap<String, String>,
}

/// A section within an entity (e.g. "Claim", "Evidence").
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionDef {
    pub key: String,
    pub heading: String,
    pub required: bool,
    pub search_weight: f32,
    #[serde(default)]
    pub catch_all: bool,
    #[serde(default)]
    pub write_rules: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A metadata (frontmatter) field.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MetadataFieldDef {
    pub key: String,
    pub description: String,
    pub field_type: FieldType,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub init_timestamp: bool,
    #[serde(default)]
    pub auto_timestamp: bool,
    #[serde(default)]
    pub serialization: Serialization,
    #[serde(default)]
    pub filterable: Filterable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Number,
    Date,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Serialization {
    #[default]
    Default,
    CsvArray,
    OmitWhenFalsy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Filterable {
    #[default]
    None,
    Equality,
    Range,
}

impl Filterable {
    /// Agent-facing wire token for this posture, or `None` when the field
    /// is not filterable. Single source of truth for the string both MCP
    /// schema projections (`memstead_schema`) emit so an agent reads a field's
    /// `filters` / `range_filters` eligibility straight from the schema
    /// body instead of trial-and-error against filter warnings.
    pub fn as_wire_str(self) -> Option<&'static str> {
        match self {
            Filterable::None => None,
            Filterable::Equality => Some("equality"),
            Filterable::Range => Some("range"),
        }
    }
}
