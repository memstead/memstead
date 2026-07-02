//! Operation request/response types and the gix-free read paths
//! (`health`, `search`).
//!
//! Per-entity delta envelopes for `memstead_changes_since` live in
//! [`changes`] — backend-neutral so both the git-branch tree-diff
//! and any future folder-backend JSONL-walk produce the same shape.
//! Wire types for the agent-notes payload live in [`agent_notes`] —
//! pure data shapes, no gix. The producer functions
//! (`agent_notes_since`, `read_memstead_ref`) stay in
//! `memstead-git-branch::ops::agent_notes` because they read from a
//! gitdir.
//! The git-touching operation submodules (`crud`, `export`) still
//! live in `memstead-git-branch` and are re-exported into
//! `memstead_git_branch::ops` for downstream callers.

pub mod agent_notes;
pub mod branch_reset;
pub mod changes;
pub mod commit_envelope;
pub mod diff;
pub mod export;
pub mod transport;
pub mod health;
pub mod integrity;
#[cfg(not(target_arch = "wasm32"))]
pub mod search;

pub use agent_notes::{AgentNotesReport, CommitNote};
pub use commit_envelope::{CommitEnvelope, EntityChange};
pub use diff::{Diff, DiffConfig, EntityDiff, IncomingRipple};
pub use branch_reset::BranchResetOutcome;
pub use export::{MemExportBytes, MemExportError};
pub use transport::{FetchOutcome, PullOutcome, PushOutcome, UpdatedRef};
pub use changes::{
    BackendChanges, ChangeEnvelope, ChangesReport, EMPTY_TREE_SHA, NoticeByChange,
    NoticeChanges, RENAME_SIMILARITY_DEFAULT, RENAME_SIMILARITY_MAX, RENAME_SIMILARITY_MIN,
    MemChangedNotice, folder_changes_since,
};

use crate::entity::EntityId;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use std::collections::HashMap;
use std::fmt;

/// Allowed `include` keys for `memstead_overview` — single source of
/// truth shared across the basis MCP server, pro MCP server, and the
/// basis CLI's `overview` command. Mirrors `HEALTH_INCLUDE_KEYS` for
/// the `health` surface. The CLI `--include` flag validates against
/// this list and surfaces `UNKNOWN_INCLUDE_KEY` warnings, matching the
/// MCP tool's behaviour.
pub const OVERVIEW_INCLUDE_KEYS: &[&str] = &[
    "community_members",
    "community_bridges",
    "mem_distribution",
    "dangling_links",
];

// The unknown-filter warning prose lives in the `Display` impl of the
// typed `WarningHint::UnknownFilterKey` / `WarningHint::UnknownRangeFilterField`
// variants below. These helpers are shared with that Display impl. They
// are pure string formatting with no search/tantivy dependency, so they
// live here (not in the wasm-gated `search` module) and stay available
// on `wasm32`.

/// Render the type-list clause as quoted items only — `"'X'"` for one
/// declarer, `"'X', 'Y'"` for many — without a leading "type" /
/// "types" word. Caller composes the leading word via
/// [`type_word_for`] so prose contexts like `"of types ..."` don't
/// produce the duplicate-word output `"of types types '...'"`.
pub(crate) fn format_types_clause(types: &[String]) -> String {
    types
        .iter()
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Leading word to pair with [`format_types_clause`]: `"type"` for a
/// single declarer, `"types"` for many. Empty slice maps to `"types"`
/// (callers should not invoke this for an empty list; the typed-
/// warning sites guard the `is_empty` case already).
pub(crate) fn type_word_for(types: &[String]) -> &'static str {
    if types.len() == 1 { "type" } else { "types" }
}

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
    /// Relationships to create: `[{ to: EntityId, type: "USES" }]`.
    pub relations: Vec<RelateArg>,
    /// When true, validate and compute the result but do not write to
    /// disk, mutate the store, create edges, or commit. Response carries
    /// the prospective `id`, `file_path`, `content_hash`, and any
    /// `warnings` — `commit_sha` is empty.
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
    /// Per-mem commit SHA produced by this mutation. Agents remember it
    /// and feed it to `memstead_changes_since` to pick up incremental updates.
    /// Empty for dry runs (no commit happens).
    #[serde(default)]
    pub commit_sha: String,
    /// Typed non-fatal issues — same shape as `CreateResult::warnings`.
    /// Pre-Bug-4 this was `Vec<String>` and unused; now carries
    /// `WarningHint` so e.g. `INLINE_WIKI_LINK_AUTO_STUBBED` from update
    /// flows out via the same `{code, message, details}` envelope agents
    /// already branch on for create-time warnings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// Typed non-fatal issue surfaced from engine operations. Serialises as the
/// uniform `{ code, message, details }` envelope so a generic warning handler
/// (log sink, UI, alerting) can read `code` + `message` without branching on
/// variant. `Display` renders the agent-facing text, reachable via
/// [`WarningHint::message`]; per-variant structured fields land under
/// `details`, their shape keyed by `code`.
///
/// Shared across `CreateResult`, `RelateResult`, and `HealthSummary`. New
/// variants are additive; they widen the enum rather than fork a per-site
/// type so wire-level warning consumers keep a single discriminated union
/// to branch on. The wire shape matches what [`envelope`] produces for the
/// MCP error channel, so one decoder handles both surfaces.
#[derive(Debug, Clone)]
pub enum WarningHint {
    /// A required section was empty or missing at create time. Carries
    /// the type and section keys plus the section's own `write_rules`
    /// so the agent can self-correct with a follow-up `memstead_update`.
    /// Type-level `write_rules` no longer ride per warning — they
    /// ship once at the mutation-response top level on
    /// `type_guidance` keyed by `entity_type` (F9). Decoders look up
    /// the guidance via `entity_type` against the top-level map.
    MissingRequiredSection {
        entity_type: String,
        key: String,
        heading: String,
        write_rules: Vec<String>,
    },
    /// A required metadata field was not supplied at create time and the
    /// schema does not auto-fill the value (no `default_value`, no
    /// `init_timestamp`, no `auto_timestamp`). The entity still lands —
    /// the generator may write an empty / today's-date placeholder into
    /// the frontmatter — but the warning surfaces the gap so the agent
    /// follows up via `memstead_update` rather than leaving the entity in a
    /// stuck state. Payload mirrors [`Self::MissingRequiredSection`] in
    /// shape so a single decoder handles both. Wire-equivalent shape
    /// with `EngineError::RequiredFieldUnset`'s `details` payload, since
    /// the recovery path is the same (read the description / allowed
    /// enum values from the envelope rather than re-fetching the
    /// schema).
    MissingRequiredField {
        entity_type: String,
        key: String,
        description: String,
        enum_values: Vec<String>,
    },
    /// An undeclared relationship was admitted because the mem's schema
    /// is in open mode. The caller can still suggest the name be added to
    /// the schema vocabulary.
    UndeclaredRelationshipOpen {
        rel_type: String,
        message: String,
    },
    /// `memstead_relate` was asked to add an edge that already exists. The
    /// op is a successful no-op — the warning surfaces what would otherwise
    /// be silent so an agent relying on `renames` / side-effects can notice
    /// the call didn't change the graph.
    DuplicateRelationship {
        rel_type: String,
        from: EntityId,
        to: EntityId,
    },
    /// `memstead_relate` with `remove: true` was asked to drop an edge that
    /// wasn't present. Successful no-op, surfaced so an agent operating on
    /// a stale mental model sees the mismatch.
    NoSuchRelationship {
        rel_type: String,
        from: EntityId,
        to: EntityId,
    },
    /// An `include` key passed to `memstead_health` was outside the accepted
    /// set. The key is ignored; the allowed list is echoed back verbatim so
    /// an agent with a typo can correct on the next call without opening a
    /// schema doc.
    UnknownIncludeKey {
        key: String,
        allowed: Vec<String>,
    },
    /// A paged/bounded parameter exceeded its cap. The cap is authoritative
    /// so the op still ran, but the warning surfaces what the caller
    /// requested vs. what was served.
    LimitClamped {
        requested: usize,
        actual: usize,
    },
    /// `memstead_rename` was asked to change the title but normalisation
    /// (lowercase, diacritic-folding, punctuation-strip, hyphen-collapse)
    /// mapped the requested title to the existing slug — so the id is
    /// unchanged and nothing is written to disk. Surfaced so autonomous
    /// skills don't mistake the silent short-circuit for a successful
    /// cosmetic rewrite.
    TitleNormalizedToSlugNoop {
        requested_title: String,
        current_slug: String,
    },
    /// `memstead_update` produced a post-mutation entity whose regenerated
    /// markdown is bytes-identical to the on-disk content — no field,
    /// section, metadata value, relation, or auto-timestamp actually
    /// changed. The op is a successful no-op: no disk write, no
    /// commit, `content_hash` unchanged. Surfaced so autonomous skills
    /// branching on `commit_sha != ""` see an explicit signal, and
    /// `expected_hash`-based polling stays stable across the no-op.
    /// Mirrors `TitleNormalizedToSlugNoop` for the rename surface.
    UpdateNoop { id: EntityId },
    /// `memstead_search` was called with both `stub=true` and `entity_type`
    /// set. Stubs carry no `entity_type` (they are ID-only placeholders),
    /// so the combined filter excludes every stub — the call is an empty
    /// set by construction. Surfaced so an agent doesn't interpret the
    /// empty result as "no stubs of this type exist" when in fact no
    /// stub can ever satisfy the filter. Drop `entity_type` to list stubs.
    StubFilterExcludesAll {
        entity_type: String,
    },
    /// `memstead_search(filters: {<key>: ...})` named a filter key that the
    /// queried type does not declare. The wire `code()` discriminates
    /// the two outcomes, so a consumer branches on `code` alone:
    /// - `declared_on_other_types` **empty** → no reachable schema
    ///   declares the key → `UNKNOWN_FILTER_KEY`; the filter is truly
    ///   ignored and the result set equals the same search without it.
    /// - `declared_on_other_types` **non-empty** → the key is declared
    ///   on other type(s) and the filter was applied with strict
    ///   type-narrowing (result restricted to the declaring type(s), or
    ///   emptied when the call scoped to a non-declaring type) →
    ///   `FILTER_TYPE_SCOPED`.
    /// `declared_on_other_types` stays on the wire as enrichment, not as
    /// the disambiguator.
    UnknownFilterKey {
        key: String,
        /// `entity_type` the search call scoped to (`None` for an
        /// unscoped call).
        scoped_type: Option<String>,
        /// Types where the filter IS declared, sorted alphabetically.
        /// Empty when no reachable schema declares the key at all.
        declared_on_other_types: Vec<String>,
    },
    /// `memstead_search(filters: {<field>: ...})` named a field that the
    /// schema declares but with `filterable: none` — the filter is
    /// ignored, the hit set is unconstrained by it.
    FieldNotFilterable {
        field: String,
    },
    /// `memstead_search(filters: {<csv-field>: "a,b"})` passed a comma-bearing
    /// value to a csv-array field. csv fields match a *single* member, so
    /// the whole rendered value (e.g. the `tags: dedup,retry` an entity
    /// displays) can never equal any one member — the filter matches
    /// nothing. Surfaced so an agent that copied the rendered value gets a
    /// recoverable signal (split into repeated single-member filters)
    /// rather than an empty result indistinguishable from a true
    /// no-match. The filter still applies as written (matches nothing);
    /// this only adds the advisory.
    FilterValueMultiMember {
        key: String,
        value: String,
    },
    /// `memstead_search(filters: {<field>: <value>})` passed a value the
    /// schema field constrains with an `enum_values` allow-list, but the
    /// value (or, for a csv-array field, one of its comma members) is not a
    /// member. The filter still applies as written and matches nothing for
    /// that value, so an empty result is otherwise indistinguishable from a
    /// true no-match — this surfaces the typo plus the allowed values so an
    /// agent corrects without opening the schema. Reuses the
    /// `INVALID_ENUM_VALUE` code from the mutation surface.
    FilterValueNotInEnum {
        key: String,
        value: String,
        allowed: Vec<String>,
    },
    /// `memstead_search(related_to: <id>)` reached a neighbourhood larger
    /// than the cap. The results were ranked by proximity (nearer first)
    /// and bounded to the nearest `kept` of `total` reachable entities so a
    /// hub can't flood the caller. Surfaced so the agent knows the
    /// neighbourhood was truncated — narrow with `depth`/filters for more.
    NeighbourhoodCapped {
        kept: usize,
        total: usize,
    },
    /// `memstead_search` trimmed the returned page to fit the token budget.
    /// The highest-ranked `kept` hits that fit under `budget` are returned;
    /// the rest of the page is dropped so the response stays under the MCP
    /// transport cap. `_total` still reflects the full match count — page the
    /// remainder with `offset`, narrow the query, or raise `token_budget`.
    SearchResultsTruncated {
        kept: usize,
        budget: usize,
    },
    /// `memstead_search(range_filters: {<key>: ...})` named a key that
    /// doesn't follow the `min_<field>` / `max_<field>` / `<field>_before`
    /// / `<field>_after` grammar. The key is ignored.
    RangeFilterKeyMalformed {
        key: String,
    },
    /// `memstead_search(range_filters: {<key>: ...})` named a range-filter
    /// key whose underlying field the queried type does not declare.
    /// Same shape and same one-code-per-outcome split as
    /// [`Self::UnknownFilterKey`]: `code()` is `UNKNOWN_RANGE_FILTER_FIELD`
    /// when `declared_on_other_types` is empty (truly ignored, result =
    /// unfiltered) and `RANGE_FILTER_TYPE_SCOPED` when non-empty (applied
    /// with strict type-narrowing). Includes the literal `key` (the
    /// prefixed/suffixed form the caller sent) alongside the bare `field`.
    UnknownRangeFilterField {
        field: String,
        /// The literal filter key the caller sent, e.g. `min_count`.
        key: String,
        scoped_type: Option<String>,
        declared_on_other_types: Vec<String>,
    },
    /// `memstead_search(range_filters: {<field>: ...})` named a field that
    /// the schema declares but with a filterability other than `range`.
    /// The range filter is ignored.
    FieldNotRangeFilterable {
        field: String,
    },
    /// `memstead_search` could not query a target mem's search index —
    /// either the mem has no index yet (`reason: "missing_index"`)
    /// or a tantivy execution failure surfaced (`reason:
    /// "query_failed"` plus the error string).
    SearchMemIndexUnavailable {
        mem: String,
        /// Discriminator: `"missing_index"` or `"query_failed"`.
        reason: &'static str,
        /// The underlying error string when `reason == "query_failed"`;
        /// `None` for `"missing_index"`.
        error: Option<String>,
    },
    // There is deliberately no `RenameSimilarityClamped` variant:
    // out-of-range `rename_similarity` hard-refuses
    // (`EngineError::RenameSimilarityOutOfRange` → typed
    // `INVALID_INPUT`) rather than clamping, so the warning channel has
    // no story to tell and the typed-warning vocabulary tracks the live
    // wire shape.
    /// `memstead_create` (or `memstead_rename`) received a `title` with leading
    /// or trailing whitespace. The engine silently strips the surround
    /// before slug derivation and storage; the warning records what the
    /// caller sent vs. what landed so the audit trail can spot the
    /// drift. Internal whitespace (between words) is preserved
    /// untouched. Fully-whitespace titles are still refused at the
    /// validator boundary (those collapse to empty).
    TitleTrimmed {
        original: String,
        trimmed: String,
    },
    /// An inline wiki-link resolved to an ID of the form
    /// `<current-mem>--<other-known-mem-suffix>--<slug>`. This is
    /// almost always drift from a mem-rename — the author wrote
    /// `[[plugin--slug]]` expecting `plugin` to be the mem prefix, but
    /// the current mem is `test-mem-plugin`, so the literal
    /// resolution nests the prefix. Detection only — the load path still
    /// creates the stub (no silent rewrite). Fix via `memstead_update
    /// patch_sections` to either the bare slug or the fully-qualified ID.
    /// Emitted at load / reload / attach time and carried through
    /// `HealthSummary.warnings`; mutation paths never emit this warning
    /// to avoid noise on every edit.
    SuspiciousNestedPrefix {
        from: EntityId,
        resolved_id: EntityId,
        /// Stripped-and-resolved candidate via the two-pass resolver
        /// (cross-mem lookup first, bare-slug fallback second). `None`
        /// when no real entity was found — the author must disambiguate.
        candidate_target: Option<EntityId>,
        section: String,
    },
    /// Inline `[[wiki-link]]` syntax in entity section bodies parsed to
    /// targets that did not yet resolve, so the engine auto-created stub
    /// entities for them. A common authoring hazard: an agent illustrating
    /// link syntax in prose (`[[example:slug]]`) inadvertently creates
    /// ghost stubs and a REFERENCES edge from the prose entity to each.
    /// Surfaced so the agent reviews the list and either replaces the
    /// inline literal with a fenced/quoted form or removes the entity if
    /// the stub was not intended. Carries the source entity id (`from`)
    /// and every newly-stubbed `target` id created by THIS call.
    InlineWikiLinkAutoStubbed {
        from: EntityId,
        stubs: Vec<EntityId>,
    },
    /// A body wiki-link resolved to the entity's own id, so the
    /// alias-synthesis pass dropped the would-be self-referential edge
    /// (F11) — a self-edge carries no navigational value and would render
    /// as both an Outgoing and an Incoming neighbour of itself. The
    /// create/update still succeeds (the author may have written their
    /// own slug); this warns so the dropped link is observable, matching
    /// the alias pass's other side-effect warnings (`AUTO_STUB_CREATED` /
    /// `INLINE_WIKI_LINK_AUTO_STUBBED`).
    SelfLinkIgnored {
        id: EntityId,
    },
    /// `memstead_relate` to a cross-mem target whose mem is not (yet)
    /// mounted in the workspace. The cross-mem link policy permits
    /// the edge, so the engine auto-stubs the target as a forward
    /// reference — but with the target mem entirely absent from
    /// `writable_mems()`, the stub has no `_mem_schema` resolution
    /// and any later read sees an indeterminate-schema entity. The
    /// warning makes the missing-mem state visible so an operator
    /// can distinguish a typo (intended `B` but typed `b`) from a
    /// deliberate forward reference that expects the mem to be
    /// created later. (F4)
    CrossMemTargetMemUncreated {
        from_mem: String,
        to_mem: String,
        target_id: EntityId,
    },
    /// A mutation landed without a `note` field while the workspace
    /// config's `[mutations].require_notes = true` — provenance is
    /// best-effort, so the engine completes the commit but flags the
    /// absence so autonomous skills can audit their coverage. The
    /// mutation still writes to disk and produces a commit; this warning
    /// exists purely to surface the missed opportunity for a human- /
    /// agent-readable body line. `tool` carries the MCP tool name
    /// (`memstead_create`, `memstead_update`, …) so consumers can attribute the
    /// gap without re-deriving it from the response context.
    NoteMissing { tool: String },
    /// A create supplied a value for an auto-managed metadata field
    /// (`init_timestamp` like `created_date`, or `auto_timestamp` like
    /// `last_modified`); the engine owns those values, so the supplied
    /// one was discarded and the engine value stamped instead. The
    /// entity still lands — this warning closes the silent-drop gap so
    /// the agent learns its input had no effect without a follow-up
    /// read. `field` names the discarded key; `supplied` echoes the
    /// rejected value. (The `memstead_update` path refuses the same keys
    /// outright with `READ_ONLY_FIELD`; create's posture is
    /// stamp-and-proceed, so it warns rather than refusing.)
    IgnoredReadonlyField { field: String, supplied: String },
    /// The workspace is embedded inside another git repository
    /// (`outer_repo_root`) whose `.gitignore` does not list
    /// `mem-repo/`. Without that ignore line, the outer repo would
    /// either swallow `mem-repo-git` as a nested untracked tree or
    /// (worse) record it as a submodule via gitlink — both shapes
    /// silently corrupt the mem-repo identity.
    ///
    /// Surfaced from `memstead_health` so the agent / operator can fix
    /// the outer repo's `.gitignore` (or pass `--no-gitignore` at
    /// `memstead mem-repo init`/`migrate-from-disk` time and accept the
    /// risk explicitly).
    OuterRepoNotIgnoringMemRepo {
        outer_repo_root: String,
        workspace_root: String,
    },
    /// One or more `required_outgoing` blocks on the entity's type are
    /// not yet satisfied by its post-application outgoing edges. Tier-2
    /// — the create/update lands; the warning surfaces every unsatisfied
    /// block in a single payload so the agent can emit one batched
    /// `memstead_relate` follow-up.
    MissingRequiredOutgoing {
        entity_type: String,
        entity_id: EntityId,
        /// Each entry mirrors one unsatisfied `RequiredOutgoing` block:
        /// the alternative relationship names plus the rendered
        /// cardinality literal (`"at_least_one"`).
        missing: Vec<MissingRequiredOutgoingBlock>,
    },
    /// A markdown file declared the same `## <Heading>` twice or more for a
    /// schema-declared section key. The parser keeps the first occurrence's
    /// body and drops the rest — the duplicate headers and their bodies are
    /// removed from the storage value, so the next read-modify-write cycle
    /// emits a single heading. Surfaced so the operator (or the next ingest
    /// cycle) sees that content was discarded; common cause is an agent
    /// appending a section instead of replacing it.
    ///
    /// Emitted at load / reload / attach time only; mutation paths do not
    /// re-parse the just-written file.
    DuplicateSectionHeading {
        entity_id: EntityId,
        section_key: String,
        heading: String,
        occurrences: usize,
    },
    /// The engine detected that a sibling writer (another `Engine`
    /// instance, an out-of-band `git pull`, etc.) advanced the on-disk
    /// HEAD of `mem` past the engine's cached `last_known_head`, so
    /// the engine reloaded that mem's slice of the in-memory store
    /// before serving the current call. The response carries fresh
    /// content; the warning explains why state shifted under the
    /// caller. Agents that need the per-entity diff call
    /// `memstead_changes_since` with the supplied `old_head`.
    MemReloaded {
        mem: String,
        old_head: String,
        new_head: String,
        entities_loaded: usize,
    },
    /// `memstead_relate` add path landed on a not-yet-real target id and
    /// the engine materialised a stub at that id (in-memory upsert; the
    /// file lands when a follow-up `memstead_create` promotes the stub).
    /// Pre-fix surfaced through a top-level `stub_warning: Option<String>`
    /// field on the relate response — agents iterating `warnings[]` to
    /// surface non-fatal findings silently skipped the auto-stub case.
    /// Carries the materialised stub id so the agent can pin a
    /// follow-up `memstead_create` (or `memstead_relate remove=true` to drop
    /// the edge before authoring).
    AutoStubCreated { stub_id: EntityId },
    /// A relation parsed from an entity's `## Relationships` section
    /// at load time failed validation against the source mem's
    /// schema (or wiki-link grammar). The entity itself loads
    /// normally; the offending relation is dropped from the
    /// in-memory store. `reason` discriminates:
    /// - `unknown_rel_type` — the rel-type is not declared in the
    ///   source mem's schema and the schema is in `strict` mode.
    /// - `shape` — the `(source_type, target_type)` pair is not
    ///   allowed by the rel-type's `source_types` / `target_types`.
    /// - `cycle` — adding this relation would close a cycle in an
    ///   acyclic-declared subgraph (emitted by the post-load
    ///   second-pass cycle check; not yet implemented).
    ///
    /// Hand-edits, external tooling, and the macOS app's editor
    /// surface can inject relations that bypass `memstead_relate`; the
    /// parse-path validation catches those. Mutation-path writes
    /// pre-validated by the engine never trip this warning.
    ///
    /// `origin` discriminates the source mount's capability:
    /// `"writable"` (the operator can fix the source markdown via
    /// `memstead_update` / `memstead_relate` and re-run) or `"readonly"`
    /// (the source mem is mounted read-only — purely diagnostic,
    /// the operator either uninstalls the archive or accepts the
    /// dropped relation).
    ///
    /// `recovery` carries an abstract-action payload sufficient to
    /// reverse the drop without consulting another response. `Some`
    /// when `origin == "writable"` — the engine can rewrite the
    /// source markdown via the mutation surface, so a consumer (an
    /// agent walking `memstead_health`, a bulk-fix orchestrator, the
    /// macOS app's drift panel) maps `kind` to the concrete call on
    /// whichever MCP / CLI / UniFFI surface it uses. `None` when
    /// `origin == "readonly"` — the source markdown is not reachable
    /// via the engine, so no abstract action exists; the warning's
    /// message names the operator-level path (uninstall the archive
    /// or accept the drop).
    ParsedRelationInvalid {
        entity_id: EntityId,
        rel_type: String,
        target: EntityId,
        reason: String,
        origin: String,
        recovery: Option<ParsedRelationRecovery>,
    },
    /// `memstead_delete` (or `memstead_rename`, when implemented) on a
    /// Write-Mem entity that had **no** Write-Mem referrers but
    /// **does** have ReadOnly-mount referrers. The on-disk file is
    /// removed and committed; the in-memory entity is demoted to a
    /// stub at the same id so the surviving incoming edges from the
    /// ReadOnly mount(s) keep a valid target. The agent sees
    /// `memstead_entity <id>` returning a stub immediately and not
    /// stale data after a server reload — fresh boot from disk
    /// reconstructs the same stub via the parser's auto-stub-on-
    /// unresolved-link path. `referrers` carries the surviving
    /// ReadOnly source ids so the agent can either accept the stub
    /// or uninstall the archive.
    ResidualStubForReadOnlyReferrers {
        id: EntityId,
        referrers: Vec<EntityId>,
    },
    /// `memstead_mem_delete` was called with `delete_files: true` but
    /// at least one part of the symmetric cleanup did not complete.
    /// The mem is already unregistered from the router; this
    /// warning surfaces what survived so an agent reading
    /// `files_deleted: false` doesn't trigger redundant cleanup or
    /// blame the wrong layer. `reason` discriminates:
    /// - `rmdir_failed` — folder-backed mem directory survived
    ///   `remove_dir_all` (filesystem permission, busy handle, …).
    ///   `path` names the directory; `error` carries the OS-level
    ///   diagnostic.
    /// - `backend_prune_failed` — git-branch backend rejected the
    ///   ref-edit transaction that prunes
    ///   `refs/heads/<branch_leaf>` + `__MEMSTEAD:mems/.../config.json`
    ///   (gitdir IO, concurrent writer racing the ref). `path` is
    ///   `None`; `error` carries the wrapped backend message.
    /// One emission per failed step — both can land in the same
    /// response when a folder mount somehow has both an rmdir
    /// failure and a backend cleanup failure (rare; the folder
    /// backend's `delete_artifacts` is a no-op default).
    MemFilesNotDeleted {
        mem: String,
        reason: String,
        path: Option<String>,
        error: Option<String>,
    },
    /// `memstead mem init` detected a pre-existing branch + config
    /// blob carrying the `unregistered_at` tombstone marker that
    /// `memstead mem unregister` writes — the operator's deliberate
    /// "preserve for re-attach" signal. The create path adopted the
    /// residual entities, cleared the tombstone, and registered the
    /// branch as a writable mount. Audit visibility for the
    /// reattach so an agent reading the warnings sees what shape
    /// the new mount took. `unregistered_at` carries the ISO-8601
    /// timestamp the tombstone recorded so the operator can correlate
    /// the reattach with a prior unregister event.
    MemReattachedAfterUnregister {
        mem: String,
        unregistered_at: String,
    },
    /// A `## Relationships` row was followed by trailing content that
    /// did not match the canonical em-dash delimiter (` — `, U+2014
    /// framed by spaces) — ASCII `--`, ASCII `-`, en-dash U+2013, or
    /// minus U+2212. The relation parses with `description: None`;
    /// the trailing content is NOT preserved on the in-memory
    /// `Relationship`, so the next render of this entity normalises
    /// the row to the simple form `- **TYPE**: [[X]]`. The warning is
    /// the operator's signal that content was dropped — restore the
    /// description with an explicit em-dash if it should round-trip.
    /// Emitted at parse time (load / reload / attach); mutation paths
    /// never trip it because they go through the typed `description`
    /// parameter rather than markdown text.
    AmbiguousDescriptionDelimiter {
        from: EntityId,
        rel_type: String,
        target: EntityId,
        /// Literal trailing content captured between `]]` and end of
        /// line — surfaced verbatim so the operator can paste the
        /// intended text back in with a canonical delimiter.
        trailing: String,
    },
    /// Parse-time variant of [`crate::EngineError::MissingRequiredDescription`].
    /// A hand-edited `## Relationships` row used a rel-type whose
    /// schema declares `per_edge_description: required` without a
    /// trailing description. The relation still loads (the engine
    /// does not block the file from booting), but the warning
    /// surfaces the gap so the operator follows up with `memstead_update`
    /// / `memstead_relate` to author the missing description.
    ParseMissingRequiredDescription {
        from: EntityId,
        rel_type: String,
        target: EntityId,
    },
    /// Parse-time variant of [`crate::EngineError::DescriptionNotPermitted`].
    /// A hand-edited `## Relationships` row used a rel-type whose
    /// schema declares `per_edge_description: forbidden` together
    /// with a trailing em-dash description. The relation still loads
    /// (the engine does not block the file from booting); the
    /// description is dropped from the in-memory `Relationship` and
    /// the next render normalises the row to the simple form. The
    /// warning surfaces the violation so the operator either removes
    /// the text from disk or asks the schema author to widen the
    /// rel-type's posture.
    ParseDescriptionNotPermitted {
        from: EntityId,
        rel_type: String,
        target: EntityId,
    },
    /// A mem's `Mount.schema` expectation (the pin recorded in the
    /// workspace `mounts.json`) disagreed with the authoritative pin in
    /// the mem's own per-mem config. Boot resolves the effective
    /// schema from the mem config (authoritative — a copied/cloned
    /// mem is self-resolvable); this warning surfaces the discrepancy
    /// so neither value is silently dropped. Recovery: align the
    /// `mounts.json` entry to the mem's config, or correct the config.
    SchemaPinMismatch {
        /// Mem whose mount expectation and config pin disagree.
        mem: String,
        /// Authoritative pin from the mem's per-mem config.
        config_pin: String,
        /// Expectation pin recorded on the workspace mount.
        mount_pin: String,
    },
}

/// Wire-shape entry inside `MissingRequiredOutgoing.missing`. Lists the
/// relationship-name alternatives and the rendered cardinality literal
/// for one unsatisfied `RequiredOutgoing` block. Custom struct so the
/// JSON output is `{ "relationships": [...], "cardinality": "at_least_one" }`
/// — identical to the schema YAML shape, so an agent can copy the
/// envelope's `details.missing` entry directly into a `memstead_relate`
/// plan without renaming fields.
#[derive(Debug, Clone, Serialize)]
pub struct MissingRequiredOutgoingBlock {
    pub relationships: Vec<String>,
    pub cardinality: String,
}

/// Abstract recovery action attached to a `PARSED_RELATION_INVALID`
/// warning when the source mem is writable. The shape is tool-
/// agnostic: it names *what* to do, not *which tool* to call. A
/// consumer (agent, bulk-fix orchestrator, app surface) maps `kind`
/// to the concrete call on whichever MCP / CLI / UniFFI path it
/// uses; the warning's payload itself does not drift when the
/// mutation surface evolves.
///
/// `kind` is the discriminator. Additive — new variants may land as
/// the recovery taxonomy grows. Current values:
///
/// - `"remove_explicit_relation"` — drop the relation from the
///   source entity's `## Relationships` section. Agents map this to
///   `memstead_relate { from: source_id, to: target_id, type: rel_type,
///   remove: true }`. The CLI maps it to the equivalent
///   `memstead relate --remove` invocation. The bulk-fix consumer reads
///   `source_id`, `target_id`, `rel_type` straight from the payload.
///
/// The mirrored `source_id` / `target_id` / `rel_type` fields are
/// redundant with the warning's `entity_id` / `target` / `rel_type`
/// — duplication is intentional. A consumer that branches on
/// `recovery` and forwards the payload downstream does not need to
/// stitch the warning's top-level fields back in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedRelationRecovery {
    pub kind: String,
    pub source_id: EntityId,
    pub target_id: EntityId,
    pub rel_type: String,
}

impl ParsedRelationRecovery {
    /// Stable discriminator for the "drop the relation from the
    /// source markdown" recovery — the only abstract action this
    /// warning emits today.
    pub const KIND_REMOVE_EXPLICIT_RELATION: &'static str = "remove_explicit_relation";

    /// Constructor for the standard `remove_explicit_relation`
    /// recovery — the only shape produced by the parser today.
    /// Emission sites use this so the discriminator string lives in
    /// one place.
    pub fn remove_explicit_relation(
        source_id: EntityId,
        target_id: EntityId,
        rel_type: String,
    ) -> Self {
        Self {
            kind: Self::KIND_REMOVE_EXPLICIT_RELATION.to_string(),
            source_id,
            target_id,
            rel_type,
        }
    }
}

/// Per-entry result of an `apply_parse_recovery` call. One entry per
/// `PARSED_RELATION_INVALID` warning the engine observed at the call
/// site: the bulk-fix dispatches the writable-origin recoveries and
/// reports the read-only-origin warnings as skipped. Wire-equivalent
/// across MCP, CLI, and UniFFI surfaces; the renderer chooses the
/// shape it prefers.
///
/// `outcome` is the stable discriminator. Current values:
/// - `"removed"` — the source entity was re-rendered; the parse-time-
///   dropped row no longer appears in the on-disk markdown. `reason`
///   is `None`.
/// - `"skipped"` — the engine intentionally did not attempt the
///   recovery. `reason` carries a stable code: `"readonly_mount"`
///   (source mem is read-only and not engine-writable).
/// - `"failed"` — the engine attempted the recovery and the underlying
///   mutation surfaced a typed error. `reason` carries the engine's
///   `UPPER_SNAKE_CASE` error code (`HASH_MISMATCH`,
///   `WIKILINK_WITHOUT_RELATION`, etc.). The original entity-side
///   drift survives and will surface again on the next reload.
#[derive(Debug, Clone, Serialize)]
pub struct ParseRecoveryEntry {
    pub entity_id: EntityId,
    pub rel_type: String,
    pub target: EntityId,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ParseRecoveryEntry {
    pub const OUTCOME_REMOVED: &'static str = "removed";
    pub const OUTCOME_SKIPPED: &'static str = "skipped";
    pub const OUTCOME_FAILED: &'static str = "failed";

    /// Stable reason value for read-only-origin warnings the bulk-fix
    /// cannot act on — the source markdown is not engine-writable.
    pub const REASON_READONLY_MOUNT: &'static str = "readonly_mount";
}

/// Outcome of `Engine::apply_parse_recovery`. Carries one
/// `ParseRecoveryEntry` per parse-time-dropped relation observed at
/// the call site plus the last successful commit sha for callers that
/// want to poll `memstead_changes_since` for the per-entity diff. An empty
/// `entries` list means the workspace was already clean.
///
/// Idempotency: re-running on a workspace where the writable drops
/// were already cleaned produces an empty `entries` list (no work,
/// no commits, no errors).
#[derive(Debug, Clone, Default, Serialize)]
pub struct ParseRecoveryReport {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entries: Vec<ParseRecoveryEntry>,
    /// Last successful commit sha across all per-source re-renders
    /// the bulk-fix performed. Empty when no recovery wrote to disk
    /// (workspace already clean, only read-only warnings, or every
    /// writable attempt failed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit_sha: String,
}

impl fmt::Display for WarningHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WarningHint::SchemaPinMismatch {
                mem,
                config_pin,
                mount_pin,
            } => write!(
                f,
                "mem '{mem}': the workspace mount expects schema '{mount_pin}' but the \
                 mem's own config pins '{config_pin}' — the config pin is authoritative and \
                 was used; align the mounts.json entry or the mem config to clear this"
            ),
            WarningHint::MissingRequiredSection {
                key,
                heading,
                write_rules,
                ..
            } => {
                write!(
                    f,
                    "required section '{key}' (heading \"{heading}\") is empty — \
                     entity will show as unhealthy"
                )?;
                if !write_rules.is_empty() {
                    write!(f, ". Writing guidance:")?;
                    for rule in write_rules {
                        write!(f, "\n  - {rule}")?;
                    }
                }
                Ok(())
            }
            WarningHint::MissingRequiredField {
                key,
                entity_type,
                description,
                enum_values,
            } => {
                write!(
                    f,
                    "required metadata field '{key}' on type '{entity_type}' was not \
                     supplied — entity landed with a placeholder. {description}"
                )?;
                if !enum_values.is_empty() {
                    write!(f, " Allowed values: [{}].", enum_values.join(", "))?;
                }
                Ok(())
            }
            WarningHint::UndeclaredRelationshipOpen { message, .. } => f.write_str(message),
            WarningHint::DuplicateRelationship {
                rel_type,
                from,
                to,
            } => write!(
                f,
                "relationship {rel_type} from {from} to {to} already exists — no-op"
            ),
            WarningHint::NoSuchRelationship {
                rel_type,
                from,
                to,
            } => write!(
                f,
                "relationship {rel_type} from {from} to {to} does not exist — no-op"
            ),
            WarningHint::UnknownIncludeKey { key, allowed } => write!(
                f,
                "unknown include key '{key}' ignored. Allowed: [{}]",
                allowed.join(", ")
            ),
            WarningHint::LimitClamped { requested, actual } => write!(
                f,
                "limit clamped from {requested} to {actual} (max for memstead_health)"
            ),
            WarningHint::TitleNormalizedToSlugNoop {
                requested_title,
                current_slug,
            } => write!(
                f,
                "requested title '{requested_title}' normalises to the existing slug \
                 '{current_slug}' — no change written to disk"
            ),
            WarningHint::UpdateNoop { id } => write!(
                f,
                "update on {id} produced bytes-identical content — no \
                 disk write, no commit, content_hash unchanged"
            ),
            WarningHint::StubFilterExcludesAll { entity_type } => write!(
                f,
                "stub=true combined with entity_type='{entity_type}' excludes every \
                 stub — stubs carry no entity_type. Drop entity_type to list stubs."
            ),
            WarningHint::UnknownFilterKey {
                key,
                scoped_type,
                declared_on_other_types,
            } => {
                let on_other = !declared_on_other_types.is_empty();
                let scoped_matches_other = matches!(
                    scoped_type.as_deref(),
                    Some(t) if declared_on_other_types.iter().any(|o| o == t)
                );
                if let Some(t) = scoped_type.as_deref() {
                    if on_other && !scoped_matches_other {
                        let word = type_word_for(declared_on_other_types);
                        let items = format_types_clause(declared_on_other_types);
                        return write!(
                            f,
                            "filter '{key}' applied with strict type-exclusion semantics — exists on {word} {items} but the query scoped to type '{t}', where it is not declared. All entities of type '{t}' will be excluded; scope to a declaring type to apply the filter."
                        );
                    }
                    return write!(
                        f,
                        "unknown filter key '{key}' for type '{t}' — filter ignored"
                    );
                }
                if on_other {
                    let word = type_word_for(declared_on_other_types);
                    let items = format_types_clause(declared_on_other_types);
                    return write!(
                        f,
                        "filter '{key}' applied with strict type-exclusion semantics — only entities of {word} {items} will match. Scope explicitly via entity_type=… to suppress this warning."
                    );
                }
                write!(
                    f,
                    "unknown filter key '{key}' — no reachable schema declares it — filter ignored"
                )
            }
            WarningHint::FieldNotFilterable { field } => write!(
                f,
                "field '{field}' is not filterable — filter ignored"
            ),
            WarningHint::FilterValueMultiMember { key, value } => write!(
                f,
                "filter '{key}={value}' targets a csv-array field but the value contains a comma — \
                 csv fields match a single member, so the full value matches nothing. Filter on one \
                 member at a time (e.g. `{key}={first}`)",
                first = value.split(',').next().map(str::trim).unwrap_or("").trim(),
            ),
            WarningHint::FilterValueNotInEnum { key, value, allowed } => write!(
                f,
                "filter '{key}={value}' is not an allowed value for '{key}' — allowed: [{}]. \
                 The filter applies as written and matches nothing.",
                allowed.join(", ")
            ),
            WarningHint::NeighbourhoodCapped { kept, total } => write!(
                f,
                "related_to neighbourhood has {total} entities; ranked by proximity and bounded to \
                 the nearest {kept}. Narrow with `depth` or filters to see fewer, more specific hits."
            ),
            WarningHint::SearchResultsTruncated { kept, budget } => write!(
                f,
                "results trimmed to the highest-ranked {kept} hits to fit the {budget}-token budget. \
                 `_total` is the full match count — page the rest with `offset`, narrow the query, \
                 or raise `token_budget`."
            ),
            WarningHint::RangeFilterKeyMalformed { key } => write!(
                f,
                "range filter key '{key}' must start with 'min_'/'max_' or end with '_before'/'_after' — filter ignored"
            ),
            WarningHint::UnknownRangeFilterField {
                field,
                key,
                scoped_type,
                declared_on_other_types,
            } => {
                let on_other = !declared_on_other_types.is_empty();
                let scoped_matches_other = matches!(
                    scoped_type.as_deref(),
                    Some(t) if declared_on_other_types.iter().any(|o| o == t)
                );
                if let Some(t) = scoped_type.as_deref() {
                    if on_other && !scoped_matches_other {
                        let word = type_word_for(declared_on_other_types);
                        let items = format_types_clause(declared_on_other_types);
                        return write!(
                            f,
                            "range filter field '{field}' (from key '{key}') applied with strict type-exclusion semantics — exists on {word} {items} but the query scoped to type '{t}', where it is not declared. All entities of type '{t}' will be excluded; scope to a declaring type to apply the filter."
                        );
                    }
                    return write!(
                        f,
                        "unknown range filter field '{field}' (from key '{key}') for type '{t}' — filter ignored"
                    );
                }
                if on_other {
                    let word = type_word_for(declared_on_other_types);
                    let items = format_types_clause(declared_on_other_types);
                    return write!(
                        f,
                        "range filter field '{field}' (from key '{key}') applied with strict type-exclusion semantics — only entities of {word} {items} will match. Scope explicitly via entity_type=… to suppress this warning."
                    );
                }
                write!(
                    f,
                    "unknown range filter field '{field}' (from key '{key}') — no reachable schema declares it — filter ignored"
                )
            }
            WarningHint::FieldNotRangeFilterable { field } => write!(
                f,
                "field '{field}' is not range-filterable — filter ignored"
            ),
            WarningHint::SearchMemIndexUnavailable {
                mem,
                reason,
                error,
            } => match (*reason, error.as_deref()) {
                ("missing_index", _) => write!(
                    f,
                    "mem '{mem}' has no search index — query returns no hits"
                ),
                ("query_failed", Some(e)) => write!(
                    f,
                    "search index for mem '{mem}' errored: {e}"
                ),
                _ => write!(
                    f,
                    "search index for mem '{mem}' is unavailable ({reason})"
                ),
            },
            WarningHint::TitleTrimmed {
                original,
                trimmed,
            } => write!(
                f,
                "title trimmed of surrounding whitespace: {original:?} → {trimmed:?}"
            ),
            WarningHint::SuspiciousNestedPrefix {
                from,
                resolved_id,
                candidate_target,
                section,
            } => {
                write!(
                    f,
                    "wiki-link in {from}#{section} resolves to nested prefix \
                     {resolved_id} — almost certainly mem-rename drift"
                )?;
                if let Some(cand) = candidate_target {
                    write!(f, "; did you mean {cand}?")?;
                }
                Ok(())
            }
            WarningHint::InlineWikiLinkAutoStubbed { from, stubs } => {
                write!(
                    f,
                    "{from} contained {n} inline wiki-link(s) that auto-created stub \
                     entities — review whether the stubs were intended; if not, \
                     remove the inline syntax or wrap the example in a fenced/quoted \
                     form. Auto-stubbed targets:",
                    n = stubs.len(),
                )?;
                for s in stubs {
                    write!(f, "\n  - {s}")?;
                }
                Ok(())
            }
            WarningHint::SelfLinkIgnored { id } => write!(
                f,
                "{id} contains a body wiki-link to its own id — the self-referential edge \
                 was dropped (a self-link carries no navigational value). The entity was \
                 created/updated normally; remove the `[[{slug}]]` link if it was a mistake",
                slug = id.name(),
            ),
            WarningHint::CrossMemTargetMemUncreated {
                from_mem,
                to_mem,
                target_id,
            } => write!(
                f,
                "cross-mem relate from '{from_mem}' to '{target_id}': \
                 target mem '{to_mem}' is not mounted in the workspace — \
                 the auto-stub has no schema resolution until the mem is created. \
                 If '{to_mem}' is a typo, fix the relate; if forward-reference \
                 is intended, create the mem to promote the stub."
            ),
            WarningHint::NoteMissing { tool } => write!(
                f,
                "{tool} called without a `note` while \
                 `[mutations].require_notes = true` — commit landed, \
                 body carries no provenance line"
            ),
            WarningHint::IgnoredReadonlyField { field, supplied } => write!(
                f,
                "'{field}' is auto-managed by the engine — the supplied \
                 value '{supplied}' was discarded and the engine value \
                 stamped instead"
            ),
            WarningHint::OuterRepoNotIgnoringMemRepo {
                outer_repo_root,
                workspace_root,
            } => write!(
                f,
                "workspace at '{workspace_root}' is embedded inside the git \
                 repository at '{outer_repo_root}' but the outer .gitignore \
                 does not list 'mem-repo/'. Add 'mem-repo/' (or the \
                 workspace-relative equivalent) to the outer repo's \
                 .gitignore to keep mem-repo-git out of the outer index."
            ),
            WarningHint::MissingRequiredOutgoing {
                entity_type,
                entity_id,
                missing,
            } => {
                write!(
                    f,
                    "{entity_id} ({entity_type}) is missing required outgoing edges — \
                     schema declares {n} `required_outgoing` block(s) still unsatisfied:",
                    n = missing.len(),
                )?;
                for block in missing {
                    write!(
                        f,
                        "\n  - [{}] cardinality={}",
                        block.relationships.join(", "),
                        block.cardinality,
                    )?;
                }
                Ok(())
            }
            WarningHint::DuplicateSectionHeading {
                entity_id,
                section_key,
                heading,
                occurrences,
            } => write!(
                f,
                "{entity_id} declared `## {heading}` {occurrences} times — \
                 section '{section_key}' kept the first occurrence's body \
                 and dropped the rest. The next read-modify-write will \
                 collapse the markdown to one heading."
            ),
            WarningHint::MemReloaded {
                mem,
                old_head,
                new_head,
                entities_loaded,
            } => write!(
                f,
                "mem '{mem}' was reloaded — on-disk HEAD advanced from \
                 {old_head} to {new_head} (a sibling writer or out-of-band \
                 commit landed since the engine last read the mem). \
                 {entities_loaded} entities reloaded; response carries \
                 fresh content. Re-derive any conclusions that depended on \
                 the prior content of this mem before continuing. Call \
                 `memstead_changes_since since={old_head}` for the per-entity \
                 diff."
            ),
            WarningHint::AutoStubCreated { stub_id } => write!(
                f,
                "target '{stub_id}' did not exist — stub auto-created. \
                 Promote it via memstead_create when authoring the real \
                 entity (stub adoption preserves the incoming edge)."
            ),
            WarningHint::ParsedRelationInvalid {
                entity_id,
                rel_type,
                target,
                reason,
                origin,
                recovery: _,
            } => {
                let recovery_msg = if origin == "readonly" {
                    "Source mem is mounted read-only; the engine cannot \
                     rewrite the markdown. Either uninstall the archive \
                     or accept the dropped relation."
                } else {
                    "Fix the source markdown (via memstead_update / \
                     memstead_relate — `details.recovery` carries the abstract \
                     action) or adjust the schema."
                };
                write!(
                    f,
                    "parsed relation {rel_type} from {entity_id} to \
                     {target} was dropped — reason: {reason}, origin: \
                     {origin}. The entity loaded but the relation does \
                     not appear in the in-memory graph. {recovery_msg}"
                )
            }
            WarningHint::ResidualStubForReadOnlyReferrers { id, referrers } => write!(
                f,
                "{id} was deleted from disk but {n} read-only-mount \
                 referrer(s) still target it; the in-memory entity is \
                 demoted to a stub at the same id so the surviving \
                 incoming edges keep a valid target. Surviving referrers: \
                 [{}]. Either accept the stub or uninstall the source \
                 archive — read-only content cannot be rewritten by the \
                 engine.",
                referrers
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                n = referrers.len(),
            ),
            WarningHint::AmbiguousDescriptionDelimiter {
                from,
                rel_type,
                target,
                trailing,
            } => write!(
                f,
                "{from} → {target} ({rel_type}): trailing content {trailing:?} \
                 after `]]` did not match the canonical em-dash delimiter ` — ` \
                 (U+2014); content dropped, the relation parses with no \
                 description. Restore with `memstead_relate {from} {rel_type} \
                 {target} --description \"<text>\"` (or hand-edit using \
                 ` — `) if the text was intentional."
            ),
            WarningHint::ParseMissingRequiredDescription {
                from,
                rel_type,
                target,
            } => write!(
                f,
                "{from} → {target} ({rel_type}): rel-type declares \
                 `per_edge_description: required` but the row has no \
                 trailing em-dash description. Add one via `memstead_relate \
                 {from} {rel_type} {target} --description \"<text>\"` (or \
                 hand-edit the markdown using ` — `)."
            ),
            WarningHint::ParseDescriptionNotPermitted {
                from,
                rel_type,
                target,
            } => write!(
                f,
                "{from} → {target} ({rel_type}): rel-type declares \
                 `per_edge_description: forbidden` but the markdown row \
                 carries a trailing description. The description is \
                 dropped from the in-memory graph and the next render \
                 normalises the row to the simple form. Drop the trailing \
                 text from the source markdown if it should not round-trip."
            ),
            WarningHint::MemReattachedAfterUnregister {
                mem, unregistered_at,
            } => write!(
                f,
                "mem '{mem}' was reattached to pre-existing storage \
                 that carried an `unregistered_at: {unregistered_at}` \
                 tombstone marker. The entities from the prior session \
                 were adopted; the tombstone has been cleared. If this \
                 reattach was unexpected, run `memstead mem delete \
                 {mem}` to destroy the storage and start fresh."
            ),
            WarningHint::MemFilesNotDeleted {
                mem, reason, path, error,
            } => {
                match (reason.as_str(), path.as_deref(), error.as_deref()) {
                    ("rmdir_failed", Some(p), Some(e)) => write!(
                        f,
                        "mem '{mem}' was unregistered but rmdir of \
                         {p:?} failed: {e}. Files remain on disk; agent \
                         may follow up with manual cleanup."
                    ),
                    ("rmdir_failed", Some(p), None) => write!(
                        f,
                        "mem '{mem}' was unregistered but rmdir of \
                         {p:?} failed. Files remain on disk."
                    ),
                    ("backend_prune_failed", _, Some(e)) => write!(
                        f,
                        "mem '{mem}' was unregistered but backend \
                         artifact cleanup failed: {e}. The mem-repo \
                         branch and/or `__MEMSTEAD:mems/.../config.json` \
                         entry may survive; rerun delete with the same \
                         arguments or have an operator inspect."
                    ),
                    ("backend_prune_failed", _, None) => write!(
                        f,
                        "mem '{mem}' was unregistered but backend \
                         artifact cleanup failed. The mem-repo branch \
                         and/or `__MEMSTEAD` config entry may survive."
                    ),
                    _ => write!(
                        f,
                        "mem '{mem}' was unregistered but \
                         `delete_files: true` did not run to completion \
                         (reason: {reason})."
                    ),
                }
            }
        }
    }
}

impl WarningHint {
    /// Stable UPPER_SNAKE_CASE identifier. Wire-level contract — never rename
    /// an existing value; new variants add new codes. Agents branch on this,
    /// not on [`WarningHint::message`].
    pub fn code(&self) -> &'static str {
        match self {
            Self::InlineWikiLinkAutoStubbed { .. } => "INLINE_WIKI_LINK_AUTO_STUBBED",
            Self::CrossMemTargetMemUncreated { .. } => "CROSS_MEM_TARGET_MEM_UNCREATED",
            Self::MissingRequiredSection { .. } => "MISSING_REQUIRED_SECTION",
            Self::MissingRequiredField { .. } => "MISSING_REQUIRED_FIELD",
            Self::UndeclaredRelationshipOpen { .. } => "UNDECLARED_RELATIONSHIP_OPEN",
            Self::DuplicateRelationship { .. } => "DUPLICATE_RELATIONSHIP",
            Self::NoSuchRelationship { .. } => "NO_SUCH_RELATIONSHIP",
            Self::UnknownIncludeKey { .. } => "UNKNOWN_INCLUDE_KEY",
            Self::LimitClamped { .. } => "LIMIT_CLAMPED",
            Self::TitleNormalizedToSlugNoop { .. } => "TITLE_NORMALIZED_TO_SLUG_NOOP",
            Self::UpdateNoop { .. } => "UPDATE_NOOP",
            Self::StubFilterExcludesAll { .. } => "STUB_FILTER_EXCLUDES_ALL",
            // One code per outcome:
            // a key declared on some OTHER reachable type was applied
            // with strict type-narrowing (the filter took effect — it
            // restricts the result to the declaring type(s)), so it
            // carries a distinct code from a key no schema declares
            // (which is truly ignored). A consumer branches on `code`
            // alone to learn whether its filter took effect, without
            // inspecting `declared_on_other_types`.
            Self::UnknownFilterKey { declared_on_other_types, .. } => {
                if declared_on_other_types.is_empty() {
                    "UNKNOWN_FILTER_KEY"
                } else {
                    "FILTER_TYPE_SCOPED"
                }
            }
            Self::FieldNotFilterable { .. } => "FIELD_NOT_FILTERABLE",
            Self::FilterValueMultiMember { .. } => "FILTER_VALUE_MULTI_MEMBER",
            Self::FilterValueNotInEnum { .. } => "INVALID_ENUM_VALUE",
            Self::NeighbourhoodCapped { .. } => "NEIGHBOURHOOD_CAPPED",
            Self::SearchResultsTruncated { .. } => "SEARCH_RESULTS_TRUNCATED",
            Self::RangeFilterKeyMalformed { .. } => "RANGE_FILTER_KEY_MALFORMED",
            Self::UnknownRangeFilterField { declared_on_other_types, .. } => {
                if declared_on_other_types.is_empty() {
                    "UNKNOWN_RANGE_FILTER_FIELD"
                } else {
                    "RANGE_FILTER_TYPE_SCOPED"
                }
            }
            Self::FieldNotRangeFilterable { .. } => "FIELD_NOT_RANGE_FILTERABLE",
            Self::SearchMemIndexUnavailable { .. } => "SEARCH_MEM_INDEX_UNAVAILABLE",
            Self::TitleTrimmed { .. } => "TITLE_TRIMMED",
            Self::SuspiciousNestedPrefix { .. } => "SUSPICIOUS_NESTED_PREFIX",
            Self::NoteMissing { .. } => "NOTE_MISSING",
            Self::IgnoredReadonlyField { .. } => "IGNORED_READONLY_FIELD",
            Self::OuterRepoNotIgnoringMemRepo { .. } => "OUTER_REPO_NOT_IGNORING_MEM_REPO",
            Self::MissingRequiredOutgoing { .. } => "MISSING_REQUIRED_OUTGOING",
            Self::DuplicateSectionHeading { .. } => "DUPLICATE_SECTION_HEADING",
            Self::MemReloaded { .. } => "MEM_RELOADED",
            Self::SchemaPinMismatch { .. } => "SCHEMA_PIN_MISMATCH",
            Self::AutoStubCreated { .. } => "AUTO_STUB_CREATED",
            Self::SelfLinkIgnored { .. } => "SELF_LINK_IGNORED",
            Self::ParsedRelationInvalid { .. } => "PARSED_RELATION_INVALID",
            Self::ResidualStubForReadOnlyReferrers { .. } => {
                "RESIDUAL_STUB_FOR_READONLY_REFERRERS"
            }
            Self::MemFilesNotDeleted { .. } => "MEM_FILES_NOT_DELETED",
            Self::MemReattachedAfterUnregister { .. } => "MEM_REATTACHED_AFTER_UNREGISTER",
            Self::AmbiguousDescriptionDelimiter { .. } => "AMBIGUOUS_DESCRIPTION_DELIMITER",
            Self::ParseMissingRequiredDescription { .. } => "MISSING_REQUIRED_DESCRIPTION",
            Self::ParseDescriptionNotPermitted { .. } => "DESCRIPTION_NOT_PERMITTED",
        }
    }

    /// Human-readable message — delegates to `Display`. May change across
    /// releases; use [`WarningHint::code`] for branching.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// Mem that "owns" the warning when one can be attributed.
    /// Workspace-/request-scoped variants return `None` — `memstead_health`'s
    /// mem filter keeps those visible regardless of scope, while
    /// mem-attributable variants drop out when the filter doesn't
    /// match. The contract mirrors the data fields the same filter
    /// gates (counts, distributions, detail lists are source-mem
    /// scoped; rosters stay global).
    pub fn source_mem(&self) -> Option<&str> {
        match self {
            Self::SuspiciousNestedPrefix { from, .. } => Some(from.mem()),
            Self::DuplicateSectionHeading { entity_id, .. } => Some(entity_id.mem()),
            Self::SchemaPinMismatch { mem, .. } => Some(mem.as_str()),
            Self::MemReloaded { mem, .. } => Some(mem.as_str()),
            Self::MemFilesNotDeleted { mem, .. } => Some(mem.as_str()),
            Self::MemReattachedAfterUnregister { mem, .. } => Some(mem.as_str()),
            Self::MissingRequiredOutgoing { entity_id, .. } => Some(entity_id.mem()),
            Self::DuplicateRelationship { from, .. } => Some(from.mem()),
            Self::NoSuchRelationship { from, .. } => Some(from.mem()),
            Self::InlineWikiLinkAutoStubbed { from, .. } => Some(from.mem()),
            Self::SelfLinkIgnored { id } => Some(id.mem()),
            Self::CrossMemTargetMemUncreated { from_mem, .. } => Some(from_mem.as_str()),
            Self::AutoStubCreated { stub_id } => Some(stub_id.mem()),
            Self::UpdateNoop { id } => Some(id.mem()),
            Self::ParsedRelationInvalid { entity_id, .. } => Some(entity_id.mem()),
            Self::ResidualStubForReadOnlyReferrers { id, .. } => Some(id.mem()),
            Self::AmbiguousDescriptionDelimiter { from, .. } => Some(from.mem()),
            Self::ParseMissingRequiredDescription { from, .. } => Some(from.mem()),
            Self::ParseDescriptionNotPermitted { from, .. } => Some(from.mem()),
            // Search-mem-index unavailability is attributable to the
            // failing mem; the filter-key warnings are request-
            // derived (the agent's filter payload) and fall through
            // to `None` below to stay visible to the caller.
            Self::SearchMemIndexUnavailable { mem, .. } => Some(mem.as_str()),
            // Workspace- or request-scoped — no mem to attribute.
            // OuterRepoNotIgnoringMemRepo concerns the embedding repo,
            // not a specific mem; an agent should see it under any
            // filter. UnknownIncludeKey / LimitClamped / NoteMissing /
            // TitleNormalizedToSlugNoop / StubFilterExcludesAll /
            // UndeclaredRelationshipOpen / MissingRequiredSection /
            // MissingRequiredField are request-derived (mutation
            // payload or schema-level), so the mem is the
            // request's mem — `None` here keeps them visible to
            // the caller that triggered them.
            _ => None,
        }
    }

    /// One representative of every `WarningHint` variant — the single
    /// source of truth consumed by stability tests (`envelope_*`,
    /// `code_values_are_upper_snake_case`) and by the MCP description
    /// drift-guard (`every_warning_code_appears_in_a_description`).
    /// Adding a new variant without extending this list fails those tests;
    /// that's the forcing function.
    pub fn all_samples() -> Vec<WarningHint> {
        vec![
            WarningHint::MissingRequiredSection {
                entity_type: "t".into(),
                key: "k".into(),
                heading: "H".into(),
                write_rules: vec![],
            },
            WarningHint::MissingRequiredField {
                entity_type: "decision".into(),
                key: "decided_on".into(),
                description: "Date the decision was accepted.".into(),
                enum_values: vec![],
            },
            WarningHint::UndeclaredRelationshipOpen {
                rel_type: "X".into(),
                message: "m".into(),
            },
            WarningHint::DuplicateRelationship {
                rel_type: "X".into(),
                from: EntityId("a".into()),
                to: EntityId("b".into()),
            },
            WarningHint::NoSuchRelationship {
                rel_type: "X".into(),
                from: EntityId("a".into()),
                to: EntityId("b".into()),
            },
            WarningHint::UnknownIncludeKey {
                key: "x".into(),
                allowed: vec![],
            },
            WarningHint::LimitClamped { requested: 1, actual: 1 },
            WarningHint::SearchResultsTruncated {
                kept: 12,
                budget: 12_000,
            },
            WarningHint::TitleNormalizedToSlugNoop {
                requested_title: "Hello World!".into(),
                current_slug: "hello-world".into(),
            },
            WarningHint::UpdateNoop {
                id: EntityId("specs--example".into()),
            },
            WarningHint::StubFilterExcludesAll {
                entity_type: "spec".into(),
            },
            // Non-empty `declared_on_other_types` → code FILTER_TYPE_SCOPED.
            WarningHint::UnknownFilterKey {
                key: "nonexistent_field".into(),
                scoped_type: Some("spec".into()),
                declared_on_other_types: vec!["decision".into()],
            },
            // Empty `declared_on_other_types` → code UNKNOWN_FILTER_KEY.
            WarningHint::UnknownFilterKey {
                key: "stauts".into(),
                scoped_type: None,
                declared_on_other_types: vec![],
            },
            WarningHint::FieldNotFilterable {
                field: "title".into(),
            },
            WarningHint::RangeFilterKeyMalformed {
                key: "weird_key".into(),
            },
            // Empty `declared_on_other_types` → code UNKNOWN_RANGE_FILTER_FIELD.
            WarningHint::UnknownRangeFilterField {
                field: "count".into(),
                key: "min_count".into(),
                scoped_type: None,
                declared_on_other_types: vec![],
            },
            // Non-empty → code RANGE_FILTER_TYPE_SCOPED.
            WarningHint::UnknownRangeFilterField {
                field: "priority".into(),
                key: "min_priority".into(),
                scoped_type: Some("spec".into()),
                declared_on_other_types: vec!["decision".into()],
            },
            WarningHint::FieldNotRangeFilterable {
                field: "tags".into(),
            },
            WarningHint::SearchMemIndexUnavailable {
                mem: "specs".into(),
                reason: "missing_index",
                error: None,
            },
            WarningHint::SuspiciousNestedPrefix {
                from: EntityId("test-mem-plugin--audit-skill".into()),
                resolved_id: EntityId(
                    "test-mem-plugin--plugin--memstead-mcp-tool-surface".into(),
                ),
                candidate_target: Some(EntityId(
                    "test-mem-plugin--memstead-mcp-tool-surface".into(),
                )),
                section: "constraints".into(),
            },
            WarningHint::InlineWikiLinkAutoStubbed {
                from: EntityId("specs--demo".into()),
                stubs: vec![EntityId("specs--example-target".into())],
            },
            WarningHint::CrossMemTargetMemUncreated {
                from_mem: "specs".into(),
                to_mem: "memos".into(),
                target_id: EntityId("memos--example".into()),
            },
            WarningHint::NoteMissing {
                tool: "memstead_update".into(),
            },
            WarningHint::OuterRepoNotIgnoringMemRepo {
                outer_repo_root: "/repos/demo".into(),
                workspace_root: "/repos/demo/memstead".into(),
            },
            WarningHint::MissingRequiredOutgoing {
                entity_type: "decision".into(),
                entity_id: EntityId("planning--decision-x".into()),
                missing: vec![
                    MissingRequiredOutgoingBlock {
                        relationships: vec!["CHOSEN".into()],
                        cardinality: "at_least_one".into(),
                    },
                    MissingRequiredOutgoingBlock {
                        relationships: vec!["REJECTED".into()],
                        cardinality: "at_least_one".into(),
                    },
                ],
            },
            WarningHint::DuplicateSectionHeading {
                entity_id: EntityId("plugin--hooks-subsystem".into()),
                section_key: "realization".into(),
                heading: "Realization".into(),
                occurrences: 3,
            },
            WarningHint::MemReloaded {
                mem: "test-mem-plugin".into(),
                old_head: "abc123".into(),
                new_head: "def456".into(),
                entities_loaded: 42,
            },
            WarningHint::AutoStubCreated {
                stub_id: EntityId("specs--future-target".into()),
            },
            WarningHint::ParsedRelationInvalid {
                entity_id: EntityId("specs--example-source".into()),
                rel_type: "EXECUTES".into(),
                target: EntityId("specs--example-target".into()),
                reason: "shape".into(),
                origin: "writable".into(),
                recovery: Some(ParsedRelationRecovery::remove_explicit_relation(
                    EntityId("specs--example-source".into()),
                    EntityId("specs--example-target".into()),
                    "EXECUTES".into(),
                )),
            },
            WarningHint::ResidualStubForReadOnlyReferrers {
                id: EntityId("specs--archived-target".into()),
                referrers: vec![EntityId("archive--archived-source".into())],
            },
            WarningHint::MemFilesNotDeleted {
                mem: "plan-example".into(),
                reason: "backend_prune_failed".into(),
                path: None,
                error: Some("ref-edit transaction rejected".into()),
            },
            WarningHint::MemReattachedAfterUnregister {
                mem: "plan-example".into(),
                unregistered_at: "2026-05-17T08:43:29Z".into(),
            },
            WarningHint::AmbiguousDescriptionDelimiter {
                from: EntityId("specs--example-source".into()),
                rel_type: "OTHER".into(),
                target: EntityId("specs--example-target".into()),
                trailing: " -- legacy delimiter".into(),
            },
            WarningHint::ParseMissingRequiredDescription {
                from: EntityId("specs--example-source".into()),
                rel_type: "OTHER".into(),
                target: EntityId("specs--example-target".into()),
            },
            WarningHint::ParseDescriptionNotPermitted {
                from: EntityId("specs--example-source".into()),
                rel_type: "IMPLEMENTS".into(),
                target: EntityId("specs--example-target".into()),
            },
        ]
    }

    fn details_payload(&self) -> serde_json::Value {
        match self {
            Self::MissingRequiredSection {
                entity_type,
                key,
                heading,
                write_rules,
            } => serde_json::json!({
                "entity_type": entity_type,
                "key": key,
                "heading": heading,
                "write_rules": write_rules,
            }),
            Self::MissingRequiredField {
                entity_type,
                key,
                description,
                enum_values,
            } => serde_json::json!({
                "entity_type": entity_type,
                "key": key,
                "field_description": description,
                "enum_values": enum_values,
            }),
            Self::UndeclaredRelationshipOpen { rel_type, .. } => {
                serde_json::json!({ "rel_type": rel_type })
            }
            Self::DuplicateRelationship { rel_type, from, to } => {
                serde_json::json!({ "rel_type": rel_type, "from": from, "to": to })
            }
            Self::NoSuchRelationship { rel_type, from, to } => {
                serde_json::json!({ "rel_type": rel_type, "from": from, "to": to })
            }
            Self::UnknownIncludeKey { key, allowed } => {
                serde_json::json!({ "key": key, "allowed": allowed })
            }
            Self::LimitClamped { requested, actual } => {
                serde_json::json!({ "requested": requested, "actual": actual })
            }
            Self::TitleNormalizedToSlugNoop {
                requested_title,
                current_slug,
            } => serde_json::json!({
                "requested_title": requested_title,
                "current_slug": current_slug,
            }),
            Self::UpdateNoop { id } => serde_json::json!({ "id": id }),
            Self::StubFilterExcludesAll { entity_type } => {
                serde_json::json!({ "entity_type": entity_type })
            }
            Self::UnknownFilterKey {
                key,
                scoped_type,
                declared_on_other_types,
            } => serde_json::json!({
                "key": key,
                "scoped_type": scoped_type,
                "declared_on_other_types": declared_on_other_types,
            }),
            Self::FieldNotFilterable { field } => serde_json::json!({ "field": field }),
            Self::FilterValueMultiMember { key, value } => {
                serde_json::json!({ "key": key, "value": value })
            }
            Self::FilterValueNotInEnum { key, value, allowed } => {
                serde_json::json!({ "key": key, "value": value, "allowed": allowed })
            }
            Self::NeighbourhoodCapped { kept, total } => {
                serde_json::json!({ "kept": kept, "total": total })
            }
            Self::SearchResultsTruncated { kept, budget } => {
                serde_json::json!({ "kept": kept, "budget": budget })
            }
            Self::RangeFilterKeyMalformed { key } => serde_json::json!({ "key": key }),
            Self::UnknownRangeFilterField {
                field,
                key,
                scoped_type,
                declared_on_other_types,
            } => serde_json::json!({
                "field": field,
                "key": key,
                "scoped_type": scoped_type,
                "declared_on_other_types": declared_on_other_types,
            }),
            Self::FieldNotRangeFilterable { field } => serde_json::json!({ "field": field }),
            Self::SearchMemIndexUnavailable {
                mem,
                reason,
                error,
            } => serde_json::json!({
                "mem": mem,
                "reason": reason,
                "error": error,
            }),
            Self::TitleTrimmed {
                original,
                trimmed,
            } => serde_json::json!({
                "original": original,
                "trimmed": trimmed,
            }),
            Self::SuspiciousNestedPrefix {
                from,
                resolved_id,
                candidate_target,
                section,
            } => serde_json::json!({
                "from": from,
                "resolved_id": resolved_id,
                "candidate_target": candidate_target,
                "section": section,
            }),
            Self::InlineWikiLinkAutoStubbed { from, stubs } => serde_json::json!({
                "from": from,
                "stubs": stubs,
            }),
            Self::SelfLinkIgnored { id } => serde_json::json!({ "id": id }),
            Self::CrossMemTargetMemUncreated {
                from_mem,
                to_mem,
                target_id,
            } => serde_json::json!({
                "from_mem": from_mem,
                "to_mem": to_mem,
                "target_id": target_id,
            }),
            Self::NoteMissing { tool } => serde_json::json!({ "tool": tool }),
            Self::IgnoredReadonlyField { field, supplied } => {
                serde_json::json!({ "field": field, "supplied": supplied })
            }
            Self::OuterRepoNotIgnoringMemRepo {
                outer_repo_root,
                workspace_root,
            } => serde_json::json!({
                "outer_repo_root": outer_repo_root,
                "workspace_root": workspace_root,
            }),
            Self::MissingRequiredOutgoing {
                entity_type,
                entity_id,
                missing,
            } => serde_json::json!({
                "entity_type": entity_type,
                "entity_id": entity_id,
                "missing": missing,
            }),
            Self::DuplicateSectionHeading {
                entity_id,
                section_key,
                heading,
                occurrences,
            } => serde_json::json!({
                "entity_id": entity_id,
                "section_key": section_key,
                "heading": heading,
                "occurrences": occurrences,
            }),
            Self::MemReloaded {
                mem,
                old_head,
                new_head,
                entities_loaded,
            } => serde_json::json!({
                "mem": mem,
                "old_head": old_head,
                "new_head": new_head,
                "entities_loaded": entities_loaded,
            }),
            Self::AutoStubCreated { stub_id } => serde_json::json!({ "stub_id": stub_id }),
            Self::ParsedRelationInvalid { entity_id, rel_type, target, reason, origin, recovery } => {
                serde_json::json!({
                    "entity_id": entity_id,
                    "rel_type": rel_type,
                    "target": target,
                    "reason": reason,
                    "origin": origin,
                    "recovery": recovery,
                })
            }
            Self::ResidualStubForReadOnlyReferrers { id, referrers } => serde_json::json!({
                "id": id,
                "referrers": referrers,
            }),
            Self::MemFilesNotDeleted { mem, reason, path, error } => serde_json::json!({
                "mem": mem,
                "reason": reason,
                "path": path,
                "error": error,
            }),
            Self::MemReattachedAfterUnregister { mem, unregistered_at } => serde_json::json!({
                "mem": mem,
                "unregistered_at": unregistered_at,
            }),
            Self::AmbiguousDescriptionDelimiter {
                from,
                rel_type,
                target,
                trailing,
            } => serde_json::json!({
                "from": from,
                "rel_type": rel_type,
                "target": target,
                "trailing": trailing,
            }),
            Self::ParseMissingRequiredDescription { from, rel_type, target } => {
                serde_json::json!({ "from": from, "rel_type": rel_type, "target": target })
            }
            Self::ParseDescriptionNotPermitted { from, rel_type, target } => {
                serde_json::json!({ "from": from, "rel_type": rel_type, "target": target })
            }
            Self::SchemaPinMismatch { mem, config_pin, mount_pin } => {
                serde_json::json!({
                    "mem": mem,
                    "config_pin": config_pin,
                    "mount_pin": mount_pin,
                })
            }
        }
    }
}

impl Serialize for WarningHint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Direct struct emission — avoids the intermediate `Value` allocation
        // `envelope(...).serialize(serializer)` would incur. Wire shape is
        // bit-identical to `envelope(...)`'s output; DRY lives at the
        // constructor level via the shared `envelope` helper used by the MCP
        // error path (`engine_err_with_suggestions`).
        let details = self.details_payload();
        let mut state = serializer.serialize_struct("WarningHint", 3)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.message())?;
        state.serialize_field("details", &details)?;
        state.end()
    }
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
    /// Per-mem commit SHA — see `UpdateResult::commit_sha`. Empty under
    /// `dry_run`.
    #[serde(default)]
    pub commit_sha: String,
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
    /// Per-mem commit SHA — see `UpdateResult::commit_sha`.
    #[serde(default)]
    pub commit_sha: String,
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
    /// Pass this as `expected_hash` on the next hash-protected op
    /// (`memstead_update`, `memstead_rename`, `memstead_delete`) on the entity — no
    /// `memstead_entity` re-read required. Mirrors `RelateResult._hash`.
    /// Wire key `_hash`.
    #[serde(default, rename = "_hash", skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    /// Per-mem commit SHA — see `UpdateResult::commit_sha`. Empty on the
    /// no-op same-title rename (no file change, no commit).
    #[serde(default)]
    pub commit_sha: String,
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
    pub to: EntityId,
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
    /// Per-mem commit SHA — see `UpdateResult::commit_sha`.
    #[serde(default)]
    pub commit_sha: String,
    /// Typed non-fatal issues — open-mode schema admissions, duplicate-add
    /// no-ops (`DuplicateRelationship`), remove-nonexistent no-ops
    /// (`NoSuchRelationship`). Previously silent edge cases now surface here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
    /// True if the op wrote to disk (real add or real remove). False on
    /// duplicate-add and remove-nonexistent-edge. Internal signal — the
    /// wrapper gates reindex + vcs_commit on this; the MCP wire relies on
    /// `commit_sha.is_empty()` as the external no-op indicator.
    #[serde(skip)]
    pub disk_changed: bool,
    /// Stub entities that became orphaned by an edge removal (their last
    /// incoming edge was this one) and were garbage-collected. Only
    /// populated on `remove: true` calls where the edge actually existed;
    /// empty on add paths and no-op removes. Empty vec is serde-omitted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_stubs_removed: Vec<EntityId>,
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
    /// Count of failed items when refused (≥1); `0` when applied.
    pub failed: usize,
    /// The single batch commit SHA when the batch applied and produced
    /// at least one write — an honest `memstead_changes_since` cursor /
    /// revert handle for the whole batch. Empty when the batch was
    /// refused, when it was empty, or when every item was a no-op (no
    /// commit happens). For a batch spanning multiple mems this names
    /// the last mem committed; single-mem batches (the common case)
    /// name their one commit.
    #[serde(default)]
    pub commit_sha: String,
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

// ---------------------------------------------------------------------------
// Search types
// ---------------------------------------------------------------------------

/// Flat query shape for full-text search. Four optional fields, all
/// combined with implicit AND across fields.
///
/// Within `any`: at least one term must match (OR semantics). Entities
/// matching more terms rank higher automatically — no explicit `and`.
/// Within `not`: none of the listed terms may appear. `phrase` requires
/// exact adjacency (case- and diacritic-folded). `field` narrows the match
/// region for all three to a single indexed field; `None` = match anywhere
/// indexed.
///
/// Empty/unset everywhere ⇒ no text predicate; `search` behaves as a
/// metadata-only filter (subsumes the former `list` semantics).
///
/// No stemming, wildcards, or regex — the caller expands morphology and
/// synonyms by enumerating variants in `any`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct Query {
    /// Terms where at least one must match (OR semantics).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any: Vec<String>,
    /// Terms that must not match (exclusion).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not: Vec<String>,
    /// Exact phrase that must appear (case- and diacritic-folded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrase: Option<String>,
    /// Restrict `any` / `not` / `phrase` to a single field (title or section
    /// key). `None` = match anywhere indexed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl Query {
    /// True if no text predicate is set — caller falls back to the
    /// metadata-only filter path.
    pub fn is_empty(&self) -> bool {
        self.any.is_empty() && self.not.is_empty() && self.phrase.is_none()
    }
}

/// Scope filters for search and list operations.
#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// Structured flat query. All text matching flows through this field;
    /// see [`Query`] for semantics. `None` (or an empty query) makes
    /// `search` behave as a metadata-only filter.
    pub query: Option<Query>,
    pub mem: Option<String>,
    pub entity_type: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Equality filters on metadata fields: `{ "level": "M0" }`.
    pub filters: HashMap<String, String>,
    /// Range filters: `{ "min_coverage": "0.5", "max_coverage": "1.0" }`.
    pub range_filters: HashMap<String, String>,
    /// Only entities with this edge type (incoming or outgoing).
    pub edge_type: Option<String>,
    /// Only entities reachable from this entity within `depth` hops.
    pub related_to: Option<EntityId>,
    pub depth: Option<usize>,
    /// Relationship types to follow from primary hits to pull in graph-proximal
    /// neighbours.
    pub expand_via: Option<Vec<String>>,
    /// Maximum hops to traverse via `expand_via` (default: 1 when `expand_via`
    /// is set).
    pub expand_depth: Option<usize>,
    /// Filter by stub status. `None` = no filter (returns both stubs and real
    /// entities); `Some(true)` = only stubs; `Some(false)` = only real entities.
    pub stub: Option<bool>,
    /// Token budget bounding the returned hit payload (search path only).
    /// `None` uses the engine default. A page whose hits exceed the budget is
    /// greedily trimmed (at least one hit always returns) with a
    /// `SEARCH_RESULTS_TRUNCATED` warning; `total` still reflects the full
    /// match count so the agent can page with `offset`.
    pub token_budget: Option<usize>,
}

/// Per-hit score components surfaced so agents can understand ranking.
///
/// Note: this is illustrative feedback, not a numerically authoritative
/// decomposition — tantivy's `Explanation` for `BoostQuery` over
/// `BooleanQuery` does not always sum cleanly. Agents should treat these
/// as proportions, not exact sums.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ScoreBreakdown {
    pub bm25: f32,
    pub title_boost: f32,
    pub field_weights: HashMap<String, f32>,
    /// `Some(f32)` on expanded hits only, carrying the depth-based decay
    /// factor (`0.5.powi(depth)`). `None` on primary hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion_decay: Option<f32>,
}

/// One snippet-level match recorded per (term, field). `heading_path` is
/// `Some` when the match falls under an H3–H6 sub-heading; elements are
/// ordered outermost → innermost.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TermMatch {
    pub field: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_path: Option<Vec<String>>,
}

/// Metadata attached to hits reached via graph expansion. The
/// primary hit that seeded the expansion is identified by `of`; `via_edge`
/// is the exact `rel_type` string; `depth` counts hops from the seed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExpansionInfo {
    pub of: EntityId,
    pub via_edge: String,
    pub depth: usize,
}

/// One sub-section-level facet entry. `path` is ordered outermost →
/// innermost, prefixed with the H2 section key (e.g. `["specifies",
/// "Response Shapes", "Markdown Output"]`). Structured vector (not a
/// delimiter-joined string) so headings containing punctuation don't break
/// the key.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubsectionFacet {
    pub path: Vec<String>,
    pub count: usize,
}

/// Fixed set of facet dimensions computed over the unpaginated hit set.
/// Tier 1 freezes the dimensions; extend later only if empirical use
/// demands it. Zero-count entries are excluded to keep the payload small.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct Facets {
    pub by_type: HashMap<String, usize>,
    pub by_mem: HashMap<String, usize>,
    pub by_level: HashMap<String, usize>,
    pub by_status: HashMap<String, usize>,
    pub by_confidence: HashMap<String, usize>,
    pub by_subsection: Vec<SubsectionFacet>,
    /// `"primary"` / `"expanded"` — counts of primary vs. graph-expanded
    /// hits. Always present; `expanded` is `0` when no expansion ran.
    pub by_expansion: HashMap<String, usize>,
}

/// A search result hit.
#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub id: EntityId,
    pub title: String,
    pub mem: String,
    pub entity_type: String,
    pub stub: bool,
    pub score: f32,
    pub tokens: usize,
    pub snippet: Option<String>,
    /// Lead/key section bodies for the hit. The `search` op leaves this
    /// **empty** — search finds entities, `memstead_entity` reads their
    /// bodies; carrying every required section per hit overflowed the MCP
    /// transport cap. The `list` op still populates it (its human-facing
    /// roster consumers read the lead section as a one-line summary).
    /// Empty maps are omitted from the serialized envelope.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sections: HashMap<String, String>,
    /// Score component breakdown — populated when the call supplied a
    /// text predicate; `None` on the metadata-only path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_breakdown: Option<ScoreBreakdown>,
    /// Per-term match details keyed by query term — populated when the
    /// call supplied a text predicate; `None` on the metadata-only path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_terms: Option<HashMap<String, Vec<TermMatch>>>,
    /// Expansion metadata — populated on hits reached via graph
    /// expansion; `None` on primary hits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<ExpansionInfo>,
    /// Lead-section summary resolved against the hit's *own* mem schema
    /// at search time (see [`SummaryPair`]). The renderer cannot resolve
    /// it correctly on its own — the global `type_by_name` only sees the
    /// `default` schema, so a `software`-schema hit (`requirement` →
    /// `Statement`, `actor` → `Role`) would miss its anchor section and
    /// render `—`. `#[serde(skip)]` keeps `SearchHit`'s wire shape
    /// unchanged; the value surfaces on the envelope's `summary_heading` /
    /// `summary_value`. `None` only on hits built outside the engine
    /// search op (FFI/bridge and test fixtures), where the renderer falls
    /// back to the default-schema lookup.
    #[serde(skip)]
    pub summary: Option<SummaryPair>,
}

/// Lead-section `(heading, value)` for a search/list hit, resolved
/// against the hit's own mem schema at search time. Carried in-memory
/// from the search op to the renderers; see [`SearchHit::summary`].
#[derive(Debug, Clone)]
pub struct SummaryPair {
    pub heading: String,
    pub value: String,
}

/// Search result with metadata.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    /// Sum of estimated tokens across all matching entities (pre-pagination).
    /// Lets agents judge read cost before paging.
    pub total_tokens: usize,
    pub hits: Vec<SearchHit>,
    /// Faceted counts over the unpaginated hit set. Stable closed
    /// struct; zero-count entries are excluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<Facets>,
    /// Non-fatal issues surfaced to the caller. Structured
    /// `WarningHint` shape (`{code, details, message}`) — same wire
    /// envelope every other tool's warnings already use. Agents
    /// branch on `code`; the message field carries the existing
    /// remediation prose.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// List result with token totals.
#[derive(Debug, Clone, Serialize)]
pub struct ListResult {
    pub total: usize,
    pub returned: usize,
    pub offset: usize,
    pub total_tokens: usize,
    pub hits: Vec<SearchHit>,
    /// Non-fatal issues surfaced to the caller — same structured
    /// shape as `SearchResult.warnings`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

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

#[derive(Debug, Clone, Serialize)]
pub struct HealthIssue {
    pub field: String,
    pub message: String,
}

/// Aggregated health report for the whole graph.
#[derive(Debug, Clone, Serialize)]
pub struct HealthSummary {
    pub stale_entities: Vec<StaleEntity>,
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
    /// Inline wiki-links in entity section bodies that resolve to stub
    /// targets (no on-disk markdown file). Populated only when the caller
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

/// One dangling wiki-link finding surfaced by
/// `memstead_health include=["dangling_links"]`. A link is dangling when its
/// resolved target is a stub (i.e. the markdown file does not exist on disk).
/// This is the post-delete / renamed-without-rewrite / typo signal.
#[derive(Debug, Clone, Serialize)]
pub struct DanglingLink {
    pub from: EntityId,
    /// Canonical ID the wiki-link resolves to. Stub-typed in the store.
    pub target_id: EntityId,
    /// Resolved mem-relative path segment of the target ID (e.g. `gone`
    /// for `specs--gone`). This is the normalised form the engine records —
    /// not the literal `[[…]]` characters as authored. Widening `WikiLink`
    /// to preserve the authored form is a future-work item if agents need
    /// grep-to-source precision.
    pub target_path: String,
    /// Section key in which the link appears (e.g. `"purpose"`). `None`
    /// only if the link appears outside any typed section — unusual but
    /// possible in free-form prose before the first heading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

// ---------------------------------------------------------------------------
// Export types
// ---------------------------------------------------------------------------

/// Export result.
///
/// Workspace-wide `export_markdown` returns this struct with
/// `skipped_mounts` populated for every mount whose active backend
/// doesn't support
/// markdown regeneration in place (git-branch, archive). Per-mem
/// export against an incompatible backend short-circuits with
/// `EngineError::MarkdownExportUnsupportedBackend` instead.
#[derive(Debug, Clone, Serialize)]
pub struct ExportResult {
    pub written: usize,
    pub unchanged: usize,
    /// Mounts that the workspace-wide export declined to write
    /// because their backend doesn't support markdown regeneration.
    /// Empty on the happy path (every mount is folder-backed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped_mounts: Vec<SkippedMount>,
}

/// One mount declined by `export_markdown` because the active
/// backend doesn't support in-place markdown regeneration.
///
/// `reason` is a stable token (today: `"backend_does_not_support_markdown_export"`);
/// `active_backend` matches [`crate::workspace::MountStorage::backend_id`].
#[derive(Debug, Clone, Serialize)]
pub struct SkippedMount {
    pub mem: String,
    pub active_backend: String,
    pub reason: String,
}

/// Result of a `.mem` mem-archive export.
#[derive(Debug, Clone, Serialize)]
pub struct MemExportResult {
    pub archive_path: String,
    pub name: String,
    pub version: String,
    pub entity_count: usize,
    pub size_bytes: u64,
    /// Cross-mem edges in the exported slice whose target won't travel
    /// inside this single-mem archive — `install` will reject the
    /// archive for each one. Surfaced at export time
    /// (`DANGLING_CROSS_MEM_EDGE_IN_EXPORT`) so the operator sees the
    /// install-time failure before sharing. Empty for a self-contained
    /// export.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dangling_cross_mem_edges: Vec<crate::validator::DanglingCrossMemEdge>,
}

/// Result of `Engine::set_mem_version`. Carries the (mem,
/// old_version, new_version) triple so callers (CLI, MCP) can surface
/// the change without an extra read.
#[derive(Debug, Clone, Serialize)]
pub struct SetMemVersionOutcome {
    pub mem: String,
    /// Previous version. `None` when the mem config carried no
    /// version field before this call (pre-gate / externally-imported
    /// config, or the residual `MEM_CONFIG_INCOMPLETE` path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_version: Option<semver::Version>,
    pub new_version: semver::Version,
    /// Concurrent-drift warnings detected at the pre-write probe —
    /// e.g. `MemReloaded` when a sibling engine committed between
    /// this engine's last snapshot and the set-version write. Empty
    /// on the happy path. F1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// Result of `Engine::set_mem_description`. Carries the (mem,
/// old_description, new_description) triple so callers can surface
/// the change without an extra read.
#[derive(Debug, Clone, Serialize)]
pub struct SetMemDescriptionOutcome {
    pub mem: String,
    /// Previous description. `None` when the mem config carried no
    /// description before this call (the common case — mem creation
    /// seeds none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_description: Option<String>,
    /// The description now persisted; `None` when the call cleared it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_description: Option<String>,
    /// Concurrent-drift warnings detected at the pre-write probe.
    /// Empty on the happy path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// Result of `Engine::set_mem_sync_state`. Carries the (mem, key,
/// previous-token) triple so callers (CLI, MCP) can surface the change
/// without an extra read. The token values are opaque to the engine —
/// see `MemConfig::sync_state`.
#[derive(Debug, Clone, Serialize)]
pub struct SetMemSyncStateOutcome {
    pub mem: String,
    /// The sync-state key that was set or cleared (opaque; the ingest
    /// layer keys per `(ingest, facet)`).
    pub key: String,
    /// Previous token under `key`, `None` when the key was unset before
    /// this call. Lets callers report set-vs-overwrite without a read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    /// True when an empty token cleared an existing key. `false` for a
    /// set/overwrite and for a clear of an already-absent key (a no-op).
    pub removed: bool,
    /// Concurrent-drift warnings detected at the pre-write probe — e.g.
    /// `MemReloaded` when a sibling engine committed between this
    /// engine's last snapshot and the write. Empty on the happy path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

// ---------------------------------------------------------------------------
// Context types
// ---------------------------------------------------------------------------

/// Context around an entity — neighbors, community, related entities.
#[derive(Debug, Clone, Serialize)]
pub struct ContextResult {
    pub entity_id: EntityId,
    pub community: Option<String>,
    pub neighbors: Vec<NeighborInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NeighborInfo {
    pub id: EntityId,
    pub title: String,
    pub relationship: String,
    pub direction: Direction,
}

#[derive(Debug, Clone, Serialize)]
pub enum Direction {
    Outgoing,
    Incoming,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Graph statistics.
#[derive(Debug, Clone, Serialize)]
pub struct Stats {
    pub entity_count: usize,
    pub edge_count: usize,
    pub edge_types: HashMap<String, usize>,
    pub community_count: usize,
    pub mem_count: usize,
    pub types_in_use: Vec<String>,
}

// ---------------------------------------------------------------------------
// Reload result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ReloadResult {
    pub added: Vec<EntityId>,
    pub changed: Vec<EntityId>,
    pub removed: Vec<EntityId>,
}

/// Per-mem reload outcome — produced by [`Engine::reload_one_mem`]
/// and surfaced verbatim in the `memstead_reload` MCP tool's response when
/// an explicit operator-triggered reload runs against a single mem.
/// Auto-reloads on the read path consume this internally and emit a
/// [`WarningHint::MemReloaded`] (which carries `mem`, `old_head`,
/// `new_head`, `entities_loaded` — the diff list is intentionally
/// omitted from the lean warning payload; agents that need it call
/// `memstead_changes_since` themselves with the supplied `old_head`).
///
/// `head_before` / `head_after` are hex-rendered SHAs (or
/// `EMPTY_TREE_SHA` for the no-baseline case) so the wire shape
/// matches what `memstead_changes_since` already accepts as `since`.
/// `changed_entity_ids` is the list of non-stub IDs whose
/// `content_hash` differs between the pre- and post-reload store
/// snapshots, plus every newly-added or newly-removed id — same
/// semantic as `ReloadResult { added, changed, removed }` flattened
/// into a single set so callers don't have to merge three lists.
#[derive(Debug, Clone, Serialize)]
pub struct ReloadReport {
    pub mem: String,
    pub head_before: String,
    pub head_after: String,
    pub entities_loaded: usize,
    pub changed_entity_ids: Vec<EntityId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Locks the wire shape of `Query` across every combination of
    // set/unset fields. Agents compose queries on the fly; a drift here
    // silently changes the MCP tool's JSON contract.
    #[test]
    fn query_json_roundtrip_every_combination() {
        let cases: Vec<Query> = vec![
            Query::default(),
            Query {
                any: vec!["auth".into()],
                ..Default::default()
            },
            Query {
                not: vec!["mock".into()],
                ..Default::default()
            },
            Query {
                phrase: Some("client side agent".into()),
                ..Default::default()
            },
            Query {
                field: Some("identity".into()),
                ..Default::default()
            },
            Query {
                any: vec!["a".into(), "b".into()],
                not: vec!["x".into()],
                phrase: Some("ex act".into()),
                field: Some("purpose".into()),
            },
        ];
        for q in &cases {
            let json = serde_json::to_string(q).expect("serialize");
            let back: Query = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(q.any, back.any, "any field round-trip: {json}");
            assert_eq!(q.not, back.not, "not field round-trip: {json}");
            assert_eq!(q.phrase, back.phrase, "phrase field round-trip: {json}");
            assert_eq!(q.field, back.field, "field field round-trip: {json}");
            assert_eq!(q.is_empty(), back.is_empty());
        }
    }

    // Empty fields stay out of the wire shape — agents see a lean object.
    #[test]
    fn query_default_serializes_as_empty_object() {
        let q = Query::default();
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, "{}", "default query must serialize as `{{}}`");
    }

    // Null / missing keys all round-trip to the same default via serde.
    #[test]
    fn query_accepts_missing_and_null_fields() {
        let with_missing: Query = serde_json::from_str("{}").unwrap();
        let with_nulls: Query = serde_json::from_str(
            r#"{"any":[],"not":[],"phrase":null,"field":null}"#,
        )
        .unwrap();
        assert!(with_missing.is_empty());
        assert!(with_nulls.is_empty());
    }

    // Schema is generated via schemars so MCP agents see the full
    // structured contract. Cheap smoke test — locks that the four known
    // fields appear and nothing regresses to an action-discriminator.
    #[test]
    fn query_json_schema_exposes_four_fields() {
        let schema = schemars::schema_for!(Query);
        let rendered = serde_json::to_string(&schema).unwrap();
        for field in ["any", "not", "phrase", "field"] {
            assert!(
                rendered.contains(&format!("\"{field}\"")),
                "schema must mention `{field}`: {rendered}"
            );
        }
    }

    // ------------------------------------------------------------------
    // WarningHint wire-envelope snapshots. Each variant locks `code`
    // (stable UPPER_SNAKE_CASE), a message substring (phrasing may
    // drift — we assert a durable anchor), and the `details` key-set.
    // Arrays are asserted shape-only because their content depends on
    // the active schema / allowed-include list.
    // ------------------------------------------------------------------

    fn to_envelope(w: &WarningHint) -> serde_json::Value {
        serde_json::to_value(w).expect("WarningHint serializes")
    }

    #[test]
    fn warning_hint_missing_required_section_envelope() {
        // F9: type-level write_rules moved out of per-warning details
        // to the mutation response's top-level `type_guidance` map.
        // The warning now carries only section-axis fields.
        let w = WarningHint::MissingRequiredSection {
            entity_type: "spec".into(),
            key: "purpose".into(),
            heading: "Purpose".into(),
            write_rules: vec!["one sentence".into(), "state the why".into()],
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "MISSING_REQUIRED_SECTION");
        assert!(json["message"].as_str().unwrap().contains("required section"));
        assert_eq!(json["details"]["entity_type"], "spec");
        assert_eq!(json["details"]["key"], "purpose");
        assert_eq!(json["details"]["heading"], "Purpose");
        assert!(json["details"]["write_rules"].is_array());
        // type_write_rules no longer rides on the per-warning envelope.
        assert!(json["details"].get("type_write_rules").is_none());
    }

    #[test]
    fn warning_hint_undeclared_relationship_open_envelope() {
        let w = WarningHint::UndeclaredRelationshipOpen {
            rel_type: "USES".into(),
            message: "USES admitted in open mode".into(),
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "UNDECLARED_RELATIONSHIP_OPEN");
        // Display delegates to the stored message — substring anchor is safe.
        assert!(json["message"].as_str().unwrap().contains("open mode"));
        assert_eq!(json["details"]["rel_type"], "USES");
        // Consistency rule: details must not duplicate the envelope message.
        assert!(json["details"].get("message").is_none());
        // Only rel_type belongs under details for this variant.
        assert_eq!(json["details"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn warning_hint_duplicate_relationship_envelope() {
        let w = WarningHint::DuplicateRelationship {
            rel_type: "USES".into(),
            from: EntityId("specs--a".into()),
            to: EntityId("specs--b".into()),
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "DUPLICATE_RELATIONSHIP");
        assert!(json["message"].as_str().unwrap().contains("already exists"));
        assert_eq!(json["details"]["rel_type"], "USES");
        assert_eq!(json["details"]["from"], "specs--a");
        assert_eq!(json["details"]["to"], "specs--b");
    }

    #[test]
    fn warning_hint_no_such_relationship_envelope() {
        let w = WarningHint::NoSuchRelationship {
            rel_type: "USES".into(),
            from: EntityId("specs--a".into()),
            to: EntityId("specs--b".into()),
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "NO_SUCH_RELATIONSHIP");
        assert!(json["message"].as_str().unwrap().contains("does not exist"));
        assert_eq!(json["details"]["rel_type"], "USES");
        assert_eq!(json["details"]["from"], "specs--a");
        assert_eq!(json["details"]["to"], "specs--b");
    }

    #[test]
    fn warning_hint_unknown_include_key_envelope() {
        let w = WarningHint::UnknownIncludeKey {
            key: "bogus".into(),
            allowed: vec!["orphans".into(), "stubs".into()],
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "UNKNOWN_INCLUDE_KEY");
        assert!(json["message"].as_str().unwrap().contains("bogus"));
        assert_eq!(json["details"]["key"], "bogus");
        assert!(json["details"]["allowed"].is_array());
    }

    #[test]
    fn warning_hint_limit_clamped_envelope() {
        let w = WarningHint::LimitClamped {
            requested: 1000,
            actual: 100,
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "LIMIT_CLAMPED");
        assert!(json["message"].as_str().unwrap().contains("clamped"));
        assert_eq!(json["details"]["requested"].as_u64(), Some(1000));
        assert_eq!(json["details"]["actual"].as_u64(), Some(100));
    }

    #[test]
    fn warning_hint_title_normalized_to_slug_noop_envelope() {
        let w = WarningHint::TitleNormalizedToSlugNoop {
            requested_title: "Hello World!".into(),
            current_slug: "hello-world".into(),
        };
        let json = to_envelope(&w);
        assert_eq!(json["code"], "TITLE_NORMALIZED_TO_SLUG_NOOP");
        assert!(
            json["message"]
                .as_str()
                .unwrap()
                .contains("no change written to disk")
        );
        assert_eq!(json["details"]["requested_title"], "Hello World!");
        assert_eq!(json["details"]["current_slug"], "hello-world");
    }

    // Top-level envelope shape lock — every WarningHint emits exactly
    // three keys and nothing else. Protects against accidental field
    // additions at the envelope level.
    #[test]
    fn warning_hint_envelope_has_exactly_three_top_level_keys() {
        for w in &WarningHint::all_samples() {
            let json = to_envelope(w);
            let obj = json.as_object().expect("envelope is an object");
            assert_eq!(
                obj.len(),
                3,
                "{} must emit exactly 3 top-level keys; got {:?}",
                w.code(),
                obj.keys().collect::<Vec<_>>()
            );
            assert!(obj.contains_key("code"));
            assert!(obj.contains_key("message"));
            assert!(obj.contains_key("details"));
        }
    }

    // Stability lock — `code()` values are a public wire contract. Every
    // variant must expose an UPPER_SNAKE_CASE identifier. Catches
    // accidental rename / case drift in a single test.
    #[test]
    fn warning_hint_code_values_are_upper_snake_case() {
        let re = regex::Regex::new(r"^[A-Z][A-Z0-9_]*$").unwrap();
        for w in &WarningHint::all_samples() {
            let code = w.code();
            assert!(
                re.is_match(code),
                "code() violates UPPER_SNAKE_CASE: {code}"
            );
        }
    }

    // Envelope helper emits the same shape as WarningHint::serialize — one
    // constructor, two callers (warnings + MCP error path).
    #[test]
    fn envelope_shape_is_code_message_details() {
        let v = envelope("FOO_BAR", "hello", serde_json::json!({ "x": 1 }));
        assert_eq!(v["code"], "FOO_BAR");
        assert_eq!(v["message"], "hello");
        assert_eq!(v["details"]["x"], 1);
        assert_eq!(
            v.as_object().unwrap().len(),
            3,
            "envelope has exactly 3 top-level keys"
        );
    }
}
