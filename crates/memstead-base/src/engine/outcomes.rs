//! Argument/outcome shapes for the mutation entrypoints
//! (`Engine::create_entity`, `update_entity`, `delete_entity`,
//! `relate_entity`, `rename_entity`). The MCP wire envelopes and CLI
//! command output formatters branch on these shapes; their field
//! layouts are part of the engine's public surface.

use indexmap::IndexMap;

use crate::entity::EntityId;
use crate::ops::{IncomingRef, ModifiedMetadata, ModifiedSections, WarningHint};

/// Arguments for [`Engine::create_entity`].
///
/// Carries the target mem (routes to the right mount), the entity
/// shape (title, type, sections, metadata), and nothing else. Caller
/// identity (actor, client, note) goes through the standalone
/// arguments so the same MCP-tool / CLI-direct shape works.
#[derive(Debug, Clone)]
pub struct CreateEntityArgs {
    pub mem: String,
    pub title: String,
    pub entity_type: String,
    pub sections: IndexMap<String, String>,
    pub metadata: IndexMap<String, String>,
    /// Inline relationships to wire as outgoing edges from the new
    /// entity. Each entry's `to` may name an absent target — the
    /// engine auto-stubs it (mirrors full's create + stub
    /// creation on relate). Open-mode admissions surface as
    /// [`WarningHint::UndeclaredRelationshipOpen`] in the outcome's
    /// `warnings`. Empty default — callers omit when no inline
    /// edges are needed.
    pub relations: Vec<crate::ops::RelateArg>,
    /// Permissive `anchors[]` provenance records to attach to the new
    /// entity — validated ([`crate::anchor::AnchorInput::validate`]) and,
    /// when non-empty, written into the mem-branch anchors sidecar in the
    /// SAME commit as the entity so the two land atomically. Empty (the
    /// default) writes no sidecar and leaves behaviour byte-identical to a
    /// pre-anchor create. A malformed element refuses the whole create with
    /// [`EngineError::InvalidAnchor`] (`INVALID_ANCHOR`) — the entity is not
    /// written. Not folded into `_hash` (sidecar lives under `.memstead/`).
    pub anchors: Vec<crate::anchor::AnchorInput>,
    /// When `true`, validate and compute the prospective hash but
    /// do not write to disk, mutate the store, create edges, or
    /// commit. Outcome carries `content_hash` = the prospective
    /// hash and `write_id` empty — wire-equivalent to full's
    /// `CreateArgs.dry_run` semantics.
    pub dry_run: bool,
}

/// Successful outcome of [`Engine::create_entity`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct CreateEntityOutcome {
    pub id: EntityId,
    /// Echoed from the request — full's `CreateResult.title` carries
    /// the same value so wire callers don't need to derive it from
    /// the id.
    pub title: String,
    /// Echoed from the request — full's `CreateResult.mem`. The
    /// `EntityId.mem()` accessor projects the same value, but
    /// surfacing it explicitly mirrors full's wire shape.
    pub mem: String,
    /// Mem-relative path of the freshly-written `.md` file.
    pub file_path: String,
    /// SHA-256 of the canonical bytes. Round-trips as
    /// `expected_hash` for the next mutation against this entity.
    /// The wire
    /// key is `_hash` to match `memstead_entity`'s read envelope and
    /// the underscore-prefix convention for engine metadata. Pre-
    /// fix mutation responses serialised this as `content_hash`,
    /// forcing agents to rename the field when piping the value
    /// into a follow-up call.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// The identity the mem's backend minted for this write — a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder or
    /// in-memory mem. An identity, never a change cursor. Wire-equivalent to
    /// full's `CreateResult.write_id`.
    pub write_id: String,
    /// ISO date string from the parsed entity's `created_date`
    /// metadata. Today's date when the schema's auto-stamp filled
    /// it in; the existing value when re-materialising a stub with
    /// `init_timestamp` semantics. Wire-equivalent to full's
    /// `CreateResult.created_date`.
    pub created_date: String,
    /// Typed Tier-2 warnings — today
    /// [`WarningHint::MissingRequiredSection`] for empty / absent
    /// required sections. Populated even when the create succeeded
    /// so callers see the same self-correction prompts the existing
    /// engines emit. Wire-equivalent to full's
    /// `CreateResult.warnings`.
    pub warnings: Vec<WarningHint>,
    /// Type-level `write_rules` keyed by `entity_type` — the
    /// MISSING_REQUIRED_SECTION / MISSING_REQUIRED_FIELD warnings
    /// reference this top-level map via their `entity_type` field
    /// rather than each carrying the (identical, type-axis) array.
    /// Empty when no such warnings fire; stable empty shape ships
    /// on the wire so consumers don't branch on field presence
    /// (F9). Sorted by key for deterministic output.
    pub type_guidance: std::collections::BTreeMap<String, Vec<String>>,
    /// Number of incoming edges adopted from a pre-existing stub
    /// at this id. `None` when no stub adoption happened (no
    /// pre-existing entity, or a real entity at the id — but that
    /// path errors with `AlreadyExists` before this field is
    /// computed). Wire-equivalent to full's
    /// `CreateResult.incoming_count`.
    pub incoming_count: Option<usize>,
    /// Incoming edges present at this id post-create — populated
    /// from `store.incoming(id)` after the parse + upsert. Empty
    /// when no pre-existing stub had referrers. Wire-equivalent to
    /// full's `CreateResult.incoming`.
    pub incoming: Vec<IncomingRef>,
    /// Batched relation declarations from the request's existing
    /// `relations[]` parameter. Mirrors
    /// [`UpdateEntityOutcome::relations_declared`] so the agent sees
    /// one wire shape across `memstead_create` and `memstead_update`. Empty
    /// `[]` when no relations were declared. `target_was_stubbed`
    /// reports the same flag the existing relate auto-stub path
    /// emits via `WarningHint::InlineWikiLinkAutoStubbed`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations_declared: Vec<RelationDeclared>,
}

/// Arguments for [`Engine::update_entity`].
#[derive(Debug, Clone)]
pub struct UpdateEntityArgs {
    pub id: EntityId,
    /// Optimistic locking. `None` skips the check.
    pub expected_hash: Option<String>,
    /// Section keys whose body should be replaced wholesale. Empty
    /// values overwrite with empty content.
    pub sections: IndexMap<String, String>,
    /// Section keys whose body should be appended to. Existing body
    /// gets a `\n` separator before the append; empty/absent body
    /// is replaced wholesale with the append value (parity with
    /// full's append-on-empty behaviour). The same key may not
    /// appear in both `sections` and `append_sections`; conflict
    /// is rejected with [`EngineError::ConflictingSectionModes`].
    pub append_sections: IndexMap<String, String>,
    /// Section keys whose body should be patched via find-and-
    /// replace. Each value is a LIST of [`crate::ops::PatchArg`]
    /// (`old`, `new`, `all`), applied in order against the section's
    /// evolving body — batched edits to one section land in one call
    /// instead of one call per patch. Errors with
    /// [`EngineError::PatchSectionEmpty`] when the section is absent
    /// and [`EngineError::PatchOldNotFound`] when an `old` doesn't
    /// appear at its turn. Mutually exclusive with the other section
    /// modes for the same key.
    pub patch_sections: IndexMap<String, Vec<crate::ops::PatchArg>>,
    /// Section keys to REMOVE from the entity — heading and body both.
    /// The close gesture for a declared-but-empty heading with nothing
    /// to receive (the shape a discovery build leaves behind), and the
    /// repair for a legacy undeclared heading. Silently no-ops on an
    /// absent key (symmetric with `metadata_unset`). Refused for a
    /// schema-REQUIRED section (`MISSING_REQUIRED_SECTION` — the right
    /// repair there is filling, not removing), for `relationships`
    /// (`SECTION_NOT_UPDATABLE`), and for a key also named in any other
    /// section mode (`CONFLICTING_SECTION_MODES`).
    pub sections_unset: Vec<String>,
    /// Metadata fields to set or replace. Values land as
    /// `MetadataValue::String` for V1.
    pub metadata: IndexMap<String, String>,
    /// Metadata field keys to unset. Silently no-ops on absent keys.
    pub metadata_unset: Vec<String>,
    /// When `true`, validate and compute the prospective hash but
    /// do not write to disk, mutate the store, or commit. Outcome
    /// carries `content_hash` = the unchanged on-disk hash (so the
    /// caller can use it as `expected_hash` on the follow-up real
    /// call) and `prospective_hash` = the hash the entity would
    /// have after the proposed write. Wire-equivalent to full's
    /// `UpdateArgs.dry_run`. Optimistic-lock check is skipped on
    /// the dry_run path so an agent can preview a change without
    /// holding a fresh hash — designated stale-hash recovery path.
    pub dry_run: bool,
    /// Atomic batched relation declarations applied before the
    /// section/metadata changes land. Each entry is validated like
    /// any individual `memstead_relate` call (schema-shape, cross-mem
    /// policy, target-id grammar), appended to the entity's
    /// `relationships` list, and — for absent Write-target peers —
    /// auto-stubbed in the target's mem. The strict
    /// wiki-link/relation validator then runs against the
    /// post-mutation state with the freshly-declared relations
    /// already in place, so a body wiki-link added in the same
    /// `memstead_update` call passes the gate without a separate
    /// `memstead_relate` round-trip. Empty default — omit when no
    /// batched declarations are needed.
    pub declare_relations: Vec<crate::ops::RelateArg>,
    /// Permissive `anchors[]` provenance records to attach to this entity
    /// — validated ([`crate::anchor::AnchorInput::validate`]) and, when
    /// non-empty, **merged** into the entity's row in the mem-branch
    /// anchors sidecar in the SAME commit as the update so entity +
    /// anchors land atomically: an incoming anchor replaces the existing
    /// anchor with the same `(artifact, grain, class)` triple and appends
    /// otherwise — writing never removes an anchor this call did not name
    /// in [`Self::anchors_unset`]. Empty (the default) merges nothing and
    /// leaves the stored set untouched. A malformed element refuses the
    /// whole update with [`EngineError::InvalidAnchor`] (`INVALID_ANCHOR`)
    /// — nothing is written. Not folded into `_hash` (sidecar lives under
    /// `.memstead/`).
    pub anchors: Vec<crate::anchor::AnchorInput>,
    /// Explicit anchor removals, applied **before** the [`Self::anchors`]
    /// merge in the same mutation (mirroring the `metadata_unset` /
    /// `relations_unset` conventions). Each selector names an `artifact`
    /// and may narrow by `grain` and/or `class`; a bare artifact removes
    /// every anchor on it. Unsetting an anchor that does not exist is a
    /// no-op, not an error — removal is idempotent. A malformed selector
    /// refuses the whole update with [`EngineError::InvalidAnchor`].
    pub anchors_unset: Vec<crate::anchor::AnchorUnsetInput>,
    /// Repair-shaped relation removals (`{ rel_type, target }`),
    /// applied atomically within this update. Accepted only when the
    /// entity currently FAILS the conformance check (against the
    /// effective schema) — a conformant entity refuses with
    /// `REPAIR_NOT_NEEDED` and stays unmodified; `memstead_relate(remove)`
    /// is the everyday detach path. Absent pairs are silent no-ops
    /// (symmetric with `metadata_unset`). The strict-write
    /// post-condition is unchanged: the post-repair entity must be
    /// integral or the whole update refuses with the relevant
    /// write-time code.
    pub relations_unset: Vec<crate::ops::RelationUnsetArg>,
}

impl UpdateEntityArgs {
    /// Whether this payload names anything that can move the entity's content
    /// hash. Anchors are deliberately absent from the list: the sidecar lives
    /// outside the hash.
    ///
    /// WHY it lives here rather than on each surface: MCP (both flavours), the
    /// CLI and the HTTP layer all gate an update on a compare-and-swap token,
    /// and on an anchors-only payload that token compares a value the write
    /// provably cannot move. Exempting the shape is right; exempting it four
    /// times, once per surface, is how surfaces come to disagree about whether
    /// a write is safe, which is the drift class this campaign closes. One
    /// predicate, one answer. The engine core does not consult it: it checks
    /// the token only when a caller supplies one, and always has.
    ///
    /// A payload naming NOTHING changes no content either, and must fall
    /// through to the empty-update refusal rather than be told it is missing a
    /// token: that refusal names the recognised keys, which is what a caller
    /// who typo'd a mutation key actually needs. A first version asked
    /// "is this anchors-only" instead, and turned every empty payload into a
    /// hash complaint; the plan's criterion 5 caught it.
    pub fn changes_content(&self) -> bool {
        !self.sections.is_empty()
            || !self.append_sections.is_empty()
            || !self.patch_sections.is_empty()
            || !self.sections_unset.is_empty()
            || !self.metadata.is_empty()
            || !self.metadata_unset.is_empty()
            || !self.declare_relations.is_empty()
            || !self.relations_unset.is_empty()
    }
}

/// Successful outcome of [`Engine::update_entity`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateEntityOutcome {
    pub id: EntityId,
    /// Title from the parsed entity after the write — wire-equivalent
    /// to full's `UpdateResult.title`. Reflects post-write state in
    /// case a future update path touches the title (today the update
    /// surface doesn't, but reading from the parsed entity rather
    /// than echoing `args` keeps the field correct as the surface
    /// evolves).
    pub title: String,
    pub file_path: String,
    /// Wire key `_hash`.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// The identity the mem's backend minted for this write — a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder or
    /// in-memory mem. An identity, never a change cursor. Wire-equivalent to
    /// full's `UpdateResult.write_id`.
    pub write_id: String,
    /// ISO date string from the parsed entity's `modified_date`
    /// metadata. Populated when the schema auto-stamps the field
    /// on update; empty when the schema doesn't declare it. Wire-
    /// equivalent to full's `UpdateResult.modified_date`.
    pub modified_date: String,
    /// Section-level mutations grouped by mode (replaced / appended /
    /// patched). Wire-equivalent to full's
    /// `UpdateResult.modified_sections`. Empty inner vecs serde-omit
    /// per `ModifiedSections`'s field attributes; the outer key is
    /// always present.
    pub modified_sections: ModifiedSections,
    /// Metadata-level mutations grouped by direction (set / unset).
    /// Wire-equivalent to full's `UpdateResult.modified_metadata`.
    /// Same empty-vec-omit convention as `modified_sections`.
    pub modified_metadata: ModifiedMetadata,
    /// `Some(hash)` on the dry_run path — the hash the entity
    /// would have after the proposed write. `None` on real
    /// updates (the post-write hash is in `content_hash`).
    /// Wire-equivalent to full's `UpdateResult.prospective_hash`.
    pub prospective_hash: Option<String>,
    /// Stub entities whose last incoming edge was severed by this
    /// update — when a body wiki-link was removed, the alias-resync
    /// drops the backing pointer-rel-type edge, and if that was the
    /// stub target's last referrer the stub is GC'd here. Empty on
    /// updates that didn't orphan a stub (including section edits with
    /// no wiki-link change, dry-run, and no-op). Shares the field name
    /// and always-present shape with
    /// [`DeleteEntityOutcome::orphan_stubs_removed`] and
    /// [`RelateEntityOutcome::orphan_stubs_removed`] so MCP / CLI
    /// consumers branch uniformly across the three GC paths.
    pub orphan_stubs_removed: Vec<EntityId>,
    /// Typed non-fatal issues — empty on the unified path today
    /// (update doesn't surface InlineWikiLinkAutoStubbed or
    /// MissingRequiredOutgoing yet). Wire-equivalent to full's
    /// `UpdateResult.warnings`; the field shape parity matters for
    /// the upcoming handler migration so callers see the same
    /// `warnings: []` envelope position across flavours.
    pub warnings: Vec<WarningHint>,
    /// Batched relation declarations applied by this call (per the
    /// optional `declare_relations` request param). Empty `[]`
    /// when no batched declarations were requested; populated with
    /// one entry per declared relation otherwise. `target_was_stubbed`
    /// flags which targets were absent at call time and got
    /// auto-stubbed; agents use this to skip a follow-up
    /// `memstead_entity` round-trip on the stubbed target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations_declared: Vec<RelationDeclared>,
    /// Whether the update's anchors changed the sidecar: absent when the
    /// update carried no anchors or unsets, `true` when a row was added,
    /// replaced or removed, `false` when every supplied row restated
    /// what was stored (nothing written, the sidecar bytes untouched).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchors_changed: Option<bool>,
}

/// One batched relation declaration applied by a mutation call.
/// Echoed in [`UpdateEntityOutcome::relations_declared`] and
/// [`CreateEntityOutcome::relations_declared`] so agents see, in the
/// same response, what landed and which targets had to be stubbed.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct RelationDeclared {
    pub rel_type: String,
    pub target: EntityId,
    /// `true` when the target was absent at call time and the
    /// engine materialised a stub for it (subject to the same
    /// rules as `memstead_relate`'s auto-stub mechanic).
    pub target_was_stubbed: bool,
}

/// Arguments for [`Engine::delete_entity`].
///
/// No `force` flag — delete is binary. The engine refuses on any
/// Write-Mem incoming reference (typed `HAS_INCOMING_REFS`); when
/// only ReadOnly-mount referrers remain, the entity is demoted to a
/// stub in-memory and the delete proceeds, surfaced via a typed
/// `RESIDUAL_STUB_FOR_READONLY_REFERRERS` warning on the outcome.
#[derive(Debug, Clone)]
pub struct DeleteEntityArgs {
    pub id: EntityId,
    /// Optimistic locking. `None` skips the check.
    pub expected_hash: Option<String>,
}

/// Successful outcome of [`Engine::delete_entity`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeleteEntityOutcome {
    pub id: EntityId,
    pub file_path: String,
    /// Ids of entities that referenced the deleted entity (only
    /// populated on the residual-stub-demotion path — the surviving
    /// ReadOnly-mount referrers are listed here for diagnostic
    /// continuity with the warning payload).
    pub removed_incoming: Vec<String>,
    /// Total edges removed across incoming + outgoing — full
    /// `DeleteResult.relations_removed`. Counted from the store
    /// pre-delete; both directions sum into one number for callers
    /// that need a single "how much did this delete cascade" signal.
    pub relations_removed: usize,
    /// The identity the mem's backend minted for this write — a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder or
    /// in-memory mem. An identity, never a change cursor. Wire-equivalent to
    /// full's `DeleteResult.write_id`.
    pub write_id: String,
    /// Stub entities that became orphaned by this delete (their last
    /// incoming edge disappeared with this entity) and were
    /// garbage-collected. Empty on deletes that didn't sever a
    /// stub's last referrer. Wire-equivalent to full's
    /// `DeleteResult.orphan_stubs_removed`.
    pub orphan_stubs_removed: Vec<EntityId>,
    /// Typed non-fatal issues — populated on the residual-stub
    /// demotion path with a `RESIDUAL_STUB_FOR_READONLY_REFERRERS`
    /// warning naming the surviving ReadOnly-mount referrers. Empty
    /// on the clean-removal path.
    pub warnings: Vec<WarningHint>,
}

/// Arguments for [`Engine::relate_entity`].
#[derive(Debug, Clone)]
pub struct RelateEntityArgs {
    pub source: EntityId,
    /// Optimistic locking on the source. `None` skips the check.
    pub expected_hash: Option<String>,
    pub rel_type: String,
    pub target: EntityId,
    /// `false` (default) appends. `true` removes the matching pair.
    pub remove: bool,
    /// Optional per-edge description applied on the add path.
    /// Validated against the rel-type's `per_edge_description`
    /// posture at call time — `forbidden` rejects `Some`; `required`
    /// rejects `None`. Empty / whitespace-only strings normalise to
    /// `None` before validation. Ignored on the remove path (`None`
    /// keeps the existing behaviour intact).
    pub description: Option<String>,
    /// Rehearsal mode (agent-trust plan 07): run the FULL validation
    /// stage — identical refusals, identical warnings (including the
    /// would-be `AUTO_STUB_CREATED`) — then stop before any write.
    /// The response carries the marker form: empty `write_id` with
    /// `_hash` set to the PROSPECTIVE post-write hash. Nothing is
    /// staged, committed, or stubbed.
    pub dry_run: bool,
}

/// What a relate call did to the source's relationships.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelateAction {
    Added,
    Removed,
    NoOpAlreadyPresent,
    NoOpAbsent,
}

/// Successful outcome of [`Engine::relate_entity`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct RelateEntityOutcome {
    pub from: EntityId,
    pub to: EntityId,
    pub rel_type: String,
    pub action: RelateAction,
    /// Source entity's content hash after the call. Unchanged on
    /// no-op paths so callers can chain follow-ups without
    /// refetching. Wire key `_hash`.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// The identity the mem's backend minted for this write — a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder or
    /// in-memory mem. An identity, never a change cursor. Empty on the no-op
    /// paths ([`RelateAction::NoOpAlreadyPresent`],
    /// [`RelateAction::NoOpAbsent`]) — those branches skip the disk
    /// write so no commit happens. Wire-equivalent to the full
    /// `RelateResult.write_id`.
    pub write_id: String,
    /// Edge provenance label — always `"explicit"` for relate-call
    /// outcomes. Wire-equivalent to full's `RelateResult.source` field;
    /// reserved for future inline-link-derived edge surfacing.
    pub source: String,
    /// Typed non-fatal issues — open-mode schema admissions
    /// ([`WarningHint::UndeclaredRelationshipOpen`]), duplicate-add
    /// no-ops ([`WarningHint::DuplicateRelationship`]),
    /// remove-nonexistent no-ops ([`WarningHint::NoSuchRelationship`]),
    /// and auto-stubbed targets
    /// ([`WarningHint::AutoStubCreated`]). Pre-Item-03 the auto-stub
    /// case rode through a deprecated top-level
    /// `stub_warning: Option<String>` field that didn't follow the
    /// `warnings[]` shape — agents iterating diagnostics silently
    /// skipped it; the field has been retired in favour of the
    /// uniform warning vocabulary. Empty on the strict-add and
    /// strict-remove happy paths.
    pub warnings: Vec<WarningHint>,
    /// Stub entities whose last incoming edge was severed by a
    /// `remove=true` call and that were garbage-collected as orphans.
    /// Empty on the add path and on remove paths that didn't strip
    /// the last referrer. Wire-equivalent to
    /// [`DeleteEntityOutcome::orphan_stubs_removed`]; both surfaces
    /// share the same field name so MCP / CLI consumers can branch
    /// uniformly (F7).
    pub orphan_stubs_removed: Vec<EntityId>,
}

/// Arguments for [`Engine::rename_entity`].
#[derive(Debug, Clone)]
pub struct RenameEntityArgs {
    pub id: EntityId,
    /// Optimistic locking. `None` skips the check.
    pub expected_hash: Option<String>,
    pub new_title: String,
}

/// Successful outcome of [`Engine::rename_entity`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenameEntityOutcome {
    pub old_id: EntityId,
    pub new_id: EntityId,
    /// Mem-relative path of the renamed entity before the rewrite.
    /// Wire-equivalent to full's `RenameResult.old_path`.
    pub old_path: String,
    /// Mem-relative path of the renamed entity after the rewrite.
    /// Wire-equivalent to full's `RenameResult.new_path`.
    pub new_path: String,
    /// Wire key `_hash`.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// The identity the mem's backend minted for this write — a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder or
    /// in-memory mem. An identity, never a change cursor. Empty on the
    /// slug-noop short-circuit (no disk write happened).
    /// Wire-equivalent to full's `RenameResult.write_id`.
    pub write_id: String,
    /// Typed non-fatal issues. The slug-noop short-circuit
    /// ([`WarningHint::TitleNormalizedToSlugNoop`]) surfaces here
    /// when a requested title normalises to the existing slug — the
    /// op stays a silent no-op on disk, but the warning tells
    /// autonomous skills not to trust `old_id == new_id` as
    /// "cosmetic rewrite landed". Empty on the real-rename happy
    /// path. Wire-equivalent to full's `RenameResult.warnings`.
    pub warnings: Vec<WarningHint>,
}

/// Arguments for [`Engine::retype_entity`].
#[derive(Debug, Clone)]
pub struct RetypeEntityArgs {
    pub id: EntityId,
    /// Optimistic locking. `None` skips the check (dry runs always skip it).
    pub expected_hash: Option<String>,
    /// The type the entity becomes; must be declared by the mem's schema.
    pub target_type: String,
    /// Section keys to rename on the way: `old key → new key`. A key not
    /// mapped keeps its name and must be declared by the target type (or
    /// the retype refuses `UNKNOWN_SECTION` with a proposed map).
    pub section_map: IndexMap<String, String>,
    /// Metadata keys the caller explicitly lets go — the fields the
    /// source type declared and the target does not (a spec's `level`
    /// on the way to a memo). Never inferred: an undeclared field that is
    /// not listed here refuses `UNKNOWN_METADATA_FIELD`, because dropping
    /// data unannounced is what the write gates exist to prevent. A key
    /// the entity does not carry is a silent no-op.
    pub drop_metadata: Vec<String>,
    /// Validate everything and compute the prospective hash without
    /// writing, committing, or touching the store.
    pub dry_run: bool,
}

/// Successful outcome of [`Engine::retype_entity`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct RetypeEntityOutcome {
    pub id: EntityId,
    /// Unchanged by the retype — the id and path are the point.
    pub file_path: String,
    pub old_type: String,
    pub new_type: String,
    /// Wire key `_hash`: the content hash after the write (the next
    /// `expected_hash`); on a dry run the UNCHANGED current hash.
    #[serde(rename = "_hash")]
    pub content_hash: String,
    /// The hash the entity would carry after the write — dry runs only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prospective_hash: Option<String>,
    /// The identity the mem's backend minted for this write — a commit
    /// SHA on a git-branch mem, an opaque synthetic token on a folder or
    /// in-memory mem. An identity, never a change cursor. Empty on a dry
    /// run (no disk write happened).
    pub write_id: String,
    /// `(old key, new key)` pairs the `section_map` applied.
    pub sections_renamed: Vec<(String, String)>,
    /// Every edge examined against the target type's pins: outgoing,
    /// incoming (loaded), and incoming from deferred mems.
    pub edges_rechecked: usize,
    /// Always true: the content hash moved, so every check record and
    /// derivation baseline keyed to the previous hash is stale.
    pub checks_stale: bool,
    /// The sentence that says so, for the surface to print.
    pub staleness_note: String,
    pub warnings: Vec<WarningHint>,
}

/// Which side of an edge the retyped entity is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RetypeEdgeDirection {
    Outgoing,
    Incoming,
}

/// One edge the target type's pins refuse.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct RetypeEdge {
    pub direction: RetypeEdgeDirection,
    pub from: String,
    pub to: String,
    pub rel_type: String,
    pub cross_mem: bool,
    /// The shape validator's own recovery detail (allowed source and
    /// target types, suggestion).
    pub detail: serde_json::Value,
}

/// One reason a retype is refused. Every problem found is reported
/// together in [`EngineError::RetypeRefused`]; each carries the wire code
/// the same condition has on the create/update/relate surfaces.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RetypeProblem {
    /// A (mapped) section key the target type does not declare.
    UnknownSection {
        key: String,
        declared: Vec<String>,
        suggestion: Option<String>,
    },
    /// A section the target type requires is absent or empty.
    MissingRequiredSection {
        key: String,
        heading: String,
        write_rules: Vec<String>,
    },
    /// A metadata field the target type requires is unset and has no
    /// default.
    MissingRequiredField {
        key: String,
        description: String,
        enum_values: Vec<String>,
    },
    /// A validator refusal carried verbatim: body content, an unknown
    /// metadata field, an enum or field value the target rejects.
    Validation {
        code: &'static str,
        message: String,
        details: serde_json::Value,
    },
    /// `section_map` names a key the entity does not carry.
    SectionMapSourceMissing {
        key: String,
        present: Vec<String>,
    },
    /// Two sections would land under one key.
    SectionMapCollision {
        from: String,
        to: String,
        also_from: String,
    },
    EdgeShape(RetypeEdge),
    RequiredOutgoingUnsatisfied(Vec<crate::ops::MissingRequiredOutgoingBlock>),
    ConstraintUnsatisfied(Vec<crate::ops::health::UnsatisfiedConstraint>),
}

impl RetypeProblem {
    /// The wire code this condition carries everywhere else.
    pub fn code(&self) -> &'static str {
        match self {
            RetypeProblem::UnknownSection { .. } => "UNKNOWN_SECTION",
            RetypeProblem::MissingRequiredSection { .. } => "MISSING_REQUIRED_SECTION",
            RetypeProblem::MissingRequiredField { .. } => "REQUIRED_FIELD_UNSET",
            RetypeProblem::Validation { code, .. } => code,
            RetypeProblem::SectionMapSourceMissing { .. } => "SECTION_MAP_SOURCE_MISSING",
            RetypeProblem::SectionMapCollision { .. } => "SECTION_MAP_COLLISION",
            RetypeProblem::EdgeShape(_) => "INVALID_REL_SHAPE",
            RetypeProblem::RequiredOutgoingUnsatisfied(_) => "MISSING_REQUIRED_OUTGOING",
            RetypeProblem::ConstraintUnsatisfied(_) => "CONSTRAINT_UNSATISFIED",
        }
    }

    /// One line a human or agent can act on.
    pub fn message(&self) -> String {
        match self {
            RetypeProblem::UnknownSection {
                key,
                declared,
                suggestion,
            } => format!(
                "section `{key}` is not declared by the target type (declared: {}){}",
                declared.join(", "),
                suggestion
                    .as_deref()
                    .map(|s| format!("; map it with section_map {key}={s}"))
                    .unwrap_or_default()
            ),
            RetypeProblem::MissingRequiredSection { key, heading, .. } => {
                format!("required section `{key}` ({heading}) is missing or empty")
            }
            RetypeProblem::MissingRequiredField { key, .. } => {
                format!("required metadata field `{key}` is unset and has no default")
            }
            RetypeProblem::Validation { message, .. } => message.clone(),
            RetypeProblem::SectionMapSourceMissing { key, present } => format!(
                "section_map names `{key}`, which the entity does not carry (present: {})",
                present.join(", ")
            ),
            RetypeProblem::SectionMapCollision {
                from,
                to,
                also_from,
            } => {
                format!("section_map sends both `{also_from}` and `{from}` to `{to}`")
            }
            RetypeProblem::EdgeShape(e) => format!(
                "{} edge {} --{}--> {} is outside the target type's pins{}",
                match e.direction {
                    RetypeEdgeDirection::Outgoing => "outgoing",
                    RetypeEdgeDirection::Incoming => "incoming",
                },
                e.from,
                e.rel_type,
                e.to,
                if e.cross_mem { " (cross-mem)" } else { "" }
            ),
            RetypeProblem::RequiredOutgoingUnsatisfied(blocks) => format!(
                "{} block-tier required_outgoing block(s) of the target type unsatisfied",
                blocks.len()
            ),
            RetypeProblem::ConstraintUnsatisfied(v) => {
                format!(
                    "{} block-tier constraint(s) of the target type violated",
                    v.len()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_types_serialize_to_json() {
        // Lock the Serialize derives: every
        // outcome type round-trips through `serde_json::to_string`
        // without panicking. The wire shape's specific field names
        // are exercised end-to-end via the MCP handlers; this test
        // is the structural lock.
        let create = CreateEntityOutcome {
            id: EntityId("v--e".to_string()),
            title: "t".to_string(),
            mem: "v".to_string(),
            file_path: "v/e.md".to_string(),
            content_hash: "h".to_string(),
            write_id: "sha".to_string(),
            created_date: "2026-05-11".to_string(),
            warnings: Vec::new(),
            type_guidance: std::collections::BTreeMap::new(),
            incoming_count: None,
            incoming: Vec::new(),
            relations_declared: Vec::new(),
        };
        assert!(serde_json::to_string(&create).is_ok());

        let update = UpdateEntityOutcome {
            id: EntityId("v--e".to_string()),
            title: "t".to_string(),
            file_path: "v/e.md".to_string(),
            content_hash: "h".to_string(),
            write_id: "sha".to_string(),
            modified_date: "2026-05-11".to_string(),
            modified_sections: ModifiedSections::default(),
            modified_metadata: ModifiedMetadata::default(),
            prospective_hash: None,
            orphan_stubs_removed: Vec::new(),
            warnings: Vec::new(),
            relations_declared: Vec::new(),
            anchors_changed: None,
        };
        assert!(serde_json::to_string(&update).is_ok());

        let delete = DeleteEntityOutcome {
            id: EntityId("v--e".to_string()),
            file_path: "v/e.md".to_string(),
            removed_incoming: Vec::new(),
            write_id: "sha".to_string(),
            relations_removed: 0,
            orphan_stubs_removed: Vec::new(),
            warnings: Vec::new(),
        };
        assert!(serde_json::to_string(&delete).is_ok());

        let relate = RelateEntityOutcome {
            from: EntityId("v--a".to_string()),
            to: EntityId("v--b".to_string()),
            rel_type: "PART_OF".to_string(),
            action: RelateAction::Added,
            content_hash: "h".to_string(),
            write_id: "sha".to_string(),
            source: "explicit".to_string(),
            warnings: Vec::new(),
            orphan_stubs_removed: Vec::new(),
        };
        assert!(serde_json::to_string(&relate).is_ok());

        let rename = RenameEntityOutcome {
            old_id: EntityId("v--a".to_string()),
            new_id: EntityId("v--b".to_string()),
            old_path: "v/a.md".to_string(),
            new_path: "v/b.md".to_string(),
            content_hash: "h".to_string(),
            write_id: "sha".to_string(),
            warnings: Vec::new(),
        };
        let rename_json = serde_json::to_string(&rename).unwrap();
        // Field names match full's RenameResult wire shape directly.
        assert!(
            rename_json.contains("\"old_path\""),
            "RenameEntityOutcome must serialize old_path: {rename_json}",
        );
        assert!(
            rename_json.contains("\"new_path\""),
            "RenameEntityOutcome must serialize new_path: {rename_json}",
        );
    }
}

/// Outcome discriminator for [`Engine::set_mem_schema`]. The agent
/// branches on this — never on which response fields are populated
/// (stable additive shape, no response-shape polymorphism).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetSchemaResult {
    /// Requested schema == current pin; no state change.
    Noop,
    /// Mem was (or became) integral against the target — the pin
    /// now IS the target and any migration state is cleared.
    Switched,
    /// Mem was not integral against the target; dual-pin state
    /// entered, `findings` carries the non-integral entities.
    MigrationStarted,
    /// Re-issued with the same in-flight target while still not
    /// integral; `findings` carries the *remaining* non-integral
    /// entities.
    MigrationPending,
}

/// Stable response shape of [`Engine::set_mem_schema`] — all six
/// fields are always present, populated per outcome.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SetSchemaOutcome {
    pub mem: String,
    /// The settled pin after this call (`<name>@<version>`).
    pub schema_pin: String,
    /// In-flight target while a migration is in progress, else `None`.
    pub migration_target: Option<String>,
    pub outcome: SetSchemaResult,
    /// Integrity-linter findings (`{ id, axis, code, detail }`) for
    /// the entities not yet integral against the target; empty unless
    /// a migration is in progress.
    pub findings: Vec<crate::ops::integrity::IntegrityFinding>,
    /// The resolved schema the mem's mutation stamp names after this
    /// call — the marker the `ENGINE_VERSION_SKEW` hint reads. A
    /// completed switch re-stamps it with the target; a dual-pin entry
    /// leaves it where the last mutation put it, so a reader sees here
    /// which generation the marker still carries. `None` when the mem
    /// carries no stamp (config-less, or never mutated by a stamping
    /// engine).
    pub stamped_schema: Option<String>,
}
