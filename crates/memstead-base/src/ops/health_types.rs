//! Health-summary types and their axes.

use super::*;

// ---------------------------------------------------------------------------
// Health types
// ---------------------------------------------------------------------------

/// Health check result for one entity.
#[derive(Debug, Clone, Serialize)]
pub struct HealthReport {
    pub id: EntityId,
    pub title: String,
    pub score: f32,
    pub issues: Vec<HealthIssue>,
}

/// Machine-readable condition discriminator for a [`HealthIssue`] —
/// the enumeration lives here, with the issue type, and is never
/// re-derived per projection. A projection that lists issues carries
/// the code; the code is NEVER only a message-string prefix (a
/// projection that drops messages would silently collapse distinct
/// conditions — the exact misdirection `SECTION_HEADING_MISMATCH`
/// exists to prevent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum HealthIssueCode {
    /// The required section/field is genuinely absent or empty.
    Missing,
    /// The section's content is present in the file but sits under a
    /// heading that does not derive back to the section key — NOT
    /// missing; fix the schema's heading/key pair.
    SectionHeadingMismatch,
    /// The entity carries a relationship whose rel-type the mem's
    /// schema does not declare.
    UndeclaredRelationship,
    /// An existing edge violates the rel-type's declared
    /// `source_types` / `target_types` shape.
    InvalidRelShape,
}

impl HealthIssueCode {
    /// Stable wire string — matches the serde `SCREAMING_SNAKE_CASE`
    /// serialization, exposed for text renderers.
    pub fn as_wire(&self) -> &'static str {
        match self {
            HealthIssueCode::Missing => "MISSING",
            HealthIssueCode::SectionHeadingMismatch => "SECTION_HEADING_MISMATCH",
            HealthIssueCode::UndeclaredRelationship => "UNDECLARED_RELATIONSHIP",
            HealthIssueCode::InvalidRelShape => "INVALID_REL_SHAPE",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthIssue {
    pub field: String,
    /// Which condition this issue reports — see [`HealthIssueCode`].
    pub code: HealthIssueCode,
    pub message: String,
}

/// One quarantine-roster entry on [`HealthSummary`]: the mem, the
/// typed reason code, and the full reason message (repair command
/// included — plan-01 material).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QuarantinedMemReport {
    pub mem: String,
    pub reason_code: String,
    pub reason_message: String,
}

/// One per-file load failure surfaced on the health report. `file` is
/// the path the loader reported (absolute for folder mounts after the
/// reload-normalization pass); `error` is the loader's message, which
/// names the remedy where one exists (the merge-conflict refusal names
/// `memstead conflicts resolve`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoadErrorReport {
    pub file: String,
    pub error: String,
}

/// Aggregated health report for the whole graph.
#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    pub stale_entities: Vec<StaleEntity>,
    /// Entities the day threshold would list as stale but whose anchors
    /// resolve: fresh by the anchor clock, absent from `stale_entities`,
    /// listed here so the reading names the clock that overruled the
    /// threshold. Empty whenever no anchor spoke.
    pub anchor_fresh: Vec<StaleEntity>,
    pub missing_fields: Vec<HealthReport>,
    pub orphan_count: usize,
    pub stub_count: usize,
    /// Typed non-fatal issues visible to every caller of `Engine::health()`.
    /// Populated in two layers: `Engine.load_warnings` contributes drift
    /// warnings surfaced during mem load / reload / attach
    /// (`SuspiciousNestedPrefix`, future load-time checks); the MCP
    /// handler additionally appends request-scoped warnings (unknown
    /// `include` keys, clamped `limit`) on top of whatever the engine
    /// merged. Empty on the happy path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
    /// Quarantine roster: mems that failed their mem-level boot step
    /// and serve nothing until repaired + reloaded. Always present in
    /// `Engine::health()` output when non-empty — a boot-honesty fact,
    /// never behind an include gate. Empty (and omitted from the wire)
    /// on a healthy workspace, keeping default output byte-unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quarantined: Vec<QuarantinedMemReport>,
    /// Per-file load failures: entity files the loader refused (git
    /// merge-conflict markers, unreadable bytes, parser failures). The
    /// same boot-honesty class as `quarantined` — always present when
    /// non-empty, never behind an include gate — because each entry's
    /// message names the remedy (e.g. the conflict refusal names
    /// `memstead conflicts resolve`), and a remedy no surface renders
    /// is a capability nobody finds at the moment it is needed. Empty
    /// (and omitted from the wire) on a clean workspace.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub load_errors: Vec<LoadErrorReport>,
    /// Workspace-level boot diagnosis from a diagnostic-shell engine
    /// (`{code, message}`): why the real workspace could not boot at
    /// all. Absent on every ordinarily booted engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_diagnosis: Option<serde_json::Value>,
    /// Real-entity count per leaf-declared type (`<schema_ref>:<type>`
    /// keys) — the population the orphan axis exempts because those
    /// types are terminal by construction.
    /// Visible, never vanished. Empty (and omitted from the wire) for
    /// schemas that declare nothing, keeping default output
    /// byte-unchanged.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub leaf_entities_by_type: std::collections::BTreeMap<String, usize>,
    /// Dangling references: the three conditions [`DanglingLink`] carries,
    /// which are NOT all "a body wiki-link to a missing file" — that is one
    /// of them. See [`DanglingLinkKind`] for the other two (a body link to
    /// a written entity the referrer does not relate to, and a
    /// relationships row naming an absent entity) and their repairs.
    /// Populated only when the caller
    /// opts in via `include=["dangling_links"]`; `None` otherwise, so
    /// absence-of-key means "not requested" and presence-of-empty-array
    /// means "requested, zero findings". Scan is handler-driven (same
    /// pattern as `warnings` above), so non-MCP callers of
    /// `Engine::health()` always see `None` unless they invoke
    /// [`health::collect_dangling_links`] directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangling_links: Option<Vec<DanglingLink>>,
    /// Integrity findings (`{ id, axis, code, detail }`) over the
    /// conformance axis — and, under `include=["integrity"]`, the
    /// consistency axis too. Populated only when the caller opts in
    /// via `include=["conformance"]` / `include=["integrity"]`;
    /// `None` otherwise (same handler-driven pattern as
    /// `dangling_links`: absence means "not requested", an empty
    /// array means "requested, fully integral").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<Vec<integrity::IntegrityFinding>>,
    /// Tag distribution (count per distinct tag, case-sensitive) over non-stub
    /// entities. Populated only when the caller opts in via `include=["tags"]`.
    /// Case-variant drift is surfaced via the sibling field [`tag_distribution_folded`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_distribution: Option<Vec<TagDistribution>>,
    /// Case-drift audit sidecar: entries where two or more casings of the same
    /// canonical tag (lowercase) both appear in authored tags. Only entries with
    /// `variants.len() > 1` are returned — the default read of `tag_distribution`
    /// stays untouched. Populated alongside `tag_distribution`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_distribution_folded: Option<Vec<FoldedTag>>,
    /// Count of non-stub entities whose `tags` metadata is missing, empty,
    /// or resolves to zero effective tags after splitting on `,` and trimming.
    /// Populated alongside `tag_distribution` when `include=["tags"]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub untagged_entities: Option<UntaggedStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleEntity {
    pub id: EntityId,
    pub title: String,
    pub days_since_modified: u64,
    /// The anchor state that produced this row when the anchor clock
    /// spoke (`drifted` or `recheck`, or `resolves` on a fresh-by-anchor
    /// row); `None` means the day threshold produced it. The two clocks
    /// never both speak for one entity: an entity with an adjudicated
    /// hash-bearing anchor reads by its anchors, the rest by the day
    /// threshold.
    pub anchor_state: Option<String>,
}

/// One entry in the tag distribution surface: an authored tag string, the
/// number of non-stub entities carrying it, and the per-entity-type breakdown
/// of those hits. Comparison is case-sensitive — `decision` and `Decision`
/// count as distinct entries here (see `tag_distribution_folded` for the
/// drift-aware sidecar).
#[derive(Debug, Clone, Serialize)]
pub struct TagDistribution {
    pub tag: String,
    pub count: usize,
    pub by_entity_type: HashMap<String, usize>,
}

/// Case-drift audit entry. Surfaces when two or more casings of the same
/// canonical (lowercased) tag appear in the authored graph — the agent-hostile
/// bug where `decision` and `Decision` look like two healthy low-count tags
/// in the case-sensitive primary surface.
#[derive(Debug, Clone, Serialize)]
pub struct FoldedTag {
    /// Lowercase form — the canonical key.
    pub canonical: String,
    /// Sum of counts across every casing variant.
    pub total: usize,
    /// Authored casings (as-written), each with its individual count.
    /// Sorted by `count` descending; ties broken by `tag` ascending.
    pub variants: Vec<TagVariant>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagVariant {
    pub tag: String,
    pub count: usize,
}

/// Aggregate count of non-stub entities with zero effective tags, broken
/// down by `entity_type`. "Untagged" collapses three states: missing `tags`
/// metadata, empty string value, and comma-only value (e.g. `","`).
#[derive(Debug, Clone, Serialize)]
pub struct UntaggedStats {
    pub total: usize,
    pub by_entity_type: HashMap<String, usize>,
}

/// One dangling-reference finding surfaced by
/// `memstead_health include=["dangling_links"]` and projected onto the
/// consistency axis by `include=["integrity"]`.
///
/// Three conditions reach this type, and [`kind`](Self::kind) says which:
/// a body wiki-link whose target has no markdown file (the post-delete /
/// renamed-without-rewrite / typo signal), a body wiki-link to a fully
/// written entity that the referrer does not relate to, and a relationships
/// row naming an entity absent from the store. Their repairs differ, so the
/// codes differ; see [`DanglingLinkKind`].
///
/// The prose this replaces described only the first condition, which is how
/// the fusion survived: the type read as if it had one subject while
/// producing three.
#[derive(Debug, Clone, Serialize)]
pub struct DanglingLink {
    /// Which of the three conditions this is, and therefore which repair
    /// applies. Carried from the one producer, never re-derived: the split
    /// happens where the conditions are distinguished (04/06).
    pub kind: DanglingLinkKind,
    pub from: EntityId,
    /// Canonical ID the wiki-link resolves to.
    ///
    /// NOT necessarily a stub: it is a stub or absent for
    /// [`DanglingLinkKind::LinkTargetMissing`] and
    /// [`DanglingLinkKind::RelationTargetMissing`], and a real, non-stub
    /// entity for [`DanglingLinkKind::LinkNotRelated`], where the entity is
    /// fine and the relationship row is what is missing. The old wording said
    /// "stub-typed in the store", which was true of one of the three
    /// conditions this type carried.
    pub target_id: EntityId,
    /// Resolved mem-relative path segment of the target ID (e.g. `gone`
    /// for `specs--gone`). This is the normalised form the engine records —
    /// not the literal `[[…]]` characters as authored. Widening `WikiLink`
    /// to preserve the authored form is a future-work item if agents need
    /// grep-to-source precision.
    pub target_path: String,
    /// Section key the body wiki-link appears in (e.g. `"purpose"`).
    ///
    /// `None` for [`DanglingLinkKind::RelationTargetMissing`], whose source is
    /// the auto-managed relationships block rather than a body section. That
    /// absence used to be the ONLY way to tell that condition apart, which is
    /// why `kind` exists: a reader should not have to inspect a payload for
    /// nulls to learn which repair applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

/// The three conditions the one dangling-link collector distinguishes.
///
/// They were emitted under a single `DANGLING_LINK` code through an identical
/// payload, so a reader could not tell which of three repairs applied, and
/// neither could the surfaces rendering it. Two of the three were not
/// discriminable at all. The project's error discipline is that a typed code
/// names one condition, so each gets its own (04/06).
///
/// The serialised value IS the code, so a payload's `kind`, a finding's
/// `code` and a rendered line all read the same string. A kebab-case serde
/// name would be a second spelling of one condition, which is the shape of
/// the defect this plan removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DanglingLinkKind {
    /// A body wiki-link whose target is absent from the store or present only
    /// as a stub. Repair: create the target entity.
    #[serde(rename = "DANGLING_LINK_TARGET_MISSING")]
    LinkTargetMissing,
    /// A body wiki-link to an existing, non-stub entity that the referrer's
    /// relationships list does not name. The entity is fine; the relationship
    /// row is missing. Repair: `memstead_relate` the two.
    #[serde(rename = "DANGLING_LINK_NOT_RELATED")]
    LinkNotRelated,
    /// A relationships row whose target is entirely absent, neither stub nor
    /// real. Repair: remove the row, or create the target.
    ///
    /// A stub target here is a legitimate forward reference (the alias
    /// machinery auto-stubs absent targets by design) and is deliberately not
    /// flagged.
    #[serde(rename = "DANGLING_RELATION_TARGET_MISSING")]
    RelationTargetMissing,
}

impl DanglingLinkKind {
    /// The stable wire code. One code, one condition, one repair.
    pub fn code(&self) -> &'static str {
        match self {
            DanglingLinkKind::LinkTargetMissing => "DANGLING_LINK_TARGET_MISSING",
            DanglingLinkKind::LinkNotRelated => "DANGLING_LINK_NOT_RELATED",
            DanglingLinkKind::RelationTargetMissing => "DANGLING_RELATION_TARGET_MISSING",
        }
    }

    /// Every code this family can emit. The strict counter and any other
    /// consumer filtering on the literal string reads THIS rather than
    /// keeping its own copy.
    ///
    /// Kept honest by `all_codes_covers_every_variant`, whose exhaustive
    /// match stops compiling when a variant is added — without it this is
    /// just another hand-written list, and a fourth condition would fall
    /// out of the strict gate silently, which is the failure the roster
    /// exists to prevent.
    pub const ALL_CODES: &'static [&'static str] = &[
        "DANGLING_LINK_TARGET_MISSING",
        "DANGLING_LINK_NOT_RELATED",
        "DANGLING_RELATION_TARGET_MISSING",
    ];

    /// What to do about it, in one clause.
    pub fn repair(&self) -> &'static str {
        match self {
            DanglingLinkKind::LinkTargetMissing => {
                "create the target entity, or remove the wiki-link"
            }
            DanglingLinkKind::LinkNotRelated => {
                "relate the two entities, so the body link is backed by a relationship row"
            }
            DanglingLinkKind::RelationTargetMissing => {
                "remove the relationship row, or create the target entity"
            }
        }
    }
}
