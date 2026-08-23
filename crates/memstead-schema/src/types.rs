//! Schema-as-artifact type format.
//!
//! Serde-based, YAML-authorable type definitions.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Complete type definition — serde/schemars-based, authored in YAML.
/// `skip_serializing_if` helper for default-false flags.
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TypeDefinition {
    pub name: String,
    pub description: String,
    pub when_to_use: String,
    #[serde(default)]
    pub boundaries: Vec<String>,
    /// One canonical, ENGINE-VALIDATED exemplar entity for this type
    /// (agent-trust plan 09) — the few-shot material an authoring
    /// agent actually learns from. Validated against this very type
    /// through the real create path (`dry_run`) at schema
    /// install/seal time: a package whose exemplar does not conform
    /// refuses with a typed error naming the type and the defect —
    /// there is no warn-and-carry mode, so an exemplar can never
    /// drift into teaching the wrong shape. Served at
    /// `verbosity: full` only (the lite skeleton stays unchanged).
    /// Optional per type; the built-in reference schemas are complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exemplar: Option<Exemplar>,
    /// Legacy-key sentinel for the retired `examples:` list — a field
    /// that promised few-shot teaching but was never validated nor
    /// served by any surface (dead vocabulary). Authoring contexts
    /// refuse it with a typed error naming `exemplar`; sealed
    /// contexts tolerate and drop it. Never serialized.
    #[serde(default, rename = "examples", skip_serializing)]
    #[schemars(skip)]
    pub legacy_examples: Option<serde_yaml_ng::Value>,
    #[serde(default)]
    pub system_message: Option<String>,
    pub sections: Vec<SectionDef>,
    pub metadata_fields: Vec<MetadataFieldDef>,
    pub title_weight: f32,
    pub text_fields: Vec<String>,
    pub hierarchy_relationship: String,
    #[serde(default)]
    pub edge_weight_overrides: IndexMap<String, f32>,
    /// Rel-types on which `memstead_relate` refuses a SELF-LOOP
    /// (from == to) when this type is the source. That refusal is this
    /// field's ONLY effect — it propagates nothing, implies no
    /// evidence obligation (real impact propagation is the
    /// `status_propagation` constraint). Renamed from the misleading
    /// `propagating_relationships` (agent-trust plan 06): the old key
    /// refuses at authoring/install load with a typed error naming
    /// this one; sealed content (built-ins, installed refs) loads
    /// with the old key translated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_self_loop_relationships: Vec<String>,
    /// Legacy-key sentinel: captures a `propagating_relationships:`
    /// value during deserialization so the loader can refuse (strict
    /// authoring contexts) or translate (sealed contexts). Never
    /// serialized; never part of the payload surface.
    #[serde(default, rename = "propagating_relationships", skip_serializing)]
    #[schemars(skip)]
    pub legacy_propagating_relationships: Option<Vec<String>>,
    /// Terminal-by-construction marker: entities of this type are
    /// leaves — they carry no edges BY DESIGN, so health's orphan
    /// axis exempts their edge-less entities and reports them as a
    /// separate leaf population instead (visible, never vanished).
    /// Leaf means "no edges required", not "edges forbidden": a
    /// leaf-typed entity WITH edges stays legal, and every other
    /// health axis, search, and traversal treats leaf entities
    /// exactly like any other. Declarative per-type flag (the sixth
    /// declarative form the agent-toolbox constraint vocabulary
    /// anticipated), served at both schema verbosity levels.
    #[serde(default, skip_serializing_if = "is_false")]
    pub leaf: bool,
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
    /// Declared reachability obligations (see [`MustReach`]): entities
    /// of this type must reach at least one entity of a named terminal
    /// type, following edges of a named relation set in a named
    /// direction, within an optional maximum depth. Health-path only,
    /// always warn-tier (the loader refuses `block`: a transitive
    /// property is established by writes on OTHER entities, so a
    /// write-time refusal would punish the wrong mutation). Empty
    /// default keeps current behaviour.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub must_reach: Vec<MustReach>,
    /// Declared keep-health constraints (the constraint vocabulary —
    /// see [`ConstraintDef`]). Empty default: a schema declaring no
    /// constraints behaves byte-identically to before the vocabulary
    /// existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<ConstraintDef>,
    /// The type's declared due axis (see [`DueAxis`]) — absent for
    /// types without deadline semantics; a schema without any `due:`
    /// declaration behaves byte-identically to before the axis
    /// existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<DueAxis>,
    // Populated by loader from schema-level defaults merged with
    // edge_weight_overrides. Skipped during serialization so the on-disk
    // form round-trips.
    #[serde(skip)]
    pub edge_weights: IndexMap<String, f32>,
    /// Raw author-declared metadata-field keys, recorded by the loader
    /// BEFORE the base-metadata merge injects the engine fields
    /// (`type`, `created_date`, `last_modified`, `tags`). The
    /// install-path reserved-key check
    /// ([`crate::loader::check_reserved_metadata_keys`]) reads this
    /// list so it can refuse an author-declared reserved key without
    /// false-positives on the injected ones. Skipped during
    /// serialization so the on-disk form round-trips.
    #[serde(skip)]
    pub declared_metadata_keys: Vec<String>,
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
    /// Constraint severity (form 4 of the constraint vocabulary):
    /// `warn` (the historical default — health finding +
    /// `MISSING_REQUIRED_OUTGOING` write-time warning) or `block`
    /// (write-time refusal when a create/update would land, or a
    /// relate-remove would leave, the entity below cardinality).
    #[serde(default)]
    pub severity: ConstraintSeverity,
    /// Optional condition: the block applies only when this metadata
    /// field of the entity holds `when_value`. The same two keys
    /// `requires_when` uses — one vocabulary for one idea. The loader
    /// requires the pair to appear together, `when_field` to name a
    /// declared metadata field of this type carrying `enum_values`,
    /// and `when_value` to be a member. Absent pair = unconditional
    /// block = long-standing behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_field: Option<String>,
    /// The triggering value (see `when_field`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_value: Option<String>,
}

/// One reachability obligation on a type definition. The obligated
/// entity must reach at least one non-stub entity whose type is in
/// `terminal_types`, walking edges whose rel-type is in
/// `relationships` (an inline relation set), in `direction`, within
/// `max_depth` hops when bounded. Evaluated on the health sweep only
/// (`constraints` axis), never on the write path — no single write
/// completes a transitive absence. The incoming direction with
/// `max_depth: 1` covers the required-incoming-edge case.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MustReach {
    /// Edge names the walk follows — any name in the set continues the
    /// path. Loader validates each against the schema's relationship
    /// vocabulary.
    pub relationships: Vec<String>,
    /// `out` follows edges pointing away from the walked entity, `in`
    /// follows edges pointing at it — the same vocabulary the store
    /// and `memstead_search` speak.
    pub direction: ReachDirection,
    /// Type names that satisfy the obligation when reached. Loader
    /// validates each against the schema's declared types.
    pub terminal_types: Vec<String>,
    /// Maximum number of hops a conforming path may take. Absent =
    /// unbounded. Zero refuses at load (nothing is reachable in zero
    /// hops).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
    /// Always `warn` — the loader refuses `block` on this form.
    #[serde(default)]
    pub severity: ConstraintSeverity,
}

/// Walk direction for [`MustReach`] — wire literals `out` / `in`,
/// matching the store's relationship rendering and `memstead_search`'s
/// `direction` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReachDirection {
    Out,
    In,
}

impl std::fmt::Display for ReachDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ReachDirection::Out => "out",
            ReachDirection::In => "in",
        })
    }
}

/// Uniform severity for the constraint vocabulary — one model across
/// every constraint form, never five ad-hoc ones. `warn` produces a
/// health finding only; `block` additionally refuses at write time
/// (and still surfaces pre-existing violations in health). Severity
/// applies to every write surface uniformly — operator-mode bypasses
/// allowlists, never validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintSeverity {
    /// Health finding only (and, where a write-time warning exists,
    /// that warning). The default for every form except uniqueness.
    #[default]
    Warn,
    /// Write-time refusal plus health finding for pre-existing
    /// violations.
    Block,
}

impl ConstraintSeverity {
    /// Serde default for forms whose default tier is `block`
    /// (uniqueness — plenum 4's 37 duplicates are the evidence).
    pub fn block() -> Self {
        Self::Block
    }
}

/// One declared keep-health constraint on a type — the constraint
/// vocabulary (agent-toolbox plan 07). Declarations travel sealed with
/// the schema package and are rendered on the `memstead_schema`
/// response at BOTH verbosity levels (a hidden legality condition is a
/// defect class of its own). The `kind` tag is closed: an unknown kind
/// fails deserialization, so no declaration can load and be silently
/// ignored. Forms land vertically — a form is only declarable once the
/// engine evaluates it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConstraintDef {
    /// Form 1 — conditional requirement: `field` (a metadata field or
    /// section key of this type) is required whenever `when_field`
    /// holds `when_value` ("`status: checked` requires `checked_by`").
    RequiresWhen {
        /// The field or section that becomes required.
        field: String,
        /// The metadata field whose value triggers the requirement.
        when_field: String,
        /// The triggering value (validated against `when_field`'s
        /// enum, when it declares one).
        when_value: String,
        #[serde(default)]
        severity: ConstraintSeverity,
    },
    /// Form 2 — uniqueness: the tuple of metadata-field values named
    /// in `fields` is unique among entities of this type within one
    /// mem. Entities missing any of the fields carry no tuple and are
    /// not compared. Defaults to `block` — the whole point of the
    /// declaration is preventing the duplicate at write time.
    Unique {
        /// The metadata fields forming the unique tuple (each must be
        /// a declared metadata field of this type).
        fields: Vec<String>,
        #[serde(default = "ConstraintSeverity::block")]
        severity: ConstraintSeverity,
    },
    /// Form 3 — enum-from-neighbour: the legal values of `field` are
    /// the bullet-list entries (`- value` lines) of the `section`
    /// section on the entity reached from this one via a `rel_type`
    /// edge. A set value with no backing entry in any reached
    /// neighbour's section — including the no-neighbour and
    /// missing-section cases, where nothing can back it — is a
    /// violation.
    EnumFromNeighbour {
        /// The metadata field whose values the neighbour enumerates.
        field: String,
        /// The outgoing rel-type that reaches the enumerating entity.
        rel_type: String,
        /// The section key on the reached entity whose bullet entries
        /// are the legal values.
        section: String,
        #[serde(default)]
        severity: ConstraintSeverity,
    },
    /// Form 5 — status propagation: when `field` on an entity of this
    /// type holds `value` (the terminal value), every entity reaching
    /// it — transitively — via `rel_type` edges in `direction` is
    /// tainted; tainted entities surface as health findings naming
    /// their tainting ancestor. Always warn-tier: the taint arises
    /// from the ancestor's *later* change, so it can never refuse the
    /// descendant's historical write (the loader refuses a `block`
    /// declaration on this form rather than accepting a promise the
    /// engine will not keep).
    StatusPropagation {
        /// The status metadata field on this (the tainting) type.
        field: String,
        /// The terminal value that starts the taint (validated
        /// against `field`'s enum, when it declares one).
        value: String,
        /// The single rel-type the taint travels along. Exactly one
        /// of `rel_type` / `rel_types` per declaration — the loader
        /// refuses both-present and neither-present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rel_type: Option<String>,
        /// The relation SET the taint travels along — the union
        /// subgraph, so a taint crosses rel-type boundaries. Inline
        /// list of declared names, per the bundle-wide convention.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rel_types: Option<Vec<String>>,
        /// Which direction reaches the dependents: `incoming` taints
        /// the entities whose edges point at the terminal entity (and
        /// their dependents, transitively); `outgoing` the entities
        /// the terminal entity points at.
        direction: PropagationDirection,
        #[serde(default)]
        severity: ConstraintSeverity,
    },
}

impl ConstraintDef {
    /// The effective relation set of a `status_propagation`
    /// declaration: the single `rel_type` as a one-element list, or
    /// the declared `rel_types`. The loader guarantees exactly one of
    /// the two is present. Returns `None` for other constraint forms.
    pub fn propagation_rel_types(&self) -> Option<Vec<String>> {
        match self {
            ConstraintDef::StatusPropagation {
                rel_type,
                rel_types,
                ..
            } => match (rel_type, rel_types) {
                (Some(single), None) => Some(vec![single.clone()]),
                (None, Some(set)) => Some(set.clone()),
                // Loader-refused shapes; empty keeps callers total.
                _ => Some(Vec::new()),
            },
            _ => None,
        }
    }
}

/// Traversal direction for [`ConstraintDef::StatusPropagation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PropagationDirection {
    Incoming,
    Outgoing,
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

/// One canonical exemplar entity for a type — a complete entity in the
/// mem markdown shape: title, metadata overrides, section bodies, and
/// relationship entries with placeholder targets. Engine-validated at
/// schema install/seal through the real create path, so it can never
/// teach a shape the validator would refuse.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exemplar {
    /// The exemplar entity's title (drives the id slug exactly as a
    /// real create would).
    pub title: String,
    /// Metadata overrides, keyed by declared field key — validated
    /// like a real create's metadata (enums included). Engine-stamped
    /// fields (`created_date`, …) are omitted; the engine fills them.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub metadata: IndexMap<String, String>,
    /// Section bodies keyed by section key. Required sections must all
    /// be present — the validator enforces it like any create.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub sections: IndexMap<String, String>,
    /// Relationship entries with PLACEHOLDER targets: each `to` is a
    /// bare slug (no `mem--` prefix — an exemplar lives outside any
    /// mem); validation checks rel-type legality and shape, never
    /// target existence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<ExemplarRelation>,
}

/// One relationship entry on an [`Exemplar`].
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExemplarRelation {
    /// Placeholder target: a bare slug, scoped to the exemplar's own
    /// (virtual) mem at validation time.
    pub to: String,
    /// Relationship type (UPPER_SNAKE_CASE; validated against the
    /// schema's declared vocabulary).
    #[serde(rename = "type")]
    pub rel_type: String,
    /// Optional per-edge description — validated against the
    /// rel-type's `per_edge_description` posture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Derive the storage key a `## Heading` line resolves to.
///
/// The single owner of the heading→key mapping: the entity parser uses it
/// to place parsed section content, and the schema loader's round-trip
/// check uses it to refuse schemas whose declared headings could never
/// find their way back to their declared keys. A second copy of this
/// logic is how silent section forks return — both sides must call this
/// function.
///
/// The mapping is deliberately narrow: lowercase the heading, replace
/// spaces with underscores. Anything looser (slugging punctuation,
/// folding diacritics) would let two distinct declared sections collide
/// on one key, trading a visible refusal for an invisible content merge.
pub fn derive_section_key(heading: &str) -> String {
    heading.to_lowercase().replace(' ', "_")
}

/// A section within an entity (e.g. "Claim", "Evidence").
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SectionDef {
    pub key: String,
    pub heading: String,
    /// Whether an entity must carry this section — **absence means
    /// optional**, the same rule metadata fields follow. `required:
    /// true` refuses a create without the section
    /// (`MISSING_REQUIRED_SECTION`).
    #[serde(default)]
    pub required: bool,
    pub search_weight: f32,
    #[serde(default)]
    pub catch_all: bool,
    #[serde(default)]
    pub write_rules: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Declared markdown shape (section-format vocabulary, plan 08):
    /// a flat content expression over the mdast block vocabulary —
    /// see [`crate::content_expr::ContentExpr`]. Absent = free-form,
    /// exactly the pre-declaration behavior. Validated and compiled
    /// at schema load ([`SectionDef::compiled_content`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Regex applied to the repeating unit of the declared `content`
    /// (list items with lazy continuation joined; paragraph source
    /// lines). Implicitly anchored `^…$`; named capture groups name
    /// the parts in refusal payloads. Legal only when `content`
    /// contains exactly one of `list` / `paragraph`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_pattern: Option<String>,
    /// Table contract — only legal when `content` contains `table`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableFormat>,
    /// One conforming snippet, echoed verbatim in every format
    /// refusal — for an agent, a conforming example outperforms any
    /// grammar string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    /// Severity of format violations (plan 07's uniform model).
    /// Default `block`: a shape violation is deterministic and
    /// one-round-trip repairable (the enum-value analogy) — `warn`
    /// stays available per section.
    #[serde(
        default = "ConstraintSeverity::block",
        skip_serializing_if = "severity_is_block"
    )]
    pub format_severity: ConstraintSeverity,
    /// The compiled `content` expression — populated by the loader
    /// (parse once, match per write). Skipped in serialization so the
    /// on-disk form round-trips. `None` when no format is declared OR
    /// the declaration is defective (see `format_problems`).
    #[serde(skip)]
    pub compiled_content: Option<crate::content_expr::ContentExpr>,
    /// Problems the loader found in this section's format declaration.
    /// Same posture as the reserved-metadata-key check: install and
    /// strict validation refuse on these
    /// ([`crate::loader::check_section_formats`]); boot and
    /// sealed-schema loads do NOT — a sealed schema carrying a bad
    /// declaration keeps loading (refusing at boot would brick the
    /// workspace) and the defect surfaces as a health finding. A
    /// defective declaration is never enforced (`compiled_content`
    /// stays `None`).
    #[serde(skip)]
    pub format_problems: Vec<String>,
}

fn severity_is_block(s: &ConstraintSeverity) -> bool {
    *s == ConstraintSeverity::Block
}

/// The table contract of a format-declared section: `columns` pins
/// header names and order; `column_patterns` maps column name → regex
/// per cell (implicitly anchored). Column-count enforcement is ours by
/// decision — GFM silently pads/truncates short or long rows, so a
/// row with the wrong cell count is *our* refusal, not the parser's.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TableFormat {
    pub columns: Vec<String>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub column_patterns: IndexMap<String, String>,
}

/// A type's declared **due axis** (first-author-path plan 08): which
/// of its fields carry deadline semantics, so the engine's due-brief
/// (`memstead due`) can render "what is due next" without knowing any
/// domain vocabulary. Validated at schema load: `date_field` must be
/// a date-typed metadata field of the type, `status_field` an
/// enum-typed one, every `open_values` entry a member of that enum,
/// and `lead_section` (optional — rendered as "what must happen
/// first") a declared section key. The axis is rendering-only: it
/// never enforces anything (constraints own enforcement) and the
/// engine never advances a date (the agent loop is the runtime).
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DueAxis {
    /// Date-typed metadata field holding the deadline.
    pub date_field: String,
    /// Enum-typed metadata field holding the lifecycle status.
    pub status_field: String,
    /// The `status_field` values under which the entity counts as
    /// still open (due-relevant). Every entry must be declared in the
    /// field's `enum_values`.
    pub open_values: Vec<String>,
    /// Optional section key whose content renders with each entry as
    /// "what must happen first".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_section: Option<String>,
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
    /// Whether an entity must carry this field — **absence means
    /// optional**, the same rule sections follow. `required: true`
    /// refuses a create that leaves the field unset
    /// (`REQUIRED_FIELD_UNSET`); a required field with a
    /// `default_value` (or an `init_timestamp`) is auto-filled and
    /// therefore never refused — required-with-default means "always
    /// present", not "caller must type it". Replaces the retired
    /// `optional:` key (opposite polarity): sealed schemas carrying
    /// `optional` keep loading with inverted-but-equivalent
    /// semantics; authoring refuses it naming this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// The retired `optional:` key, captured raw so sealed content
    /// keeps loading (inverted) while authoring refuses it. Never
    /// serialized, never part of the authoring language.
    #[serde(default, rename = "optional", skip_serializing)]
    #[schemars(skip)]
    pub legacy_optional: Option<bool>,
    /// Resolved requiredness — computed at load from `required`,
    /// the retired `optional`, and the package's format generation
    /// (an unmarked sealed package reads absence as required, the
    /// legacy meaning; everything else reads absence as optional).
    /// Read via [`Self::is_required`]; never parsed from YAML.
    #[serde(skip)]
    #[schemars(skip)]
    pub required_resolved: bool,
    #[serde(default)]
    pub init_timestamp: bool,
    #[serde(default)]
    pub auto_timestamp: bool,
    #[serde(default)]
    pub serialization: Serialization,
    #[serde(default)]
    pub filterable: Filterable,
}

impl MetadataFieldDef {
    /// Whether an entity must carry this field, after the load-time
    /// polarity resolution. The single read every validator and
    /// projection uses — `required`/`legacy_optional` are raw parse
    /// captures, not behaviour.
    pub fn is_required(&self) -> bool {
        self.required_resolved
    }
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
