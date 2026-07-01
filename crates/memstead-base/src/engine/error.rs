//! Engine error envelopes.
//!
//! `EngineError` lifts the typed payloads every consumer pattern-matches
//! on (`BackendError` via `#[from]`, `ValidationError` from the runtime
//! validator, `SlugError` from the slug helper, `ParseError` from the
//! markdown parser). `BootError` is the smaller envelope produced by
//! `Engine::from_workspace_root` and its pro counterpart — the failure
//! modes specific to layout detection, workspace-store load, per-mount
//! backend instantiation, and engine construction.

use std::fmt;
use std::path::PathBuf;

use crate::backend::BackendError;
use crate::entity::EntityId;
use crate::entity::id::SlugError;
use crate::entity::parser::ParseError;
use crate::runtime_validator::{MissingRequiredField, ValidationError};

/// Maximum items rendered inline before truncation kicks in. Picked to
/// keep the typical fanout (1–25 items) on one terminal line while
/// still bounding pathological cases (200+ referrers on a hub entity)
/// to a constant prefix plus a count.
pub const INLINE_LIST_CAP: usize = 3;

/// One blocked-direction summary entry for
/// [`EngineError::RenameBlockedByCrossMemPolicy`]. Pairs the
/// referrer's mem with the renaming entity's mem (the edge's
/// actual `referrer → renamed` direction post-rewrite) and the count
/// of distinct referrers in that mem that would emit the blocked
/// rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedReferrer {
    /// Referrer's mem — `from_mem` in the propagated edge's
    /// actual direction.
    pub from_mem: String,
    /// Renaming entity's mem — `to_mem` in the propagated edge's
    /// actual direction. Always the same value across every
    /// `blocked_referrers` entry of a single rename refusal.
    pub to_mem: String,
    /// Distinct referrers in `from_mem` that would emit the
    /// blocked rewrite.
    pub count: usize,
}

impl fmt::Display for BlockedReferrer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} → {} ({} referrer{})",
            self.from_mem,
            self.to_mem,
            self.count,
            if self.count == 1 { "" } else { "s" }
        )
    }
}

fn format_blocked_referrers(items: &[BlockedReferrer]) -> String {
    format_inline_list_overflow(items, "blocked_referrers")
}

/// Render a structured-list payload onto the text-mirror message. The
/// first [`INLINE_LIST_CAP`] items appear inline, comma-separated; when
/// the list is longer, the suffix " +N more — see details.<field>"
/// points the agent at the structured channel's typed list under
/// `field`. Empty input renders as an empty string. The function is
/// generic over any [`fmt::Display`] item — wrap structs in a small
/// `Display` newtype if their default rendering is too verbose for the
/// text channel.
pub fn format_inline_list_overflow<T: fmt::Display>(items: &[T], field: &str) -> String {
    if items.is_empty() {
        return String::new();
    }
    let head: Vec<String> = items
        .iter()
        .take(INLINE_LIST_CAP)
        .map(|i| i.to_string())
        .collect();
    let inline = head.join(", ");
    if items.len() > INLINE_LIST_CAP {
        let extra = items.len() - INLINE_LIST_CAP;
        format!("{inline} +{extra} more — see details.{field}")
    } else {
        inline
    }
}

/// One resolution-source line on [`EngineError::SchemaNotFound`]'s
/// `details.sources` payload.
///
/// The schema registry consults sources in a fixed order — local
/// storage (the mem's own storage backend), built-in (compiled into
/// the engine binary), remote (memstead.io, reserved) — and records
/// what each held for the pinned *name* so an agent or operator can
/// tell *where* a pin failed: missing from local authoring, absent
/// from the shipped catalogue, or past the not-yet-wired remote. The
/// `local_storage`/`builtin` lines report a wrong-version partial
/// match (right name, wrong version) through `pinned_version_match`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SchemaSourceDiagnostic {
    /// Stable source label: `"local_storage"`, `"builtin"`, or
    /// `"remote"`. Agents may branch on it.
    pub source: &'static str,
    /// Versions of the pinned *name* this source held, ascending.
    /// Empty when the source carried nothing for that name — or was
    /// not enumerated (today only `remote`, see `status`).
    pub versions_found: Vec<String>,
    /// `true` when the pinned exact version is among `versions_found`.
    /// Always `false` across every source on a genuine not-found (the
    /// fixed resolution order means a match on any source would have
    /// resolved); a lone `true` here signals right-name/wrong-version.
    pub pinned_version_match: bool,
    /// Non-enumerable status for sources that do not list versions —
    /// today only `remote`, which reports `"not_configured"`. `None`
    /// for the enumerable `local_storage`/`builtin` sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
}

impl SchemaSourceDiagnostic {
    /// Build the fixed-order source diagnostics for a failed pin.
    ///
    /// `consulted` is the resolution set the call site actually
    /// searched: at boot it is the workspace-authored schemas layered
    /// over the built-ins; at the create/migration sites it is the
    /// set that path consulted (built-in alone, or workspace + built-in
    /// for the migration resolver). The `builtin` line is recomputed
    /// from the static catalogue so it is honest regardless of what the
    /// caller passed; anything in `consulted` the built-in set does not
    /// carry is attributed to `local_storage`. `remote` is always the
    /// reserved `not_configured` slot.
    pub fn for_failed_pin(
        name: &str,
        requested: &semver::Version,
        consulted: &[std::sync::Arc<memstead_schema::Schema>],
    ) -> Vec<Self> {
        use std::collections::BTreeSet;
        let builtin: BTreeSet<semver::Version> =
            memstead_schema::builtins::load_builtin_schemas()
                .map(|set| {
                    set.iter()
                        .filter(|s| s.manifest.name == name)
                        .map(|s| s.version.clone())
                        .collect()
                })
                .unwrap_or_default();
        let local: BTreeSet<semver::Version> = consulted
            .iter()
            .filter(|s| s.manifest.name == name)
            .map(|s| s.version.clone())
            .filter(|v| !builtin.contains(v))
            .collect();
        let to_strings = |set: &BTreeSet<semver::Version>| {
            set.iter().map(|v| v.to_string()).collect::<Vec<_>>()
        };
        vec![
            Self {
                source: "local_storage",
                pinned_version_match: local.contains(requested),
                versions_found: to_strings(&local),
                status: None,
            },
            Self {
                source: "builtin",
                pinned_version_match: builtin.contains(requested),
                versions_found: to_strings(&builtin),
                status: None,
            },
            Self {
                source: "remote",
                versions_found: Vec::new(),
                pinned_version_match: false,
                status: Some("not_configured"),
            },
        ]
    }
}

/// Errors surfaced by [`Engine`].
///
/// `Backend` lifts [`BackendError`] verbatim through a `#[from]`
/// conversion so the engine layer's error envelope preserves the
/// backend's typed `Sealed` / `HashMismatch` payloads. The MCP layer
/// branches on the discriminant when mapping into the typed `code`
/// field of its error envelope.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// `Engine::from_mounts` received two mounts naming the same
    /// mem. Configuration error: the persistence adapter or
    /// caller produced a malformed mount list.
    #[error("duplicate mem in mount list: {0}")]
    DuplicateMem(String),
    /// No mount in this engine names the requested mem. Surfaced
    /// before reaching any backend so callers can distinguish
    /// "wrong mem name" from "backend failure".
    #[error("unknown mem: {0}")]
    UnknownMem(String),
    /// Mutation rejected because the mount declares
    /// [`MountCapability::ReadOnly`]. Surfaced before reaching the
    /// backend so the typed `Sealed` payload from the archive
    /// backend never triggers — capability gating runs first.
    #[error("mem {0} is mounted read-only; mutations rejected")]
    ReadOnlyMount(String),
    /// Entity type is not declared in the pinned schema for this
    /// mem. Carries the declared types (sorted) and a fuzzy
    /// suggestion so the agent can recover without re-reading the
    /// schema. `schema_ref` is the pinned `<name>@<version>`.
    #[error(
        "unknown entity type '{name}' in schema '{schema_ref}'. Declared types: [{}]{}",
        declared.join(", "),
        suggestion.as_deref().map(|s| format!(". Did you mean '{s}'?")).unwrap_or_default()
    )]
    UnknownType {
        name: String,
        schema_ref: String,
        declared: Vec<String>,
        suggestion: Option<String>,
    },
    /// Title slug is empty / invalid.
    #[error("title is invalid: {0}")]
    InvalidTitle(#[from] SlugError),
    /// Create attempted against an id already present in the store.
    #[error("entity already exists: {id}")]
    AlreadyExists { id: String },
    /// Mutation rejected because the named entity is not in the
    /// store. Distinct from `UnknownMem`: the mem exists, the
    /// entity does not.
    #[error("entity not found: {id}")]
    NotFound { id: String },
    /// Optimistic-locking failure: the caller's `expected_hash` does
    /// not match the entity's current `content_hash` in the store.
    /// `current` is the live hash — pass it as `expected_hash` after
    /// re-reading to retry. `is_stub` is set when the entity is a
    /// stub (no body, no content_hash); the corrective action is to
    /// pass `expected_hash: ""` rather than re-read via `memstead_entity`.
    /// Surfaces on `details.is_stub` so MCP callers branch on the
    /// structured payload instead of parsing the message text — pre-fix
    /// the wire emitted `(current: )` with an empty paren that
    /// misdirected toward hash-recovery for a stub-shaped entity.
    #[error("{}", _hash_mismatch_msg(id, current, *is_stub))]
    HashMismatch {
        id: String,
        current: String,
        is_stub: bool,
    },
    /// Refusal to delete or rename an entity because other entities
    /// in **Write-Mems** still reference it. There is no force flag
    /// or escape hatch — the agent removes the offending references
    /// (via `memstead_relate --remove` or `memstead_update`) before retrying.
    /// `referrers` carries the typed referrer info (source id,
    /// rel-type, source mem) so the response payload describes the
    /// full surface in one round-trip. ReadOnly-mount referrers are
    /// excluded from this list — they are handled by the residual-
    /// stub demotion path on the destructive mutation.
    #[error(
        "entity {id} has {n} incoming reference(s) in write mems ({inline}); remove them first via memstead_relate --remove or memstead_update",
        n = referrers.len(),
        inline = format_inline_list_overflow(referrers, "referrers"),
    )]
    HasIncomingRefs { id: String, referrers: Vec<ReferrerInfo> },
    /// Refusal to delete a mem because entities in other Write-Mems
    /// still reference entities inside it. Mirrors entity-level
    /// [`Self::HasIncomingRefs`] at the mem granularity — the
    /// edge-graph axis (F15 / CLI F8). Revoking a workspace-level grant only closes
    /// the policy axis; this check closes the actual-edge axis so a
    /// mem delete that would orphan cross-mem edges refuses with
    /// the typed envelope listing every offending `(from_id, rel_type,
    /// source_mem)` triple. No force flag — the operator must
    /// `memstead_relate --remove` (or `memstead_update` to drop the section)
    /// on each referrer first, then retry. ReadOnly-mount referrers
    /// stay out of this list and route through the residual-stub
    /// demotion path on the destructive mutation, same posture as the
    /// entity-level variant.
    #[error(
        "mem `{mem}` has {n} incoming reference(s) in write mems ({inline}); remove them first via memstead_relate --remove or memstead_update",
        n = referrers.len(),
        inline = format_inline_list_overflow(referrers, "referrers"),
    )]
    MemHasIncomingRefs {
        mem: String,
        referrers: Vec<ReferrerInfo>,
    },
    /// Relate across mems rejected because the workspace's
    /// `[cross_mem_links]` policy (or the per-create-rule
    /// `default_cross_links` synthesis) does not permit `from_mem →
    /// to_mem`. Agents adjust the policy or pick a same-mem
    /// target. The hint points at the workspace `[cross_mem_links]`
    /// section.
    #[error(
        "cross-mem link from mem `{from_mem}` to mem `{to_mem}` is not allowed by the workspace `[cross_mem_links]` policy"
    )]
    CrossMemLinkNotAllowed {
        from_mem: String,
        to_mem: String,
    },
    /// `memstead_relate` cross-mem to a target whose mem is mounted
    /// `MountCapability::ReadOnly` and the target is absent. Auto-stub
    /// is unavailable across the engine/ReadOnly-mem boundary (the
    /// engine cannot persist a stub in a mem it has no write access
    /// to), so the target must already exist before the relate call.
    #[error(
        "cross-mem relate target {target_id} is absent in read-only mem `{target_mem}` — auto-stub is unavailable across the read-only boundary; the target must exist before relating"
    )]
    CrossMemTargetNotFound {
        target_id: String,
        target_mem: String,
    },
    /// `memstead_relate` across mems pinning schemas with different
    /// *names* refused because the source schema's
    /// `cross_mem_relationships:` section declares no entry for the
    /// target schema's domain. Each source schema must explicitly
    /// enumerate outbound cross-mem edges per target domain; the
    /// absence here means the source schema does not speak the target
    /// domain's vocabulary. Eligibility is name-based — a declaration
    /// covers every version of the named target schema. The agent's
    /// recovery is to declare the rel-type in the source schema's
    /// `cross_mem_relationships:` section under the target's bare
    /// schema name (`to_schema: <name>`).
    ///
    /// Orthogonal to the `cross_mem_links` permission policy:
    /// vocabulary and permission fire independently. A policy-admissible
    /// edge that violates vocabulary surfaces here; a vocabulary-admissible
    /// edge that violates policy surfaces as
    /// [`Self::CrossMemLinkNotAllowed`].
    #[error(
        "cross-mem edge {rel_type} from `{from_id}` (schema {source_schema}) to `{to_id}` (schema {target_schema}) is not declared in {source_schema}'s `cross_mem_relationships:` section"
    )]
    CrossMemEdgeNotDeclared {
        source_schema: String,
        target_schema: String,
        rel_type: String,
        from_id: String,
        to_id: String,
    },
    /// `memstead_update` received repair-shaped input (`relations_unset`)
    /// for an entity that currently passes the conformance check.
    /// Repair-powers gate on evidence — a conformance failure on the
    /// target entity — and a conformant entity has the focused tools
    /// instead: `memstead_relate(remove)` detaches an edge, the additive
    /// `memstead_update` params evolve content. The entity is not
    /// modified.
    #[error("repair input refused for {id}: the entity currently passes the conformance check — {recovery}")]
    RepairNotNeeded { id: String, recovery: String },
    /// Rename where the new title would slugify to the existing id.
    /// Surfaced as a typed no-op so callers don't loop on a degenerate
    /// retry.
    #[error("rename would not change the id of {id} — new title {new_title:?} produces the same slug")]
    RenameNoOp { id: String, new_title: String },
    /// `memstead_update` / `memstead_batch_update` payload parsed cleanly but
    /// carries no recognised mutation content — every mutation map is
    /// empty and no relations are declared. Distinct from
    /// `UPDATE_NOOP` (a warning that fires when mutation content was
    /// provided but matched the current state): `EMPTY_UPDATE` is
    /// keyed on "no mutation content provided at all", and refuses
    /// before any mutation work runs so a misspelled/omitted mutation
    /// key doesn't silently land as `succeeded: 1, commit_sha: ""`.
    #[error(
        "no mutation content for {id} — payload carries an id but every mutation map is empty (recognised keys: sections, append_sections, patch_sections, metadata, metadata_unset, declare_relations, relations_unset)"
    )]
    EmptyUpdate { id: String },
    /// `memstead_rename` cannot proceed because one or more cross-mem
    /// referrers would emit a propagated rewrite whose direction the
    /// workspace's `cross_mem_links` policy does not permit. The
    /// engine refuses the rename up-front (before any write); the
    /// agent's recovery is either to grant the missing direction in
    /// `[cross_mem_links]` or to drop the offending edges first.
    ///
    /// Each `blocked_referrers` entry names a single blocked direction
    /// (`from_mem → to_mem`) — the referrer's mem and the
    /// renaming entity's mem, respectively — together with the
    /// count of distinct referrers in that mem that would emit the
    /// blocked rewrite. The direction is the edge's actual direction
    /// post-rewrite (`referrer → renamed`), which is what the policy
    /// gates.
    #[error(
        "rename blocked: cross-mem rewrite from referrer mem(s) into `{from_mem}` is not permitted by `[cross_mem_links]` — blocked: {} — grant the missing direction or rewrite the blocked referrers manually",
        format_blocked_referrers(blocked_referrers),
    )]
    RenameBlockedByCrossMemPolicy {
        from_mem: String,
        blocked_referrers: Vec<BlockedReferrer>,
    },
    /// `memstead_create` / `memstead_update` / `memstead_batch_update` refused
    /// because the post-mutation entity's section bodies contain
    /// inline wiki-links to targets that have no corresponding
    /// explicit relation in `entity.relationships`. Strict
    /// wiki-link / relation invariant: every body wiki-link must
    /// have a backing relation. The agent's recovery is
    /// `memstead_relate <this-entity> REFERENCES <target>` (or a more
    /// specific rel-type) for each missing entry, then re-issue
    /// the mutation. `missing` enumerates each violation as a
    /// `(section_key, target_id)` pair so the agent can fix every
    /// surviving link in one pass. This validator is gated behind
    /// the workspace's reference-coherence migration completion
    /// marker; workspaces that haven't been migrated continue
    /// running the permissive auto-stub regime.
    #[error(
        "post-mutation body of {from_id} has {n} wiki-link(s) without a backing relation ({inline}) — declare the relation(s) via memstead_relate first (REFERENCES or a more specific rel-type), then retry",
        n = missing.len(),
        inline = format_inline_list_overflow(missing, "missing"),
    )]
    WikiLinkWithoutRelation {
        from_id: String,
        missing: Vec<MissingWikiLink>,
    },
    /// `memstead_relate --remove` refused because the source entity's
    /// section bodies still contain `[[<target>]]` (or
    /// `[[<mem>:<target>]]`) wiki-links pointing at the relation's
    /// target. Removing the explicit relation while body links
    /// survive would violate the strict wiki-link/relation invariant
    /// (inline links require a backing relation). The agent's
    /// recovery is `memstead_update <source-id>` with section content
    /// that drops the wiki-link tokens, then re-issue `memstead_relate
    /// --remove`. `body_links` enumerates the surviving section keys
    /// so the agent can patch them in one pass.
    #[error(
        "cannot remove {rel_type} {from_id} → {to_id}: source body still contains wiki-link(s) to the target in section(s) {inline} — drop them via memstead_update before removing the relation",
        inline = format_inline_list_overflow(body_links, "body_links"),
    )]
    RelationHasBodyLinks {
        from_id: String,
        to_id: String,
        rel_type: String,
        body_links: Vec<String>,
    },
    /// A multi-mem `memstead_rename` partially landed: at least one
    /// mem committed successfully, then a subsequent per-mem
    /// commit aborted (typically because a sibling writer advanced
    /// the failed mem's head between the rename's snapshot and the
    /// commit attempt — the parent-ref pin tripped via
    /// `BackendError::ParentMismatch`). The committed mems' state
    /// has already landed and is durable; the failed mem's writes
    /// did not land. The agent's recovery options: retry the rename
    /// (reload the workspace first so the engine re-derives the right
    /// referrer set), or accept the partial state and reconcile
    /// manually via subsequent mutations.
    #[error(
        "rename partial-failure: mem `{failed_mem}` aborted with cause {failure_cause:?} after {committed_mems:?} already committed — reload and retry, or reconcile manually"
    )]
    RenamePartialFailure {
        committed_mems: Vec<String>,
        failed_mem: String,
        failure_cause: String,
    },
    /// `memstead_relate` source is a stub — stubs have no `entity_type`
    /// and cannot author edges. The agent must promote the stub to a
    /// real entity via `memstead_create` (stub adoption preserves any
    /// incoming references) before relating. Pre-fix surfaced as the
    /// cryptic `UnknownType { name: "" }`.
    #[error("source entity {id} is a stub — promote it to a real entity via memstead_create first")]
    StubCannotRelate { id: String },
    /// `memstead_update` target is a stub — stubs have no body, no
    /// metadata, no schema-resolved type to validate against. The
    /// agent must promote the stub to a real entity via `memstead_create`
    /// (stub adoption preserves any incoming references) before
    /// updating. Pre-Item-02 surfaced as the cryptic
    /// `UnknownType { name: "" }` cascade — identical symptom to the
    /// one `StubCannotRelate` was added to replace on `memstead_relate`.
    #[error("entity {id} is a stub — promote it to a real entity via memstead_create first")]
    StubNotUpdatable { id: String },
    /// `memstead_rename` target is a stub — stubs do not have a title to
    /// rename (their title is derived from the id). Same recovery
    /// path as [`Self::StubNotUpdatable`].
    #[error("entity {id} is a stub — promote it to a real entity via memstead_create before renaming")]
    StubNotRenamable { id: String },
    /// An `EntityId` reaching a write path (notably `memstead_relate to=`)
    /// does not match the wiki-link grammar
    /// (`^[a-z0-9-]+(/[a-z0-9-]+)*$` for the slug; `^[a-z0-9-]+$` for
    /// the mem). The gate prevents an auto-stub being created at a
    /// malformed id — once present, that stub would fail any
    /// downstream wiki-link parse that referenced it.
    #[error("entity id '{id}' is malformed: {reason}")]
    InvalidEntityId { id: String, reason: String },
    /// A body wiki-link target in a section body failed the strict
    /// slug-form grammar gate. The invariant is that every wiki-link target reaching
    /// `entity.relationships` carries a grammar-valid `EntityId` — the
    /// alias-synthesis pass would otherwise emit a relation pointing
    /// at a literal id (e.g. `mem--Knowledge Graph`) that no
    /// downstream wiki-link parse could ever resolve. `raw` is the
    /// input between brackets (after alias / `.md` strip); `suggested`
    /// is the `title_to_slug`-derived slug-form the agent lifts
    /// directly into the retry (omitted when the input has no
    /// meaningful canonical form — empty, all-punctuation, all-emoji);
    /// `section` is the section key whose body carried the link;
    /// `source` is a stable discriminator (`"body_link"`) future-
    /// proofed against additional ingress surfaces.
    #[error(
        "body wiki-link target '{raw}' in section '{section}' is not slug-form: {reason}"
    )]
    InvalidWikiLinkTarget {
        raw: String,
        suggested: Option<String>,
        section: String,
        link_source: String,
        reason: String,
    },
    /// A body wiki-link's Tier-2 mem prefix `[[mem:slug]]` failed
    /// the mem-name grammar (`^[a-z0-9-]+(/[a-z0-9-]+)*$`). Distinct
    /// from `InvalidWikiLinkTarget` because the recovery is different
    /// — mem names are fixed identifiers in the workspace, not
    /// free-form text the agent can mechanically slugify; the agent
    /// correlates the bad prefix against the workspace's known mems
    /// rather than reaching for `title_to_slug`.
    #[error(
        "body wiki-link mem prefix '{raw}' in section '{section}' is not a valid mem name: {reason}"
    )]
    InvalidWikiLinkMem {
        raw: String,
        section: String,
        reason: String,
    },
    /// `memstead_update` was asked to apply more than one section-
    /// mutation mode (`sections`, `append_sections`,
    /// `patch_sections`) to the same key. The request is ambiguous
    /// and rejected before any disk write. `modes` lists the
    /// conflicting modes for the key in canonical order.
    #[error("conflicting section modes for {section}: {modes:?}")]
    ConflictingSectionModes {
        section: String,
        modes: Vec<String>,
    },
    /// Adding the proposed edge would close a cycle in an
    /// acyclic-declared subgraph. Carries the existing back-path
    /// `[from, …, current, target's intermediates, … from]` so MCP
    /// envelopes ship the cycle's shape without a follow-up
    /// `memstead_search`. Truncated at
    /// [`RELATIONSHIP_CYCLE_PATH_CAP`] entries.
    #[error(
        "creating edge {rel_type} from '{from}' to '{to}' would close a cycle in the {rel_type} subgraph"
    )]
    RelationshipCycle {
        rel_type: String,
        from: EntityId,
        to: EntityId,
        existing_path: Vec<EntityId>,
        path_truncated: bool,
    },
    /// `memstead_update` received the same metadata key in both `metadata`
    /// (set) and `metadata_unset` lists. The request is ambiguous and
    /// rejected before any disk write — the caller picks which map the
    /// key belongs in. `keys` lists every overlapping key in alphabetical
    /// order so a single envelope describes the full conflict.
    #[error("metadata keys appear in both set and unset: {keys:?}")]
    SetAndUnsetConflict { keys: Vec<String> },
    /// `metadata_unset` targeted a required field. Carries the
    /// recovery payload so the agent reads the field's purpose,
    /// allowed values, and type-level write rules from the envelope
    /// rather than re-fetching the schema.
    ///
    /// Also fires from `memstead_create` when the
    /// caller did not supply a required metadata field that the
    /// schema does not auto-fill (`default_value` / `init_timestamp`
    /// / `auto_timestamp` all absent). Pre-fix the create path
    /// surfaced this as a `MISSING_REQUIRED_FIELD` warning and let
    /// the entity land with a placeholder — silently corrupted the
    /// export-then-install round-trip when the placeholder was
    /// invalid for the install-time strict validator. The refusal
    /// fires once per call on the first missing field (declaration
    /// order); subsequent fields surface on the next attempt.
    #[error("{}", _required_field_unset_msg(field, entity_type, *on_create))]
    RequiredFieldUnset {
        field: String,
        entity_type: String,
        /// Schema-supplied description of the field.
        field_description: Option<String>,
        /// Allowed enum values when the unset field is enum-typed;
        /// empty when the field is free-form.
        enum_values: Vec<String>,
        /// Type-level `write_rules` for the entity type.
        type_write_rules: Vec<String>,
        /// Path discriminator: `true` when the
        /// create path constructed the variant (caller didn't supply
        /// the field), `false` when the update path constructed it
        /// (caller passed `metadata_unset: ["field"]` against a
        /// required field). The typed code stays `REQUIRED_FIELD_UNSET`
        /// on both paths; only the rendered prose differs.
        ///
        /// Not exposed on the `details` payload — agents already
        /// branch on the typed code; the new field is for the prose
        /// dispatch only.
        on_create: bool,
        /// Multi-field
        /// accumulator on the create path. Every required-no-default
        /// field that was unset, in schema declaration order. Empty
        /// on the unset path (where the agent targets one field by
        /// definition and the singular fields above are authoritative);
        /// always non-empty (and at least a singleton echo of the
        /// singular fields) on the create path.
        ///
        /// Surfaces on `details.missing[]` so an agent fixes every
        /// missing field in one round-trip. `details.field` and
        /// `details.missing[0].field` agree on the first-missing
        /// entry, keeping the back-compat singular-field shape.
        missing: Vec<MissingRequiredField>,
    },
    /// `memstead_create`: one or more required sections for the entity's
    /// type were absent or whitespace-only in the request. Pre-fix
    /// the create path surfaced this as `MISSING_REQUIRED_SECTION`
    /// warnings and wrote the entity with empty placeholders for
    /// the missing sections; the resulting on-disk state failed the
    /// install-time strict validator, breaking the export-then-
    /// install round-trip. The refusal carries every missing section
    /// (one entry per affected key) plus the type-level `type_guidance`
    /// map so the agent has a single round-trip recovery via re-call
    /// with the missing content filled in.
    ///
    /// Loader / health / `memstead_update` paths keep their permissive
    /// posture — a legacy on-disk entity created when this gate was
    /// a warning continues to load, surface in health, and accept
    /// partial updates. The refusal is a write-boundary gate, not a
    /// global invariant.
    #[error(
        "missing {missing_count} required section(s) for type '{entity_type}'"
    )]
    MissingRequiredSection {
        entity_type: String,
        /// Echoed for diagnostics; equals `sections.len()`.
        missing_count: usize,
        /// One entry per missing required section, in schema
        /// declaration order. Each entry mirrors the shape of the
        /// pre-fix `WarningHint::MissingRequiredSection` warning so
        /// agents reading the recovery payload don't branch on
        /// surface (refusal vs warning).
        sections: Vec<crate::runtime_validator::MissingRequiredSection>,
        /// Type-level `write_rules` keyed by `entity_type`. Map shape
        /// matches the mutation-response top-level `type_guidance`
        /// the warning-surface ships so a single decoder reads
        /// guidance from either path.
        type_guidance: std::collections::BTreeMap<String, Vec<String>>,
    },
    /// `patch_sections` targeted a key whose section body is
    /// absent from the entity (or has never been authored).
    #[error("patch target section is empty: {section}")]
    PatchSectionEmpty { section: String },
    /// `patch_sections` provided an `old` substring that does not
    /// appear in the section's current body. Carries a truncated
    /// snapshot of the current content so the caller can surface
    /// the actual state to the operator.
    #[error("patch `old` substring not found in {section}")]
    PatchOldNotFound {
        section: String,
        current_content: String,
        truncated: bool,
    },
    /// Schema-strictness rejection from the runtime validator
    /// (`UNKNOWN_SECTION`, `UNKNOWN_METADATA`, `INVALID_ENUM_VALUE`).
    #[error("schema validation: {0}")]
    Validation(#[from] ValidationError),
    /// Re-parse of the freshly-generated markdown failed. Should
    /// never happen — the generator's contract is that its output
    /// round-trips through `parse_markdown`. Surfaces if a future
    /// generator change breaks that invariant.
    #[error("parse-after-write failed: {0}")]
    ParseAfterWrite(String),
    /// A wrapped parse error for completeness; today only the
    /// parse-after-write variant above is constructed in the create
    /// path.
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),
    /// A backend operation failed. Inner error carries the typed
    /// payload (e.g. `Sealed`, `HashMismatch`, `Io`).
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// A mem's schema pin did not resolve. `sources` carries the
    /// fixed-order resolution diagnostics (local storage / built-in /
    /// remote) so the caller can tell *where* the pin failed and spot a
    /// right-name/wrong-version partial match; it surfaces under
    /// `details.sources`. Empty `sources` marks an internal lookup miss
    /// (an already-resolved schema absent from the engine's per-mem
    /// map), not a genuine source-resolution failure.
    #[error("mem {mem}: schema pin {pin:?} did not resolve in any schema source")]
    SchemaNotFound {
        mem: String,
        pin: String,
        sources: Vec<SchemaSourceDiagnostic>,
    },
    /// `memstead_schema::builtins::load_builtin_schemas` itself failed.
    /// Surfaces during `Engine::from_mounts`; should never trip in
    /// practice (the built-in catalogue is statically embedded), but
    /// the failure path is preserved so a future on-disk catalogue
    /// switch lifts cleanly.
    #[error("built-in schema catalogue failed to load: {0}")]
    SchemaResolverInit(String),
    /// Generic mem-level error message — used by accessors that
    /// surface "mem exists, but the requested resource is not
    /// available for this backend" (e.g. `gitdir_for` against a
    /// folder mount, `worktree_for` against a git-branch mount).
    #[error("mem error: {0}")]
    Mem(String),
    /// `register_writable_mem` rejected because `name` is already
    /// registered (writable OR read-only). `source_origin` is the
    /// human-readable description of the colliding registration,
    /// rendered via [`MemOrigin::render_source`] for writable
    /// entries or a stand-in for read-only ones.
    #[error("mem name collision: {name} is already registered ({source_origin})")]
    MemNameCollision {
        name: String,
        source_origin: String,
    },
    /// Lifecycle orchestrator rejected the input. Carries a single
    /// free-form message — the orchestrator's typed payload (note
    /// length, malformed path, etc.) is the message text.
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// `memstead_fetch` / `memstead_pull` / `memstead_push` named a remote that is
    /// not configured on the workspace's mem-repo. Typed code
    /// `UNKNOWN_REMOTE`. Recovery: configure the remote via `git
    /// remote add` or pick a remote `git remote -v` already lists.
    #[error("unknown remote: {0}")]
    UnknownRemote(String),
    /// `memstead_pull` refused because the local branch has diverged from
    /// the remote-tracking ref — fast-forward is impossible without
    /// losing local commits. Recovery: run `memstead branch-reset` to the
    /// remote-tracking ref (if the local commits are dispensable) or
    /// run a replay workflow to rewrite them onto the new remote tip.
    /// Typed code `LOCAL_DIVERGENCE`.
    #[error(
        "mem `{mem}`'s local branch has diverged from `{remote_ref}` — pull cannot fast-forward without losing local commits; rebase / replay first or run memstead branch-reset"
    )]
    LocalDivergence { mem: String, remote_ref: String },
    /// `memstead_push` refused because the push would not be a fast-forward
    /// against the remote and the caller did not pass `force: true`.
    /// Typed code `NON_FAST_FORWARD`. Recovery: re-fetch + replay, or
    /// re-issue with `force: true` (warning: rewrites the remote's
    /// view of the branch — other peers will see the rewrite).
    #[error(
        "push to remote `{remote}` for mem `{mem}` is not a fast-forward; rebase / replay locally or pass `force: true` to overwrite the remote"
    )]
    NonFastForward { mem: String, remote: String },
    /// `memstead_push` refused because the local state failed pre-push
    /// schema validation. The remote was not contacted. Recovery: fix
    /// the schema violations (use `memstead_health` to find them) and
    /// retry. Typed code `LOCAL_INVALID_STATE`.
    #[error(
        "mem `{mem}` failed pre-push schema validation; remote `{remote}` was not contacted: {detail}"
    )]
    LocalInvalidState {
        mem: String,
        remote: String,
        detail: String,
    },
    /// `memstead_pull` (or any future merge path that consumes fetched
    /// commits) refused because the prospective post-merge tree
    /// contains entities that fail schema validation. The branch
    /// pointer was not moved. `violations` carries one entry per
    /// offending entity — typically `(relative_path, parse_error)`
    /// pairs rendered as strings — so the caller can surface the
    /// remediation surface without re-walking the tree. Typed code
    /// `SCHEMA_VIOLATION_IN_FETCH`.
    #[error(
        "mem `{mem}` would fail schema validation at `{ref_name}` — {n} violation(s); fix the remote or replay locally first",
        n = violations.len(),
    )]
    SchemaViolationInFetch {
        mem: String,
        ref_name: String,
        violations: Vec<String>,
    },
    /// `memstead_branch_reset` refused because at least one commit that
    /// would be discarded by the reset is already reachable from a
    /// `refs/remotes/*` ref (the engine's definition of "pushed").
    /// `pushed_shas` lists the offending commits. The agent's
    /// recovery is to pick a target SHA that does not strand a pushed
    /// commit, or to push the pre-reset state under a different
    /// branch name first. Typed code: `PUSHED_COMMITS_PROTECTED`.
    #[error(
        "branch_reset refused: {} pushed commit(s) would be discarded ({}); pick a target that preserves the pushed segment or push the pre-reset state under a different branch first",
        pushed_shas.len(),
        pushed_shas.join(", "),
    )]
    PushedCommitsProtected {
        mem: String,
        target_sha: String,
        pushed_shas: Vec<String>,
    },
    /// `memstead_diff` (or any future ref-comparing op) received a ref
    /// that does not resolve against the workspace's mem-repo.
    /// Carries the ref string verbatim so the caller can fix the
    /// input. Typed code `UNKNOWN_REF`.
    #[error("unknown ref: {0}")]
    UnknownRef(String),
    /// `memstead_changes_since` received a `rename_similarity` value
    /// outside the allowed range. Maps to wire code `INVALID_INPUT`
    /// with `details.allowed_range: [min, max]` and
    /// `details.requested`. Promoted from the prior silent-clamp + LIMIT_CLAMPED warning so
    /// nonsense inputs surface as recoverable refusal rather than
    /// silent rounding.
    #[error(
        "rename_similarity {requested} outside allowed range [{allowed_min}, {allowed_max}]"
    )]
    RenameSimilarityOutOfRange {
        requested: f32,
        allowed_min: f32,
        allowed_max: f32,
    },
    /// `memstead_changes_since` / `memstead changes --since` was given a `since`
    /// commit cursor the mem's git repository can't resolve — a
    /// malformed prefix or a well-formed-but-absent 40-hex. Surfaces the
    /// `INVALID_CURSOR` code (the documented contract for this op, which
    /// the CLI previously leaked as the `MEM_ERROR` catch-all) so a
    /// sync loop branches cleanly: `INVALID_CURSOR` → re-seed from the
    /// empty-tree sentinel; `MEM_ERROR` → genuine backend fault.
    /// `details.since` carries the offending cursor untruncated.
    #[error("commit cursor '{since}' is not a known commit in mem '{mem}' — pass a commit_sha from a prior mutation, or the empty-tree sentinel to re-seed")]
    InvalidChangesCursor { mem: String, since: String },
    /// Mem config is missing a required field that the engine
    /// itself would normally populate (today: `version` at mem
    /// init). Surfaced on the export path — pre-fix this collapsed
    /// to `INTERNAL` with a misleading `.memstead/config.json` reference
    /// that doesn't match the mem-repo backend's blob layout.
    /// Recovery: run `memstead mem set-version <mem> <version>` to
    /// populate the field, then retry the export. F1.
    #[error(
        "mem `{mem}` config is missing required field(s) {missing_fields:?} — \
         set via `memstead mem set-version {mem} <version>` (e.g. 0.1.0)"
    )]
    MemConfigIncomplete {
        mem: String,
        missing_fields: Vec<String>,
    },
    /// `memstead_relate` (or a `declare_relations` entry) targeted a
    /// rel-type whose schema declares `per_edge_description:
    /// required` without supplying a description. Recovery: re-issue
    /// the call with `--description "<text>"` describing why this
    /// particular edge exists (the rel-type's name documents the
    /// kind of edge; the description documents the instance).
    #[error(
        "rel-type `{rel_type}` declares `per_edge_description: required` — \
         {from_id} → {to_id} needs a description; re-issue with \
         `--description \"<text>\"`."
    )]
    MissingRequiredDescription {
        rel_type: String,
        from_id: String,
        to_id: String,
    },
    /// `memstead_relate` (or a `declare_relations` entry) supplied a
    /// description for a rel-type whose schema declares
    /// `per_edge_description: forbidden`. Recovery: drop the
    /// `description` parameter — the rel-type's name describes the
    /// edge; per-edge text is not permitted on this rel-type.
    #[error(
        "rel-type `{rel_type}` declares `per_edge_description: forbidden` — \
         {from_id} → {to_id} cannot carry a description; drop the \
         `--description` argument."
    )]
    DescriptionNotPermitted {
        rel_type: String,
        from_id: String,
        to_id: String,
    },
    /// `memstead_relate` (or a `declare_relations` / `memstead_create`'s
    /// inline `relations:` entry) targeted a rel-type whose schema
    /// declares `manual_authoring: forbidden`. The rel-type is
    /// reserved for engine-emitted synthesis (the body-link →
    /// relation alias machinery, typically). Recovery: don't author
    /// the relation explicitly; instead author a body wiki-link
    /// `[[target]]` in the source's section content, which the
    /// engine surfaces as the appropriate alias relation
    /// automatically.
    #[error(
        "rel-type `{rel_type}` declares `manual_authoring: forbidden` — \
         {from_id} → {to_id} cannot be authored explicitly; this rel-type \
         is reserved for engine-emitted synthesis via the body-link → \
         relation alias path. {guidance}"
    )]
    RelationManualAuthoringForbidden {
        rel_type: String,
        from_id: String,
        to_id: String,
        guidance: String,
    },
    /// Full-text search is unavailable in the current engine build —
    /// `Engine::search` is callable on every target so JS / FFI
    /// consumers don't need to re-shape their call sites, but `wasm32`
    /// builds omit the tantivy index entirely (its native-only
    /// transitives — `getrandom 0.2` without `js`, `memmap2`, `rayon`,
    /// `zstd-sys` — block WASM compilation). Browser consumers route
    /// queries to the bridge's `memstead_search` endpoint. The MCP layer
    /// maps this to typed code `SEARCH_UNAVAILABLE_IN_WASM`.
    #[error(
        "full-text search is unavailable in this engine build (wasm32); \
         route search queries to the bridge's memstead_search endpoint"
    )]
    SearchUnavailable,
    /// `memstead export --format markdown --mem-name <V>` was called
    /// against a mem whose active backend doesn't support markdown
    /// regeneration in place (today: every backend other than
    /// `folder`). Pre-fix this collapsed to a silent
    /// `ExportResult { written: 0, unchanged: 0 }` masquerading as
    /// success. Recovery: use `--format mem` to produce a portable
    /// `.mem` archive, which every backend supports.
    #[error(
        "mem `{mem}` is on backend `{active_backend}`; `memstead export --format markdown` \
         is supported only on backends [{}] — use `--format mem` to produce a portable \
         `.mem` archive instead",
        supported_backends.join(", ")
    )]
    MarkdownExportUnsupportedBackend {
        mem: String,
        active_backend: String,
        supported_backends: Vec<String>,
    },
}

/// Typed payload for a single Write-Mem referrer in
/// [`EngineError::HasIncomingRefs`]. Captures the (from_id, rel_types,
/// mem) triple the surface envelope projects so consumers can reason
/// about the offending edges without a follow-up `memstead_entity` call.
/// The mem is always a Write-Mem — ReadOnly referrers are
/// partitioned out before this struct is constructed and surfaced via
/// the residual-stub warning channel instead.
///
/// Per-source deduplication: when one source entity has multiple
/// edges of different rel-types pointing at the deletion target, the
/// engine collapses them into a single `ReferrerInfo` whose
/// `rel_types` list carries every edge type. A prior shape
/// emitted one entry per edge, making a source-with-N-edges look
/// like N distinct referrers in the error message and structured
/// payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReferrerInfo {
    pub from_id: String,
    pub rel_types: Vec<String>,
    pub mem: String,
}

/// Inline rendering on the text mirror. Single rel-type renders as
/// just the referring entity id; multiple rel-types append the
/// `×N [REL1, REL2]` annotation so the count and the offending
/// edge-types stay visible without parsing the structured payload.
impl fmt::Display for ReferrerInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.rel_types.len() <= 1 {
            f.write_str(&self.from_id)
        } else {
            write!(
                f,
                "{} ×{} [{}]",
                self.from_id,
                self.rel_types.len(),
                self.rel_types.join(", ")
            )
        }
    }
}

/// One body wiki-link that violates the strict wiki-link /
/// relation invariant. Surfaces inside
/// [`EngineError::WikiLinkWithoutRelation::missing`].
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct MissingWikiLink {
    /// Section key of the entity body where the unbacked
    /// wiki-link appears.
    pub section_key: String,
    /// EntityId target of the unbacked wiki-link.
    pub target_id: String,
}

/// Inline rendering pairs the section key with the unbacked target id
/// so an agent reading only the text mirror can see both where the link
/// lives and what it points at without decoding the structured payload.
impl fmt::Display for MissingWikiLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}→{}", self.section_key, self.target_id)
    }
}

impl EngineError {
    /// Stable, surface-independent error code token.
    ///
    /// Each surface (MCP envelope, CLI envelope, UniFFI binding) maps
    /// the variant to its wire shape; the code returned here is the
    /// canonical name agents key on. Add a new code here when a new
    /// variant lands; do not invent ad-hoc strings inside the
    /// per-surface mapping.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::DuplicateMem(_) => "DUPLICATE_MEM",
            EngineError::UnknownMem(_) => "UNKNOWN_MEM",
            EngineError::UnknownRef(_) => "UNKNOWN_REF",
            EngineError::UnknownRemote(_) => "UNKNOWN_REMOTE",
            EngineError::LocalDivergence { .. } => "LOCAL_DIVERGENCE",
            EngineError::NonFastForward { .. } => "NON_FAST_FORWARD",
            EngineError::LocalInvalidState { .. } => "LOCAL_INVALID_STATE",
            EngineError::SchemaViolationInFetch { .. } => "SCHEMA_VIOLATION_IN_FETCH",
            EngineError::PushedCommitsProtected { .. } => "PUSHED_COMMITS_PROTECTED",
            EngineError::ReadOnlyMount(_) => "READ_ONLY_MOUNT",
            EngineError::UnknownType { .. } => "UNKNOWN_ENTITY_TYPE",
            EngineError::InvalidTitle(_) => "INVALID_TITLE",
            EngineError::AlreadyExists { .. } => "ENTITY_ALREADY_EXISTS",
            EngineError::NotFound { .. } => "ENTITY_NOT_FOUND",
            EngineError::HashMismatch { .. } => "HASH_MISMATCH",
            EngineError::HasIncomingRefs { .. } => "HAS_INCOMING_REFS",
            EngineError::MemHasIncomingRefs { .. } => "MEM_HAS_INCOMING_REFS",
            EngineError::CrossMemLinkNotAllowed { .. } => "CROSS_MEM_LINK_NOT_ALLOWED",
            EngineError::CrossMemTargetNotFound { .. } => "CROSS_MEM_TARGET_NOT_FOUND",
            EngineError::CrossMemEdgeNotDeclared { .. } => "CROSS_MEM_EDGE_NOT_DECLARED",
            EngineError::RepairNotNeeded { .. } => "REPAIR_NOT_NEEDED",
            EngineError::RenameNoOp { .. } => "RENAME_NO_OP",
            EngineError::EmptyUpdate { .. } => "EMPTY_UPDATE",
            EngineError::RenameBlockedByCrossMemPolicy { .. } => {
                "RENAME_BLOCKED_BY_CROSS_MEM_POLICY"
            }
            EngineError::RenamePartialFailure { .. } => "RENAME_PARTIAL_FAILURE",
            EngineError::RelationHasBodyLinks { .. } => "RELATION_HAS_BODY_LINKS",
            EngineError::WikiLinkWithoutRelation { .. } => "WIKILINK_WITHOUT_RELATION",
            EngineError::StubCannotRelate { .. } => "STUB_CANNOT_RELATE",
            EngineError::StubNotUpdatable { .. } => "STUB_NOT_UPDATABLE",
            EngineError::StubNotRenamable { .. } => "STUB_NOT_RENAMABLE",
            EngineError::InvalidEntityId { .. } => "INVALID_ENTITY_ID",
            EngineError::InvalidWikiLinkTarget { .. } => "INVALID_WIKI_LINK_TARGET",
            EngineError::InvalidWikiLinkMem { .. } => "INVALID_MEM_NAME",
            EngineError::ConflictingSectionModes { .. } => "CONFLICTING_SECTION_MODES",
            EngineError::RelationshipCycle { .. } => "RELATIONSHIP_CYCLE",
            EngineError::SetAndUnsetConflict { .. } => "SET_AND_UNSET_CONFLICT",
            EngineError::RequiredFieldUnset { .. } => "REQUIRED_FIELD_UNSET",
            EngineError::MissingRequiredSection { .. } => "MISSING_REQUIRED_SECTION",
            EngineError::PatchSectionEmpty { .. } => "PATCH_SECTION_EMPTY",
            EngineError::PatchOldNotFound { .. } => "PATCH_OLD_NOT_FOUND",
            EngineError::Validation(v) => v.code(),
            EngineError::ParseAfterWrite(_) => "PARSE_ERROR",
            EngineError::Parse(_) => "PARSE_ERROR",
            EngineError::Backend(_) => "MEM_ERROR",
            EngineError::SchemaNotFound { .. } => "SCHEMA_NOT_FOUND",
            EngineError::SchemaResolverInit(_) => "SCHEMA_RESOLVER_INIT_FAILED",
            EngineError::Mem(_) => "MEM_ERROR",
            EngineError::MemNameCollision { .. } => "MEM_NAME_COLLISION",
            EngineError::InvalidInput(_) => "INVALID_INPUT",
            EngineError::RenameSimilarityOutOfRange { .. } => "INVALID_INPUT",
            EngineError::InvalidChangesCursor { .. } => "INVALID_CURSOR",
            EngineError::MemConfigIncomplete { .. } => "MEM_CONFIG_INCOMPLETE",
            EngineError::MissingRequiredDescription { .. } => "MISSING_REQUIRED_DESCRIPTION",
            EngineError::DescriptionNotPermitted { .. } => "DESCRIPTION_NOT_PERMITTED",
            EngineError::RelationManualAuthoringForbidden { .. } => {
                "RELATION_MANUAL_AUTHORING_FORBIDDEN"
            }
            EngineError::SearchUnavailable => "SEARCH_UNAVAILABLE_IN_WASM",
            EngineError::MarkdownExportUnsupportedBackend { .. } => {
                "MARKDOWN_EXPORT_UNSUPPORTED_BACKEND"
            }
        }
    }

    /// Variant-specific recovery payload, rendered as a structured
    /// JSON object that surfaces under `error.details` in MCP /
    /// CLI envelopes.
    ///
    /// Pre-fix the
    /// batch-update per-item envelope (`batch_error_envelope`)
    /// shipped `{}` for every typed code except `Validation`, while
    /// the singleton-call surfaces (`CliError::from_engine_op`,
    /// `memstead-mcp`'s `engine_err_unified`) populated structured
    /// payloads per-variant. Two envelopes, two details paths —
    /// agents' "fix from `details`" recovery loop worked
    /// differently in batch vs singleton mode. The centralised
    /// helper here gives both surfaces one source of truth.
    ///
    /// Returns an empty object for variants whose recovery payload
    /// is the message text alone (no structured fields beyond
    /// `code` + `message`).
    pub fn details(&self) -> serde_json::Value {
        match self {
            EngineError::NotFound { id } => serde_json::json!({ "id": id }),
            EngineError::RepairNotNeeded { id, recovery } => {
                serde_json::json!({ "id": id, "recovery": recovery })
            }
            // Same shape the pro MCP singleton envelope ships for
            // UNKNOWN_ENTITY_TYPE — keeps the centralised helper (and
            // every consumer: batch envelopes, the integrity linter)
            // aligned with the wire payload agents already decode.
            EngineError::UnknownType {
                name,
                schema_ref,
                declared,
                suggestion,
            } => serde_json::json!({
                "name": name,
                "schema_ref": schema_ref,
                "declared": declared,
                "suggestion": suggestion,
            }),
            EngineError::HashMismatch { id, current, is_stub } => serde_json::json!({
                "id": id,
                "current": current,
                "is_stub": is_stub,
            }),
            EngineError::HasIncomingRefs { id, referrers } => {
                let referrers_json: Vec<_> = referrers
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "from_id": r.from_id,
                            "rel_types": r.rel_types,
                            "mem": r.mem,
                        })
                    })
                    .collect();
                serde_json::json!({ "id": id, "referrers": referrers_json })
            }
            EngineError::MemHasIncomingRefs { mem, referrers } => {
                let referrers_json: Vec<_> = referrers
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "from_id": r.from_id,
                            "rel_types": r.rel_types,
                            "mem": r.mem,
                        })
                    })
                    .collect();
                serde_json::json!({ "mem": mem, "referrers": referrers_json })
            }
            EngineError::WikiLinkWithoutRelation { from_id, missing } => serde_json::json!({
                "from_id": from_id,
                "missing": missing,
            }),
            EngineError::RelationHasBodyLinks { from_id, to_id, rel_type, body_links } => {
                serde_json::json!({
                    "from_id": from_id,
                    "to_id": to_id,
                    "rel_type": rel_type,
                    "body_links": body_links,
                })
            }
            EngineError::InvalidEntityId { id, reason } => {
                serde_json::json!({ "id": id, "reason": reason })
            }
            EngineError::InvalidWikiLinkTarget {
                raw,
                suggested,
                section,
                link_source,
                reason,
            } => {
                // Surface
                // the slug-form retry under `proposed_slug`, mirroring the
                // title gate's `INVALID_TITLE` recovery key, so an agent
                // that wrote `[[Idempotency]]` finds `idempotency` under
                // the same field it already knows. `suggested` is the
                // general hint and is sometimes a colon-form
                // (`mem:slug`) for the ambiguous-grammar case — only
                // promote it to `proposed_slug` when it's a bare slug.
                let proposed_slug = suggested
                    .as_ref()
                    .filter(|s| !s.contains(':') && !s.contains("--"));
                serde_json::json!({
                    "raw": raw,
                    "suggested": suggested,
                    "proposed_slug": proposed_slug,
                    "section": section,
                    "source": link_source,
                    "reason": reason,
                })
            }
            EngineError::InvalidWikiLinkMem { raw, section, reason } => {
                serde_json::json!({ "raw": raw, "section": section, "reason": reason })
            }
            EngineError::ConflictingSectionModes { section, modes } => {
                serde_json::json!({ "section": section, "modes": modes })
            }
            EngineError::SetAndUnsetConflict { keys } => serde_json::json!({ "keys": keys }),
            EngineError::RequiredFieldUnset {
                field,
                entity_type,
                field_description,
                enum_values,
                type_write_rules,
                // `on_create` is a prose-dispatch
                // discriminator only; agents branch on the typed
                // `REQUIRED_FIELD_UNSET` code, not on this field.
                on_create: _,
                missing,
            } => {
                // `details.missing[]` carries every required-no-
                // default field unset on the create path so an
                // agent fixes the whole set in one retry. Each
                // entry echoes the type-level `write_rules` for
                // self-containment. Empty on the unset path.
                let missing_json: Vec<_> = missing
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "field": m.key,
                            "description": m.description,
                            "enum_values": m.enum_values,
                            "write_rules": type_write_rules,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "field": field,
                    "entity_type": entity_type,
                    "field_description": field_description,
                    "enum_values": enum_values,
                    "type_write_rules": type_write_rules,
                    "missing": missing_json,
                })
            }
            EngineError::MissingRequiredSection {
                entity_type,
                missing_count,
                sections,
                type_guidance,
            } => {
                let sections_json: Vec<_> = sections
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "entity_type": s.entity_type,
                            "key": s.key,
                            "heading": s.heading,
                            "write_rules": s.write_rules,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "entity_type": entity_type,
                    "missing_count": missing_count,
                    "sections": sections_json,
                    "type_guidance": type_guidance,
                })
            }
            EngineError::PatchSectionEmpty { section } => serde_json::json!({ "section": section }),
            EngineError::PatchOldNotFound { section, current_content, truncated } => {
                serde_json::json!({
                    "section": section,
                    "current_content": current_content,
                    "truncated": truncated,
                })
            }
            EngineError::RelationshipCycle {
                rel_type,
                from,
                to,
                existing_path,
                path_truncated,
            } => {
                let path_json: Vec<_> =
                    existing_path.iter().map(|id| id.to_string()).collect();
                serde_json::json!({
                    "rel_type": rel_type,
                    "from": from.to_string(),
                    "to": to.to_string(),
                    "existing_path": path_json,
                    "path_truncated": path_truncated,
                })
            }
            EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => {
                serde_json::json!({ "from_mem": from_mem, "to_mem": to_mem })
            }
            EngineError::EmptyUpdate { id } => {
                serde_json::json!({
                    "id": id,
                    "recognised_keys": [
                        "sections", "append_sections", "patch_sections",
                        "metadata", "metadata_unset", "declare_relations", "relations_unset",
                    ],
                })
            }
            EngineError::RenameBlockedByCrossMemPolicy {
                from_mem,
                blocked_referrers,
            } => {
                let entries: Vec<_> = blocked_referrers
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "from_mem": r.from_mem,
                            "to_mem": r.to_mem,
                            "count": r.count,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "from_mem": from_mem,
                    "blocked_referrers": entries,
                })
            }
            EngineError::CrossMemTargetNotFound { target_id, target_mem } => {
                serde_json::json!({ "target_id": target_id, "target_mem": target_mem })
            }
            EngineError::Validation(v) => v.details(),
            EngineError::MissingRequiredDescription { rel_type, from_id, to_id } => {
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })
            }
            EngineError::DescriptionNotPermitted { rel_type, from_id, to_id } => {
                serde_json::json!({
                    "rel_type": rel_type,
                    "from_id": from_id,
                    "to_id": to_id,
                })
            }
            EngineError::RelationManualAuthoringForbidden {
                rel_type,
                from_id,
                to_id,
                guidance,
            } => serde_json::json!({
                "rel_type": rel_type,
                "from_id": from_id,
                "to_id": to_id,
                "guidance": guidance,
            }),
            EngineError::MarkdownExportUnsupportedBackend {
                mem,
                active_backend,
                supported_backends,
            } => serde_json::json!({
                "mem": mem,
                "active_backend": active_backend,
                "supported_backends": supported_backends,
            }),
            EngineError::InvalidChangesCursor { mem, since } => serde_json::json!({
                "mem": mem,
                "since": since,
            }),
            EngineError::SchemaNotFound { mem, pin, sources } => serde_json::json!({
                "mem": mem,
                "pin": pin,
                "sources": sources,
            }),
            _ => serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Render rich, fully-inlined recovery prose for the agent-visible
    /// text channel.
    ///
    /// Warnings
    /// already render their structured payload inline via
    /// `WarningHint::Display`; pre-fix errors with rich payloads
    /// collapsed to `Display` plus `format_inline_list_overflow`'s
    /// "+N more — see details.X" pointer pointing at a structured
    /// channel the agent's MCP client doesn't surface to the model.
    /// This method gives errors the same prose-rich rendering warnings
    /// have, so `result.content[0].text` is self-recoverable.
    ///
    /// Variants whose `Display` already inlines every recovery field
    /// (no truncation, no "see details" pointer) inherit the default
    /// trait impl — they just `to_string()`. Override only the
    /// variants that need richer rendering than `Display` provides.
    ///
    /// The structured `details()` channel is unchanged; consumers
    /// branching on `code` continue to receive the typed shape. The
    /// `Display` impl stays terse for logs, tracing, panic messages,
    /// and other non-agent consumers.
    pub fn prose_render(&self) -> String {
        match self {
            EngineError::HasIncomingRefs { id, referrers } => {
                let inline = render_referrers_inline(referrers);
                format!(
                    "entity {id} has {n} incoming reference(s) in write mems ({inline}); remove them first via memstead_relate --remove or memstead_update",
                    n = referrers.len(),
                )
            }
            EngineError::MemHasIncomingRefs { mem, referrers } => {
                let inline = render_referrers_inline(referrers);
                format!(
                    "mem `{mem}` has {n} incoming reference(s) in write mems ({inline}); remove them first via memstead_relate --remove or memstead_update",
                    n = referrers.len(),
                )
            }
            EngineError::WikiLinkWithoutRelation { from_id, missing } => {
                let inline = missing
                    .iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "post-mutation body of {from_id} has {n} wiki-link(s) without a backing relation ({inline}) — declare the relation(s) via memstead_relate first (REFERENCES or a more specific rel-type), then retry",
                    n = missing.len(),
                )
            }
            EngineError::RelationHasBodyLinks {
                from_id,
                to_id,
                rel_type,
                body_links,
            } => {
                let inline = body_links.join(", ");
                format!(
                    "cannot remove {rel_type} {from_id} → {to_id}: source body still contains wiki-link(s) to the target in section(s) {inline} — drop them via memstead_update before removing the relation"
                )
            }
            EngineError::RelationshipCycle {
                rel_type,
                from,
                to,
                existing_path,
                path_truncated,
            } => {
                let path_inline = if existing_path.is_empty() {
                    String::from("(unavailable)")
                } else {
                    existing_path
                        .iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(" → ")
                };
                let trunc = if *path_truncated { " (path truncated)" } else { "" };
                format!(
                    "creating edge {rel_type} from '{from}' to '{to}' would close a cycle in the {rel_type} subgraph — existing path: {path_inline}{trunc}; remove an edge along this path to break the cycle, then retry"
                )
            }
            EngineError::RequiredFieldUnset {
                field,
                entity_type,
                field_description,
                enum_values,
                type_write_rules,
                on_create,
                missing,
            } => {
                let desc_clause = field_description
                    .as_deref()
                    .map(|d| format!(" Field purpose: {d}."))
                    .unwrap_or_default();
                let enum_clause = if enum_values.is_empty() {
                    String::new()
                } else {
                    format!(" Allowed values: {}.", enum_values.join(", "))
                };
                let rules_clause = if type_write_rules.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Type-level write_rules: {}.",
                        type_write_rules.join("; ")
                    )
                };
                // Path-aware wording — create
                // path says "not provided"; update path says "cannot
                // unset". Display impl shares the same dispatch via
                // `_required_field_unset_msg`.
                let lead = if *on_create {
                    format!(
                        "required metadata field '{field}' not provided — type '{entity_type}' declares the field as required and has no default for it"
                    )
                } else {
                    format!("cannot unset required field '{field}' for type '{entity_type}'")
                };
                // Multi-field accumulator. On the create path,
                // append a tail-list naming every other unset
                // required field so the agent's one-shot retry
                // covers all of them. The unset path's `missing`
                // is empty (or singleton), so the clause is empty
                // there.
                let tail_clause = if missing.len() > 1 {
                    let others: Vec<&str> =
                        missing.iter().skip(1).map(|m| m.key.as_str()).collect();
                    format!(
                        " Also unset (declaration order): {}.",
                        others.join(", ")
                    )
                } else {
                    String::new()
                };
                format!("{lead}.{desc_clause}{enum_clause}{rules_clause}{tail_clause}")
            }
            EngineError::MissingRequiredSection {
                entity_type,
                missing_count,
                sections,
                type_guidance,
            } => {
                let mut out = format!(
                    "missing {missing_count} required section(s) for type '{entity_type}':"
                );
                for s in sections {
                    let rules = if s.write_rules.is_empty() {
                        String::new()
                    } else {
                        format!(" — write_rules: {}", s.write_rules.join("; "))
                    };
                    out.push_str(&format!("\n  - '{}' ({}){rules}", s.key, s.heading));
                }
                if !type_guidance.is_empty() {
                    out.push_str("\nType guidance:");
                    for (etype, rules) in type_guidance {
                        if rules.is_empty() {
                            continue;
                        }
                        out.push_str(&format!(
                            "\n  - {etype}: {}",
                            rules.join("; ")
                        ));
                    }
                }
                out
            }
            EngineError::Validation(v) => v.prose_render(),
            // Variants whose `Display` already inlines every recovery
            // field — title invariants, hash mismatch (already explains
            // the stub case), unknown mem / type (already prints
            // declared list verbatim), cross-mem gates, stubs,
            // patch errors, etc. — fall back to `Display`. Logs and
            // tracing consumers see the same string.
            _ => self.to_string(),
        }
    }
}

/// Inline-render every [`ReferrerInfo`] without the truncation suffix
/// `format_inline_list_overflow` applies. Used by
/// [`EngineError::prose_render`]'s `HasIncomingRefs` /
/// `MemHasIncomingRefs` arms — the agent text channel inlines the
/// full list so recovery doesn't depend on the structured channel.
fn render_referrers_inline(referrers: &[ReferrerInfo]) -> String {
    referrers
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Format the `RequiredFieldUnset` message. The same typed code
/// fires from two semantically-distinct call sites:
///
/// * The create path constructs the variant when the caller didn't
///   supply a required metadata field. The pre-fix message ("cannot
///   unset required field …") was misleading because the field was
///   never set in the first place — `on_create: true` flips the
///   wording to "required metadata field … not provided".
/// * The update path constructs the variant when the caller passed
///   `metadata_unset: ["field"]` against a required field. The
///   pre-fix wording is correct for this path — `on_create: false`
///   keeps it.
///
/// Both paths share recovery (provide the field); the typed code
/// stays `REQUIRED_FIELD_UNSET` for code-key branching consumers.
fn _required_field_unset_msg(field: &str, entity_type: &str, on_create: bool) -> String {
    if on_create {
        format!(
            "required metadata field '{field}' not provided — type '{entity_type}' declares the field as required and has no default for it"
        )
    } else {
        format!("cannot unset required field '{field}' for type '{entity_type}'")
    }
}

/// Format the `HashMismatch` message. Stub-shaped entities have no
/// `content_hash` to compare against; rendering the empty `current:`
/// paren the way pre-fix code did misdirects an agent toward
/// hash-recovery via `memstead_entity` (which returns the same empty
/// hash). Surface the actual corrective action — pass
/// `expected_hash: ""` — instead.
fn _hash_mismatch_msg(id: &str, current: &str, is_stub: bool) -> String {
    if is_stub {
        format!(
            "hash mismatch for {id} — entity is a stub (no content_hash); pass expected_hash: \"\" to operate on stubs"
        )
    } else {
        format!(
            "hash mismatch for {id} — entity was modified concurrently (current: {current})"
        )
    }
}

/// Errors surfaced by [`Engine::from_workspace_root`] (basis) and its
/// pro counterpart (`memstead_git_branch::engine_from_workspace_root`).
///
/// The boot path layers three error sources: layout detection,
/// workspace-store load failures, per-mount backend instantiation
/// (folder + archive vs git-branch), and engine construction
/// (duplicate-mem checks). `#[from]` lifts the lower-layer types so
/// callers branch on a single error envelope.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// `detect_layout` returned [`crate::Layout::Empty`] — workspace
    /// root has no recognised layout marker. Operator runs
    /// `memstead mem-repo init` rather than booting against an empty
    /// directory.
    #[error("workspace at {0} is not initialised — run `memstead mem-repo init` first")]
    NotInitialised(PathBuf),
    /// Underlying [`crate::WorkspaceStoreAdapter`] load failed
    /// (missing config file, parse error, format-mismatch).
    #[error(transparent)]
    Store(#[from] crate::workspace_store::StoreError),
    /// Per-mount backend instantiation failed. Today: a mount
    /// declared `MountStorage::GitBranch` while the basis boot path
    /// only knows folder + archive.
    #[error(transparent)]
    Instantiate(#[from] crate::workspace_store::InstantiateError),
    /// Engine construction failed (duplicate mem names, etc.).
    #[error(transparent)]
    Engine(#[from] EngineError),
}

#[cfg(test)]
mod plan05_subsystem_tests {
    use super::*;

    /// A title-case body wiki-link refusal carries the
    /// slug-form retry under `proposed_slug` (mirroring `INVALID_TITLE`),
    /// so an agent that wrote `[[Idempotency]]` finds `idempotency` under
    /// the key it already knows.
    #[test]
    fn invalid_wiki_link_details_carry_proposed_slug_for_title_case() {
        let err = EngineError::InvalidWikiLinkTarget {
            raw: "Idempotency".to_string(),
            suggested: Some("idempotency".to_string()),
            section: "purpose".to_string(),
            link_source: "body_link".to_string(),
            reason: "slugs must be lowercase".to_string(),
        };
        let d = err.details();
        assert_eq!(d["proposed_slug"], "idempotency");
        assert_eq!(d["suggested"], "idempotency");
    }

    /// `SCHEMA_NOT_FOUND` carries the fixed-order resolution
    /// diagnostics under `details.sources`: a right-name/wrong-version
    /// pin shows the built-in's available versions with
    /// `pinned_version_match = false`, and `remote` is the reserved
    /// `not_configured` slot. This is the agent-visible payload that
    /// tells the caller the name resolves but the version does not.
    #[test]
    fn schema_not_found_details_carry_fixed_order_source_diagnostics() {
        let requested: semver::Version = "99.0.0".parse().unwrap();
        let sources = SchemaSourceDiagnostic::for_failed_pin("default", &requested, &[]);
        let err = EngineError::SchemaNotFound {
            mem: "specs".to_string(),
            pin: "default@99.0.0".to_string(),
            sources,
        };
        assert_eq!(err.code(), "SCHEMA_NOT_FOUND");
        let d = err.details();
        assert_eq!(d["mem"], "specs");
        assert_eq!(d["pin"], "default@99.0.0");
        let src = d["sources"].as_array().expect("sources is an array");
        let labels: Vec<&str> = src.iter().map(|s| s["source"].as_str().unwrap()).collect();
        assert_eq!(labels, ["local_storage", "builtin", "remote"]);
        // The `default` builtin exists at 1.0.0 — right name, wrong
        // version: builtin enumerates it but the pin does not match.
        let builtin = &src[1];
        assert!(
            builtin["versions_found"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == "1.0.0"),
            "builtin must enumerate default@1.0.0, got {builtin:?}",
        );
        assert_eq!(builtin["pinned_version_match"], false);
        // No local storage was consulted (empty `consulted` slice).
        assert_eq!(src[0]["versions_found"].as_array().unwrap().len(), 0);
        // Remote is the reserved, unenumerated slot.
        assert_eq!(src[2]["status"], "not_configured");
        assert!(
            src[2].get("versions_found").is_some(),
            "remote still ships an (empty) versions_found list",
        );
    }

    /// The ambiguous-grammar case suggests a
    /// colon-form (`mem:slug`), which is NOT a bare slug — it must not
    /// be promoted to `proposed_slug`.
    #[test]
    fn invalid_wiki_link_colon_form_suggestion_is_not_a_proposed_slug() {
        let err = EngineError::InvalidWikiLinkTarget {
            raw: "team/sub--thing".to_string(),
            suggested: Some("team/sub:thing".to_string()),
            section: "purpose".to_string(),
            link_source: "body_link".to_string(),
            reason: "ambiguous".to_string(),
        };
        let d = err.details();
        assert!(d["proposed_slug"].is_null(), "colon-form must not be a proposed_slug: {d}");
        assert_eq!(d["suggested"], "team/sub:thing");
    }

    /// A bad `--since` cursor is the typed `INVALID_CURSOR`
    /// code carrying the untruncated SHA in `details.since`.
    #[test]
    fn invalid_changes_cursor_code_and_details() {
        let sha = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let err = EngineError::InvalidChangesCursor {
            mem: "specs".to_string(),
            since: sha.to_string(),
        };
        assert_eq!(err.code(), "INVALID_CURSOR");
        let d = err.details();
        assert_eq!(d["mem"], "specs");
        assert_eq!(d["since"], sha, "the offending SHA must ride untruncated in details");
    }
}

#[cfg(test)]
mod inline_list_tests {
    use super::*;

    #[test]
    fn empty_list_renders_empty_string() {
        let items: Vec<String> = Vec::new();
        assert_eq!(format_inline_list_overflow(&items, "x"), "");
    }

    #[test]
    fn list_at_cap_renders_all_no_overflow_suffix() {
        let items = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(format_inline_list_overflow(&items, "x"), "a, b, c");
    }

    #[test]
    fn list_under_cap_renders_all_no_overflow_suffix() {
        let items = vec!["a".to_string(), "b".to_string()];
        assert_eq!(format_inline_list_overflow(&items, "x"), "a, b");
    }

    #[test]
    fn list_over_cap_appends_count_and_field_name() {
        let items: Vec<String> = (0..23).map(|i| format!("id{i}")).collect();
        let rendered = format_inline_list_overflow(&items, "referrers");
        assert_eq!(rendered, "id0, id1, id2 +20 more — see details.referrers");
    }

    #[test]
    fn list_six_items_truncates_to_three_plus_three() {
        let items: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
        let rendered = format_inline_list_overflow(&items, "missing");
        assert_eq!(rendered, "t0, t1, t2 +3 more — see details.missing");
    }

    #[test]
    fn has_incoming_refs_display_inlines_first_three_referrer_ids() {
        let referrers: Vec<ReferrerInfo> = (0..23)
            .map(|i| ReferrerInfo {
                from_id: format!("specs--ref{i}"),
                rel_types: vec!["USES".to_string()],
                mem: "specs".to_string(),
            })
            .collect();
        let err = EngineError::HasIncomingRefs {
            id: "specs--hub".to_string(),
            referrers,
        };
        let s = err.to_string();
        // First three ids appear inline; the rest are summarised plus a
        // pointer to `details.referrers` on the structured channel.
        assert!(s.contains("specs--ref0, specs--ref1, specs--ref2"), "got: {s}");
        assert!(s.contains("+20 more — see details.referrers"), "got: {s}");
        // Pre-fix the message only carried the count; check the count
        // still appears so callers parsing it for "N references" keep
        // working.
        assert!(s.contains("23 incoming reference"), "got: {s}");
    }

    #[test]
    fn wiki_link_without_relation_display_lists_all_when_under_cap() {
        let missing = vec![
            MissingWikiLink {
                section_key: "specifies".to_string(),
                target_id: "specs--a".to_string(),
            },
            MissingWikiLink {
                section_key: "specifies".to_string(),
                target_id: "specs--b".to_string(),
            },
            MissingWikiLink {
                section_key: "rationale".to_string(),
                target_id: "specs--c".to_string(),
            },
        ];
        let err = EngineError::WikiLinkWithoutRelation {
            from_id: "specs--src".to_string(),
            missing,
        };
        let s = err.to_string();
        assert!(s.contains("specifies→specs--a"), "got: {s}");
        assert!(s.contains("specifies→specs--b"), "got: {s}");
        assert!(s.contains("rationale→specs--c"), "got: {s}");
        assert!(!s.contains("more — see details"), "got: {s}");
    }

    #[test]
    fn wiki_link_without_relation_display_truncates_at_cap_with_pointer() {
        let missing: Vec<MissingWikiLink> = (0..6)
            .map(|i| MissingWikiLink {
                section_key: format!("s{i}"),
                target_id: format!("specs--t{i}"),
            })
            .collect();
        let err = EngineError::WikiLinkWithoutRelation {
            from_id: "specs--src".to_string(),
            missing,
        };
        let s = err.to_string();
        assert!(s.contains("s0→specs--t0, s1→specs--t1, s2→specs--t2"), "got: {s}");
        assert!(s.contains("+3 more — see details.missing"), "got: {s}");
    }

    #[test]
    fn relation_has_body_links_display_inlines_section_keys() {
        let err = EngineError::RelationHasBodyLinks {
            from_id: "specs--src".to_string(),
            to_id: "specs--dst".to_string(),
            rel_type: "USES".to_string(),
            body_links: vec!["specifies".to_string(), "rationale".to_string()],
        };
        let s = err.to_string();
        assert!(s.contains("specifies, rationale"), "got: {s}");
        assert!(!s.contains("more — see details"), "got: {s}");
    }

    // --- prose_render -----------------------------------------------
    // The text
    // channel inlines full recovery payloads (no `+N more — see
    // details.X` pointer). Display stays terse for logs; prose_render
    // is the rich method MCP / CLI surfaces call for `content[0].text`.

    #[test]
    fn prose_render_has_incoming_refs_inlines_every_referrer() {
        let referrers = (0..7)
            .map(|i| ReferrerInfo {
                from_id: format!("specs--r{i}"),
                rel_types: vec!["DEPENDS_ON".to_string()],
                mem: "specs".to_string(),
            })
            .collect();
        let err = EngineError::HasIncomingRefs {
            id: "specs--target".to_string(),
            referrers,
        };
        let prose = err.prose_render();
        for i in 0..7 {
            assert!(
                prose.contains(&format!("specs--r{i}")),
                "every referrer must appear inline; missing r{i} in: {prose}"
            );
        }
        assert!(!prose.contains("see details"), "got: {prose}");
        // Display stays terse with the overflow suffix.
        let display = err.to_string();
        assert!(display.contains("+4 more — see details.referrers"), "got: {display}");
    }

    #[test]
    fn prose_render_required_field_unset_inlines_field_description_and_rules() {
        // Update-path semantic: `on_create: false` → "cannot unset".
        let err = EngineError::RequiredFieldUnset {
            field: "verified_on".to_string(),
            entity_type: "requirement".to_string(),
            field_description: Some("ISO-8601 date the requirement was last validated".to_string()),
            enum_values: vec![],
            type_write_rules: vec!["bump verified_on on every status change".to_string()],
            on_create: false,
            missing: Vec::new(),
        };
        let prose = err.prose_render();
        assert!(prose.contains("ISO-8601 date"), "field_description missing: {prose}");
        assert!(prose.contains("bump verified_on"), "type_write_rules missing: {prose}");
        assert!(!prose.contains("see details"), "got: {prose}");
        assert!(
            prose.contains("cannot unset"),
            "update-path wording must say 'cannot unset': {prose}"
        );
    }

    /// Create
    /// path renders "not provided" instead of "cannot unset" — the
    /// pre-fix wording was misleading on a path where nothing was
    /// ever set in the first place.
    #[test]
    fn prose_render_required_field_unset_create_path_uses_not_provided_wording() {
        let err = EngineError::RequiredFieldUnset {
            field: "verified_on".to_string(),
            entity_type: "requirement".to_string(),
            field_description: Some("ISO-8601 date the requirement was last validated".to_string()),
            enum_values: vec![],
            type_write_rules: vec![],
            on_create: true,
            missing: Vec::new(),
        };
        let prose = err.prose_render();
        assert!(
            prose.contains("not provided"),
            "create-path wording must say 'not provided': {prose}"
        );
        assert!(
            !prose.contains("cannot unset"),
            "create-path wording must NOT say 'cannot unset': {prose}"
        );
        // Same Display dispatch — `to_string()` mirrors `prose_render`'s
        // create-path lead.
        let display = err.to_string();
        assert!(display.contains("not provided"), "Display must match: {display}");
        assert!(!display.contains("cannot unset"), "Display must match: {display}");
    }

    /// The
    /// create-path multi-field accumulator surfaces every required-
    /// no-default field unset in `details.missing[]`. Each entry
    /// carries `{field, description, enum_values, write_rules}` so
    /// the agent fixes the whole set in one retry. The singular
    /// `details.field` echoes `missing[0].field` for back-compat.
    #[test]
    fn details_required_field_unset_multi_field_envelope_shape() {
        use crate::runtime_validator::MissingRequiredField;
        let err = EngineError::RequiredFieldUnset {
            field: "decided_on".to_string(),
            entity_type: "decision".to_string(),
            field_description: Some(
                "Date the decision was accepted. ISO YYYY-MM-DD.".to_string(),
            ),
            enum_values: vec![],
            type_write_rules: vec!["status transitions: proposed → accepted".to_string()],
            on_create: true,
            missing: vec![
                MissingRequiredField {
                    entity_type: "decision".to_string(),
                    key: "decided_on".to_string(),
                    description: "Date the decision was accepted. ISO YYYY-MM-DD.".to_string(),
                    enum_values: vec![],
                },
                MissingRequiredField {
                    entity_type: "decision".to_string(),
                    key: "deciders".to_string(),
                    description: "Who made the call. Comma-separated handles.".to_string(),
                    enum_values: vec![],
                },
            ],
        };
        let details = err.details();
        // Back-compat: singular `field` echoes the first-missing entry.
        assert_eq!(details["field"].as_str(), Some("decided_on"));
        // Multi-field accumulator surfaces every entry in
        // declaration order.
        let missing = details["missing"].as_array().expect("missing[] array");
        assert_eq!(missing.len(), 2);
        assert_eq!(missing[0]["field"].as_str(), Some("decided_on"));
        assert_eq!(missing[1]["field"].as_str(), Some("deciders"));
        // First entry's `field` agrees with the singular shape.
        assert_eq!(details["field"], missing[0]["field"]);
        // Per-entry `write_rules` echoes the type-level rules for
        // self-containment.
        assert_eq!(missing[0]["write_rules"], details["type_write_rules"]);
        // Prose mentions both field names so the agent reading the
        // text channel sees the whole set without crossing into the
        // structured channel.
        let prose = err.prose_render();
        assert!(prose.contains("decided_on"), "got: {prose}");
        assert!(prose.contains("deciders"), "got: {prose}");
    }

    /// The unset path's singular shape is
    /// preserved — `missing[]` is empty (the user targeted one field
    /// by definition); the singular fields above are authoritative.
    /// The typed code stays `REQUIRED_FIELD_UNSET`.
    #[test]
    fn details_required_field_unset_singular_shape_for_unset_path() {
        let err = EngineError::RequiredFieldUnset {
            field: "decided_on".to_string(),
            entity_type: "decision".to_string(),
            field_description: Some("…".to_string()),
            enum_values: vec![],
            type_write_rules: vec![],
            on_create: false,
            missing: Vec::new(),
        };
        let details = err.details();
        assert_eq!(details["field"].as_str(), Some("decided_on"));
        let missing = details["missing"].as_array().expect("missing[] array present");
        assert!(missing.is_empty(), "unset-path missing[] must be empty");
        assert_eq!(err.code(), "REQUIRED_FIELD_UNSET");
    }

    #[test]
    fn prose_render_missing_required_section_enumerates_each_section_with_write_rules() {
        use crate::runtime_validator::MissingRequiredSection;
        let sections = vec![
            MissingRequiredSection {
                entity_type: "spec".to_string(),
                key: "purpose".to_string(),
                heading: "Purpose".to_string(),
                write_rules: vec!["one-sentence statement of intent".to_string()],
            },
            MissingRequiredSection {
                entity_type: "spec".to_string(),
                key: "scope".to_string(),
                heading: "Scope".to_string(),
                write_rules: vec!["what is in and out of scope".to_string()],
            },
        ];
        let mut type_guidance: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        type_guidance.insert(
            "spec".to_string(),
            vec!["specs are immutable once stable".to_string()],
        );
        let err = EngineError::MissingRequiredSection {
            entity_type: "spec".to_string(),
            missing_count: 2,
            sections,
            type_guidance,
        };
        let prose = err.prose_render();
        assert!(prose.contains("purpose"), "got: {prose}");
        assert!(prose.contains("scope"), "got: {prose}");
        assert!(prose.contains("one-sentence statement of intent"), "got: {prose}");
        assert!(prose.contains("specs are immutable once stable"), "got: {prose}");
        assert!(!prose.contains("see details"), "got: {prose}");
    }

    #[test]
    fn prose_render_relationship_cycle_inlines_existing_path() {
        use crate::entity::EntityId;
        let path = vec![
            EntityId::canonical("specs--a"),
            EntityId::canonical("specs--b"),
            EntityId::canonical("specs--c"),
            EntityId::canonical("specs--a"),
        ];
        let err = EngineError::RelationshipCycle {
            rel_type: "PART_OF".to_string(),
            from: EntityId::canonical("specs--a"),
            to: EntityId::canonical("specs--c"),
            existing_path: path,
            path_truncated: false,
        };
        let prose = err.prose_render();
        assert!(prose.contains("specs--a → specs--b → specs--c → specs--a"), "got: {prose}");
        assert!(!prose.contains("see details"), "got: {prose}");
    }

    #[test]
    fn prose_render_falls_back_to_display_for_trivial_variants() {
        // ReadOnlyMount has no list payload — Display already inlines
        // the recovery context.
        let err = EngineError::ReadOnlyMount("archive-2024".to_string());
        assert_eq!(err.prose_render(), err.to_string());
    }
}
