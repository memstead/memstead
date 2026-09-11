//! Arguments and results for the entity CRUD operations, and the shared `envelope` builder both the warning wire and the MCP error wire use.

use super::*;

// ---------------------------------------------------------------------------
// CRUD types
// ---------------------------------------------------------------------------

/// Arguments for creating an entity.
#[derive(Debug, Clone)]
pub struct CreateArgs {
    pub title: String,
    pub mem: String,
    pub entity_type: String,
    /// Section contents keyed by section key: `{ "<section-key>": "..." }`.
    /// Valid keys depend on the schema (see `TypeDefinition::sections`).
    pub sections: IndexMap<String, String>,
    /// Metadata overrides: `{ "<field-key>": "value" }`.
    pub metadata: IndexMap<String, String>,
    /// Relationships to create: `[{ target: EntityId, rel_type: "USES" }]`.
    pub relations: Vec<RelateArg>,
    /// When true, validate and compute the result but do not write to
    /// disk, mutate the store, create edges, or commit. Response carries
    /// the prospective `id`, `file_path`, `content_hash`, and any
    /// `warnings` — `write_id` is empty.
    pub dry_run: bool,
}

/// Arguments for updating an entity.
#[derive(Debug, Clone)]
pub struct UpdateArgs {
    pub id: EntityId,
    /// Expected content hash (optimistic locking). Required.
    pub expected_hash: String,
    /// Section fields to set: `{ "<section-key>": "new content" }`.
    pub sections: IndexMap<String, String>,
    /// Section fields to append to: `{ "<section-key>": "extra content" }`.
    pub append_sections: IndexMap<String, String>,
    /// Section fields to patch: `{ "<section-key>": PatchArg { old, new } }`.
    pub patch_sections: IndexMap<String, PatchArg>,
    /// Metadata fields to set: `{ "<field-key>": "value" }`.
    pub metadata: IndexMap<String, String>,
    /// Metadata keys to remove from the entity. Silent no-op on absent
    /// keys. Errors on read-only fields (mem, id, type) and on
    /// schema-required fields for the entity's type.
    pub metadata_unset: Vec<String>,
    /// Dry-run mode — return proposed changes without persisting.
    pub dry_run: bool,
}

/// Arguments for a patch (substring replacement).
#[derive(Debug, Clone)]
pub struct PatchArg {
    pub old: String,
    pub new: String,
    /// When `true`, replace every occurrence of `old` in the target
    /// section. Default `false` replaces only the first occurrence.
    pub all: bool,
}

/// Section-level mutations applied by a single `memstead_update` call.
/// Each vec lists the section keys that landed in that mutation mode.
/// Empty inner vecs are serde-omitted so the wire stays quiet; the
/// struct itself always serialises so the outer `modified_sections` key
/// is a stable shape regardless of what the call actually touched.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModifiedSections {
    /// Section keys whose body was replaced wholesale (`sections` input).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced: Vec<String>,
    /// Section keys whose body received an append (`append_sections`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appended: Vec<String>,
    /// Section keys whose body was patched via find-and-replace
    /// (`patch_sections`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patched: Vec<String>,
    /// Section keys removed outright — heading and body (`sections_unset`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
}

/// Metadata-level mutations applied by a single `memstead_update` call.
/// Same empty-vec-omit convention as `ModifiedSections`; auto-timestamp
/// metadata fields written by the engine are NOT surfaced here (they are
/// engine-driven, not user-driven — the caller has nothing to react to).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModifiedMetadata {
    /// Metadata keys whose value was set or replaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<String>,
    /// Metadata keys that were removed from the frontmatter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
}

/// Result of an update operation.
#[derive(Debug, Clone, Serialize)]
pub struct UpdateResult {
    pub id: EntityId,
    pub title: String,
    /// Section-level mutations grouped by mode. Replaces the former flat
    /// `modified_fields: Vec<String>` (which leaked mode as a string
    /// prefix and collided on bare keys with `modified_metadata`).
    pub modified_sections: ModifiedSections,
    /// Metadata-level mutations grouped by direction (set vs unset).
    pub modified_metadata: ModifiedMetadata,
    pub modified_date: String,
    /// On a real (non-dry-run) update: the new on-disk content hash after
    /// the write. On a dry-run: the **current** on-disk hash (unchanged) —
    /// the value an agent passes back as `expected_hash` on the follow-up
    /// real call. Pair with `prospective_hash` to predict the post-write
    /// hash without a second read. Wire key `_hash`.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// Dry-run only: the hash the entity *would* have after the proposed
    /// write. `None` on real (non-dry-run) updates. Lets agents preview a
    /// change and then call the real update with `expected_hash =
    /// content_hash` (pinning the disk state) while still knowing what the
    /// post-write hash will look like. Additive optional field — stable
    /// shape for callers that ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prospective_hash: Option<String>,
    /// The identity the mem's backend minted for this write: a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder
    /// or in-memory mem. It is an identity and NOT a change cursor —
    /// `memstead_changes_since` takes a commit SHA on a git-branch mem
    /// and an RFC3339 ledger timestamp on a folder mem, and feeding it
    /// this token refuses with `INVALID_CURSOR` (before that guard it
    /// silently replayed a folder mem's whole history).
    /// Empty for dry runs (no write happens).
    #[serde(default)]
    pub write_id: String,
    /// Typed non-fatal issues — same shape as `CreateResult::warnings`.
    /// Pre-Bug-4 this was `Vec<String>` and unused; now carries
    /// `WarningHint` so e.g. `INLINE_WIKI_LINK_AUTO_STUBBED` from update
    /// flows out via the same `{code, message, details}` envelope agents
    /// already branch on for create-time warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// Build the uniform `{ code, message, details }` envelope used on both the
/// warning wire (`WarningHint`'s custom `Serialize`) and the MCP error wire
/// (`tool_error_with_payload` payloads in `engine_err_with_suggestions`).
/// Agents and other decoders branch on `code` (UPPER_SNAKE_CASE, stable)
/// and parse `details` by `code` when they need structured fields.
pub fn envelope(
    code: &str,
    message: impl Into<String>,
    details: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "code": code,
        "message": message.into(),
        "details": details,
    })
}

/// Result of a create operation.
#[derive(Debug, Clone, Serialize)]
pub struct CreateResult {
    pub id: EntityId,
    pub title: String,
    pub mem: String,
    pub file_path: String,
    pub created_date: String,
    /// Post-write content hash under the real path; the **prospective**
    /// hash under `dry_run` — bit-identical to what a real call with the
    /// same inputs would produce. Wire key `_hash`.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// The backend's identity for this write, never a cursor — see
    /// `UpdateResult::write_id`. Empty under
    /// `dry_run`.
    #[serde(default)]
    pub write_id: String,
    /// Typed non-fatal issues — missing required sections (with writing
    /// guidance) and open-mode relationship admissions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
    /// Type-level `write_rules` keyed by `entity_type` — the
    /// MISSING_REQUIRED_SECTION / MISSING_REQUIRED_FIELD warnings on
    /// `warnings[]` reference this top-level map via their
    /// `entity_type` field rather than each carrying the (identical,
    /// type-axis) array (F9). Stable empty shape (`{}`) ships when no
    /// such warnings fire — consumers don't branch on field presence.
    #[serde(default)]
    pub type_guidance: std::collections::BTreeMap<String, Vec<String>>,
    /// Number of incoming edges adopted from a pre-existing stub at this
    /// id (real path) or that would be adopted (dry_run). `None` means
    /// no pre-existing stub / no incoming refs — field is serde-omitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incoming_count: Option<usize>,
    /// Incoming edges present at this id at create time. Real path:
    /// edges preserved during stub adoption. Dry_run: edges that would
    /// be adopted if committed. Sorted by (rel_type, from) for
    /// determinism. Empty vec is serde-omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incoming: Vec<IncomingRef>,
}

/// Serialisable projection of `store::InEdge` for `CreateResult.incoming`.
/// `source` is the lowercase `EdgeSource` variant:
/// `"explicit" | "hierarchy" | "body_link"`.
#[derive(Debug, Clone, Serialize)]
pub struct IncomingRef {
    pub from: EntityId,
    pub rel_type: String,
    pub source: String,
}

/// Project `&[store::InEdge]` into a sorted `Vec<IncomingRef>`. Ordering
/// by (rel_type, from) ascending — deterministic output despite the
/// underlying HashMap iteration order.
pub fn project_incoming(edges: &[crate::store::InEdge]) -> Vec<IncomingRef> {
    let mut out: Vec<IncomingRef> = edges
        .iter()
        .map(|e| IncomingRef {
            from: e.from.clone(),
            rel_type: e.rel_type.clone(),
            source: match e.source {
                crate::store::EdgeSource::Explicit => "explicit",
                crate::store::EdgeSource::Hierarchy => "hierarchy",
                crate::store::EdgeSource::BodyLink => "body_link",
            }
            .to_string(),
        })
        .collect();
    out.sort_by(|a, b| a.rel_type.cmp(&b.rel_type).then(a.from.0.cmp(&b.from.0)));
    out
}

/// Result of a delete operation.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteResult {
    pub id: EntityId,
    pub relations_removed: usize,
    /// The backend's identity for this write, never a cursor — see
    /// `UpdateResult::write_id`.
    #[serde(default)]
    pub write_id: String,
    /// Stub entities that became orphaned by this delete (their last
    /// incoming edge disappeared with this entity) and were garbage-
    /// collected. Empty vec is serde-omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_stubs_removed: Vec<EntityId>,
}

/// Result of a rename operation.
#[derive(Debug, Clone, Serialize)]
pub struct RenameResult {
    pub old_id: EntityId,
    pub new_id: EntityId,
    pub old_path: String,
    pub new_path: String,
    /// Content hash of the renamed entity after the write. Sources by branch:
    ///   - Real rename (slug change): post-write hash from the re-parsed
    ///     entity, including the `modified_date` bump applied by
    ///     `rename_entity` and any wiki-link rewrites in referrers.
    ///   - Slug-noop short-circuit: the unchanged on-disk hash (no write
    ///     happened).
    ///
    /// Pass this as `expected_hash` on the next hash-protected op
    /// (`memstead_update`, `memstead_rename`, `memstead_delete`) on the entity — no
    /// `memstead_entity` re-read required. Mirrors `RelateResult._hash`.
    /// Wire key `_hash`.
    #[serde(default, rename = "_hash", skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    /// The backend's identity for this write, never a cursor — see
    /// `UpdateResult::write_id`. Empty on the
    /// no-op same-title rename (no file change, no commit).
    #[serde(default)]
    pub write_id: String,
    /// Typed non-fatal issues. The slug-noop short-circuit
    /// (`TitleNormalizedToSlugNoop`) surfaces here when a requested title
    /// normalises to the existing slug — the op stays a silent no-op on
    /// disk, but the warning tells autonomous skills not to trust
    /// `old_id == new_id` as "cosmetic rewrite landed".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// Arguments for a relate/unrelate operation.
#[derive(Debug, Clone)]
pub struct RelateArg {
    /// The far end of the edge. Named `target` rather than `to`
    /// because the near end is implied by the call (the entity being
    /// created or updated) — the rule the response shapes already
    /// follow: a pair is `from`/`to`, an implied near end leaves
    /// `target`.
    pub target: EntityId,
    pub rel_type: String,
    /// Optional per-edge description text. Validated against the
    /// rel-type's `per_edge_description` posture at call time —
    /// `forbidden` rejects `Some`; `required` rejects `None`.
    /// Empty / whitespace-only strings normalise to `None` before
    /// validation.
    pub description: Option<String>,
}

/// One repair-shaped relation removal on `memstead_update` —
/// `relations_unset: [{ rel_type, target }]`. Symmetric with
/// `metadata_unset`: an absent `(rel_type, target)` pair is a silent
/// no-op. Only accepted when the target entity currently fails the
/// conformance check (`REPAIR_NOT_NEEDED` otherwise) — the everyday
/// detach path stays `memstead_relate(remove)`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RelationUnsetArg {
    pub rel_type: String,
    pub target: EntityId,
}

/// Result of a relate operation.
#[derive(Debug, Clone, Serialize)]
pub struct RelateResult {
    pub from: EntityId,
    pub to: EntityId,
    pub rel_type: String,
    pub source: String,
    /// Content hash of the source entity after the relate. On successful
    /// add/remove, reflects the re-rendered file (Relationships section
    /// updated); on duplicate-add and remove-nonexistent no-ops, reflects
    /// the unchanged file. Pass this as `expected_hash` on the next
    /// hash-protected op (`memstead_update`, `memstead_rename`, `memstead_delete`) on
    /// the source — no `memstead_entity` re-read required. Wire key `_hash`.
    #[serde(default, rename = "_hash", skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    /// The backend's identity for this write, never a cursor — see
    /// `UpdateResult::write_id`.
    #[serde(default)]
    pub write_id: String,
    /// Typed non-fatal issues — open-mode schema admissions, duplicate-add
    /// no-ops (`DuplicateRelationship`), remove-nonexistent no-ops
    /// (`NoSuchRelationship`). Previously silent edge cases now surface here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
    /// True if the op wrote to disk (real add or real remove). False on
    /// duplicate-add and remove-nonexistent-edge. Internal signal — the
    /// wrapper gates reindex + vcs_commit on this; the MCP wire relies on
    /// `write_id.is_empty()` as the external no-op indicator.
    #[serde(skip)]
    pub disk_changed: bool,
    /// Stub entities that became orphaned by an edge removal (their last
    /// incoming edge was this one) and were garbage-collected. Only
    /// populated on `remove: true` calls where the edge actually existed;
    /// empty on add paths and no-op removes. Empty vec is serde-omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_stubs_removed: Vec<EntityId>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// Result of an **atomic** batch update — all-or-nothing.
///
/// A batch either applies in full as a single commit (`applied: true`)
/// or, if any item fails (validation, hash mismatch, missing entity),
/// applies *nothing* and refuses (`applied: false`) with the offending
/// item named. There is no partial-application middle state: a refused
/// batch leaves the on-disk mem and the in-memory store byte-identical
/// to the pre-call state.
#[derive(Debug, Clone, Serialize)]
pub struct BatchResult {
    /// Batch-level warnings. Today this carries `CONFIG_WRITE_INTERVENED`
    /// when the mutation version stamp merged over another writer's config
    /// change: the batch is the operation, so the batch
    /// result is where its report belongs. Empty on the ordinary path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
    /// `true` when every item applied (one commit); `false` when the
    /// batch was refused (a single item failed → nothing committed).
    pub applied: bool,
    /// One entry per submitted item, in submission order. On an applied
    /// batch every entry's `action` is `"updated"` (a real write) or
    /// `"noop"` (content unchanged). On a refused batch the failing
    /// item's `action` is `"error"` with a populated `error` envelope,
    /// and every other item's `action` is `"not_applied"`.
    pub results: Vec<BatchEntry>,
    /// Count of applied items when `applied`; `0` when refused.
    pub succeeded: usize,
    /// Number of FAILING entries whose error envelopes were suppressed
    /// beyond the reporting cap (bounded reporting for very large
    /// failing batches — the entries still carry `action: "error"`,
    /// only the detailed envelope is omitted). `0` when every failure
    /// is fully reported.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub errors_suppressed: usize,
    /// Count of failed items when refused (≥1); `0` when applied.
    pub failed: usize,
    /// Ids of stub entities GC'd because a removed edge in this batch
    /// was their last incoming reference — the batch sibling of the
    /// single relate response's `orphan_stubs_removed`. Empty (and
    /// serde-omitted) for batch-create / batch-update and for batches
    /// that removed nothing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_stubs_removed: Vec<EntityId>,
    /// The backend's identity for the batch's write — a commit SHA on
    /// a git-branch mem, a synthetic token on a folder mem, and never a
    /// change cursor. Present when the batch applied and produced at
    /// least one write. Empty when the batch was
    /// refused, when it was empty, or when every item was a no-op (no
    /// commit happens). For a batch spanning multiple mems this names
    /// the last mem committed; single-mem batches (the common case)
    /// name their one commit.
    #[serde(default)]
    pub write_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchEntry {
    pub id: EntityId,
    pub action: String,
    /// Structured error envelope when this entry failed. Mirrors the
    /// `{code, message, details}` shape single-update errors carry on
    /// the wire so a mixed-success batch is structurally uniform —
    /// consumers branch on `code` rather than prose-parsing a string.
    /// Empty (`None`) for successful entries.
    pub error: Option<BatchError>,
}

/// Per-item error envelope on a batch result. The shape matches the
/// MCP wire envelope for single-entry failures: `code` is the stable
/// `UPPER_SNAKE_CASE` token from [`crate::EngineError::code()`];
/// `details` carries the variant-specific recovery payload (e.g.
/// declared list, allowed enum values, hash-mismatch current) when
/// available, or an empty object for variants without a structured
/// payload.
#[derive(Debug, Clone, Serialize)]
pub struct BatchError {
    pub code: String,
    pub message: String,
    pub details: serde_json::Value,
}
