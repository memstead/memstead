//! The typed non-fatal warning engine operations surface: the `WarningHint` enum and the companion types its variants carry, the closed `MountUnbackedReason` vocabulary, and the custom `Serialize` that renders every variant as the uniform `{ code, message, details }` envelope.

use super::*;

mod display;
mod methods;

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
    UndeclaredRelationshipOpen { rel_type: String, message: String },
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
    UnknownIncludeKey { key: String, allowed: Vec<String> },
    /// A paged/bounded parameter exceeded its cap. The cap is authoritative
    /// so the op still ran, but the warning surfaces what the caller
    /// requested vs. what was served.
    LimitClamped { requested: usize, actual: usize },
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
    /// The title grammar admits any single-line text, but the slug
    /// alphabet stays narrow — this create/rename derived an id that
    /// dropped one or more title characters (`&`, `.`, `§`, …). The
    /// entity lands with the verbatim title; the warning keeps the
    /// title↔id divergence visible without being fatal, naming each
    /// distinct dropped character and the derived slug.
    TitleCharsDroppedFromSlug {
        title: String,
        dropped_chars: Vec<char>,
        slug: String,
    },
    /// `memstead_update` produced a post-mutation entity whose regenerated
    /// markdown is bytes-identical to the on-disk content — no field,
    /// section, metadata value, relation, or auto-timestamp actually
    /// changed. The op is a successful no-op: no disk write, no
    /// commit, `content_hash` unchanged. Surfaced so autonomous skills
    /// branching on `write_id != ""` see an explicit signal, and
    /// `expected_hash`-based polling stays stable across the no-op.
    /// Mirrors `TitleNormalizedToSlugNoop` for the rename surface.
    UpdateNoop { id: EntityId },
    /// `memstead_search` was called with both `stub=true` and `entity_type`
    /// set. Stubs carry no `entity_type` (they are ID-only placeholders),
    /// so the combined filter excludes every stub — the call is an empty
    /// set by construction. Surfaced so an agent doesn't interpret the
    /// empty result as "no stubs of this type exist" when in fact no
    /// stub can ever satisfy the filter. Drop `entity_type` to list stubs.
    StubFilterExcludesAll { entity_type: String },
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
    ///
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
    FieldNotFilterable { field: String },
    /// `memstead_search(filters: {<csv-field>: "a,b"})` passed a comma-bearing
    /// value to a csv-array field. csv fields match a *single* member, so
    /// the whole rendered value (e.g. the `tags: dedup,retry` an entity
    /// displays) can never equal any one member — the filter matches
    /// nothing. Surfaced so an agent that copied the rendered value gets a
    /// recoverable signal (split into repeated single-member filters)
    /// rather than an empty result indistinguishable from a true
    /// no-match. The filter still applies as written (matches nothing);
    /// this only adds the advisory.
    FilterValueMultiMember { key: String, value: String },
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
    NeighbourhoodCapped { kept: usize, total: usize },
    /// `memstead_search` trimmed the returned page to fit the token budget.
    /// The highest-ranked `kept` hits that fit under `budget` are returned;
    /// the rest of the page is dropped so the response stays under the MCP
    /// transport cap. `_total` still reflects the full match count — page the
    /// remainder with `offset`, narrow the query, or raise `token_budget`.
    SearchResultsTruncated { kept: usize, budget: usize },
    /// `memstead_search(range_filters: {<key>: ...})` named a key that
    /// doesn't follow the `min_<field>` / `max_<field>` / `<field>_before`
    /// / `<field>_after` grammar. The key is ignored.
    RangeFilterKeyMalformed { key: String },
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
    FieldNotRangeFilterable { field: String },
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
    TitleTrimmed { original: String, trimmed: String },
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
        /// Whether the link's prefix (`resolved_id.mem()`) is itself a
        /// mounted mem. `true` means the link is a well-formed cross-mem
        /// reference whose target is missing in that mem (no rename
        /// happened); `false` means the prefix only resembles a mem
        /// (it matches a roster member's last name segment), the
        /// classic mem-rename drift. The message says which, instead
        /// of calling every case rename drift: on this project's own graph all
        /// eight recorded hits were missing targets in mounted mems.
        prefix_mounted: bool,
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
    SelfLinkIgnored { id: EntityId },
    /// A body wiki-link crossed into a destination whose SCHEMA the
    /// source schema declares no cross-mem entry for (and no wildcard),
    /// so the alias-synthesis pass emitted no edge — the schema
    /// legitimately declines it, and the write still succeeds. Before
    /// this warning the link became inert prose SILENTLY (found by the
    /// graph-plans 02 grading: a default-schema scratch mem citing a
    /// planning mem, 2026-08-28); the write knows it dropped the edge,
    /// so it says so, naming the target and the declaration gap. The
    /// remedy is schema-side: declare the destination schema (or a
    /// wildcard) under `cross_mem_relationships`.
    CrossSchemaLinkUndeclared {
        /// The entity carrying the link.
        from: EntityId,
        /// The link's resolved target.
        target: EntityId,
        /// The source mem's schema (`name@version` display form).
        source_schema: String,
        /// The target mem's schema name — the missing `to_schema` entry.
        target_schema: String,
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
    /// A successful write moved a declared aggregate signal across a
    /// threshold, in either direction. Out-of-band diagnostics beside
    /// the success payload — never error-shaped, never changing the
    /// mutation's success semantics (a signal crossing on a
    /// successful write must not read as a failed write). Levels are
    /// the wire literals `none` / `notice` / `warn`.
    SignalThresholdCrossed {
        entity_id: EntityId,
        signal: String,
        value: u64,
        old_level: String,
        new_level: String,
    },
    /// The written entity violates warn-tier declared `constraints`
    /// of its type (e.g. `requires_when`: a field required under the
    /// current value of another field is unset). Block-tier violations
    /// refuse instead ([`EngineError::ConstraintUnsatisfied`]) — the
    /// warning only ever carries `severity: warn` entries.
    ConstraintUnsatisfied {
        entity_type: String,
        entity_id: EntityId,
        violations: Vec<crate::ops::health::UnsatisfiedConstraint>,
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
    /// `MEM_ROSTER_CHANGED`: the mount roster changed since the engine last
    /// reconciled it (a mem registered or unregistered by another process),
    /// and the engine applied the change before serving this call: `added`
    /// mems mounted cold, `removed` mems unmounted (their cached hashes are
    /// void, an operation naming one refuses `MEM_UNMOUNTED`),
    /// `quarantined` mems failed to mount under the boot rules and are on
    /// the quarantine roster with their reason, `failures` names anything
    /// that could not be applied (that part is retried next operation).
    MemRosterChanged {
        added: Vec<String>,
        removed: Vec<String>,
        quarantined: Vec<String>,
        failures: Vec<String>,
    },
    /// `OUT_OF_BAND_EDITS_UNDETECTED`: this folder mem's drift cursor is its
    /// own change ledger, which only the engine writes, so an edit made to the
    /// files by anything else advances nothing.
    ///
    /// The engine keeps serving pre-edit content and `changes_since` reports
    /// the edit as never having happened. It is not fixable cheaply: the
    /// staleness probe runs before every operation, and turning it into a
    /// directory walk would change the cost profile of the whole folder
    /// backend. So the engine says it cannot detect them rather than staying
    /// quiet, and `memstead health --include ledger` reconciles on demand.
    ///
    /// Never fires for a git-branch mem: its change set is a real two-tree
    /// diff, so the condition cannot arise.
    OutOfBandEditsUndetected { mem: String },
    /// A destination entity of a projection binding names an in-scope source
    /// artifact in its prose and carries no anchor on it — a claim no verify
    /// watches on the entity's behalf. Read off the binding's findings store
    /// (the `unanchored-mention` class the verify pass records); warn-level,
    /// never a refusal. Remedy: anchor the artifact on the entity
    /// (`memstead_update` with `anchors`), or exclude the artifact with a
    /// rationale (`memstead projection exclude`).
    UnanchoredMention {
        mem: String,
        binding: String,
        entity: EntityId,
        artifact: String,
        section: String,
    },
    /// A config write found the stored config had moved on from what this
    /// engine last observed: another writer changed it in between.
    ///
    /// The write still lands. It is applied to the CONFIG THAT IS THERE, not
    /// to the engine's cached copy, so the intervening writer's fields
    /// survive; `fields` names what they had changed. The warning exists
    /// because a caller who set one field and finds three different is owed
    /// the explanation on the response that did it, not in a log.
    ///
    /// Never fires in a single-writer workspace: the cached copy equals the
    /// file there, so there is nothing to report.
    ConfigWriteIntervened { mem: String, fields: Vec<String> },
    /// A mutation verb was given an entity id without its mem prefix
    /// and exactly one mounted mem carried an entity of that slug, so
    /// the verb acted on that entity. Announced, never silent: the
    /// caller learns the full id the write landed on, and a second
    /// mem gaining the slug later turns the same call into an
    /// `ENTITY_ID_MISSING_MEM` refusal rather than a silent retarget.
    ShortIdResolved { given: String, resolved: EntityId },
    /// `memstead_relate` add path landed on a not-yet-real target id and
    /// the engine materialised a stub at that id (in-memory upsert; the
    /// file lands when a follow-up `memstead_create` promotes the stub).
    /// Pre-fix surfaced through a top-level `stub_warning: Option<String>`
    /// field on the relate response — agents iterating `warnings[]` to
    /// surface non-fatal findings silently skipped the auto-stub case.
    /// Carries the materialised stub id so the agent can pin a
    /// follow-up `memstead_create` (or `memstead_relate remove=true` to drop
    /// the edge before authoring). `pending` marks the dry-run path:
    /// the rehearsal validated the add and REPORTS the would-be stub
    /// without writing it — the code stays `AUTO_STUB_CREATED`
    /// (response-shape stability), only the message branches, so a
    /// rehearsed response never claims a performed effect.
    AutoStubCreated { stub_id: EntityId, pending: bool },
    /// A duplicate-add `memstead_relate` on a derivation-declared
    /// rel-type refreshed the edge's baseline —
    /// the agent's explicit "I have reviewed the target's change; the
    /// derivation still holds". Sidecar-only: `_hash` unchanged, the
    /// edge unchanged; the response carries this warning so the
    /// refresh is stated rather than a bare no-op.
    DerivationBaselineRefreshed {
        from: EntityId,
        rel_type: String,
        to: EntityId,
    },
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
    /// Hand-edits, external tooling, and embedder editor
    /// surfaces can inject relations that bypass `memstead_relate`; the
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
    /// agent walking `memstead_health`, a bulk-fix orchestrator, a
    /// UI drift panel) maps `kind` to the concrete call on
    /// whichever MCP / CLI surface it uses. `None` when
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
    ///
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
    /// One-time boot migration: legacy `readMems` entries found in a
    /// writable mem's config were converted into workspace-level
    /// read-only mounts and the legacy key was removed from the
    /// config. `mems` lists the migrated read-mem names,
    /// `from_host_mems` the writable mems whose configs carried them.
    /// A second boot is silent — the source key is gone.
    ReadMemsMigratedToMounts {
        mems: Vec<String>,
        from_host_mems: Vec<String>,
    },
    /// Boot-honesty skew: the mem's engine-owned mutation stamp
    /// (`MemConfig.mutation_stamp`, written after mutations) records a
    /// different engine version than the running binary. Informative,
    /// never fatal — the next mutation under this binary re-stamps.
    /// Absence of a stamp (a pre-stamp mem) never fires this; only a
    /// present, disagreeing stamp does. Surfaces on boot output and
    /// `memstead health` without an include gate.
    EngineVersionSkew {
        mem: String,
        /// Engine version the last mutation was performed under.
        stamped_engine: String,
        /// Engine version of the running binary.
        running_engine: String,
        /// Resolved schema the last mutation validated against.
        stamped_schema: String,
        /// Which way the versions differ. Present because "they differ" left
        /// the reader to work out whether their binary was ahead of the mem
        /// or behind it, which is the only part that changes what they should
        /// do.
        direction: crate::build_info::SkewDirection,
    },
    /// Generation-behind hint: the mem's pinned schema resolved from
    /// the BUILT-IN catalogue and the catalogue registers at least
    /// one strictly-higher version of the same name (real semver
    /// ordering). Warn-tier, ungated, never blocking — retention
    /// seals every shipped version, so the pin keeps working; the
    /// hint names the newest available generation and the migration
    /// verb. Locally-installed (workspace-storage) pins are silent:
    /// the engine only knows generations for built-ins. Surfaces on
    /// boot output and `memstead health` without an include gate,
    /// like the skew hint above.
    SchemaGenerationsBehind {
        mem: String,
        /// The pinned ref (`name@version`).
        pinned: String,
        /// The newest built-in version registered under the same name.
        newest: String,
    },
    /// The mem was created on storage with no version control (a
    /// folder mount). Provenance means something WEAKER there than the
    /// headline "every mutation a reasoned commit": mutations ARE
    /// recorded — each lands in the folder backend's changelog ledger
    /// (`.memstead/changes.jsonl`) with its provenance note — but
    /// there are no commits, the `write_id` every mutation returns
    /// is a synthetic token rather than a commit and is not a change
    /// cursor (poll with the last ledger entry's `ts`), and the
    /// content is not durable until the surrounding repository
    /// commits it. Emitted once, at
    /// creation, to whoever is actually acting; never a refusal —
    /// folder mems are a supported storage class.
    FolderMemProvenance { mem: String },
    /// Authoring-drift health axis: a pinned schema's sealed copy
    /// carries an install-provenance stamp, and the authoring path it
    /// names is GONE from the working tree. Distinct from
    /// [`WarningHint::SchemaAuthoringSourceDiverged`] — a missing
    /// package and a diverged one need different actions. Only
    /// stamped schemas are checked: on git-branch workspaces the
    /// authoring folder is typically absent for unstamped seals, so a
    /// naive existence check would warn on healthy workspaces.
    SchemaAuthoringSourceMissing {
        schema_ref: String,
        stamped_path: String,
        mems: Vec<String>,
    },
    /// Authoring-drift health axis: the stamped authoring path exists
    /// but its package no longer parses EQUIVALENT to the sealed copy
    /// the engine runs on (parsed-schema comparison, never raw bytes —
    /// editor-header comment lines and serialisation reordering do not
    /// trip it). `detail` says how: a load failure's message, or the
    /// parsed-difference marker.
    SchemaAuthoringSourceDiverged {
        schema_ref: String,
        stamped_path: String,
        mems: Vec<String>,
        detail: String,
    },
    /// Low-tier rot axis for UNSTAMPED pins — distinct from the two
    /// stamped variants above, whose no-false-positive contract stays
    /// untouched. The pinned schema's sealed package still loads
    /// tolerantly (the mem runs fine), but its content no longer passes
    /// current-language AUTHORING validation — so the package is, as of
    /// the seal, no longer installable, and the (unstamped, therefore
    /// unlocatable) authoring source it was sealed from has rotted the
    /// same way unless someone has since fixed it. `detail` carries the
    /// authoring-tier load error. Remedy: re-author the package under
    /// the current language and `memstead schema install` it — which
    /// re-seals AND stamps, handing the check over to the divergence
    /// axis. An unstamped package that still parses under the authoring
    /// tier produces no hint.
    SchemaUnstampedSourceRot {
        schema_ref: String,
        mems: Vec<String>,
        detail: String,
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
    /// A mount resolved to nothing: the storage it names does not
    /// exist (`missing_ref` for a git-branch mount whose branch was
    /// never created or was deleted, `missing_path` for a folder or
    /// archive mount whose path is gone) or exists and holds no
    /// entity (`empty`). Before this warning a mount pointing at a
    /// nonexistent branch sat in the writable roster with zero
    /// entities and nothing said so (this project's own workspace carried two
    /// such mounts for weeks). Emitted at boot and on reload; a mount
    /// that resolves to at least one entity is silent. Lazy mounts are
    /// probed for storage presence only (the entity walk is deferred),
    /// so `empty` is reported for eager mounts.
    MountUnbacked {
        /// The mount's mem name.
        mem: String,
        /// Why it is unbacked.
        reason: MountUnbackedReason,
        /// What the mount names: the branch ref, the folder path or
        /// the archive path, for the operator's repair.
        location: String,
    },
    /// A mutation wrote a section whose emitted heading differs from a
    /// heading already present in the file that derives to the same
    /// section key. The write still commits — refusing would strand
    /// entities written before the round-trip gate existed — but the
    /// divergence is surfaced so the caller sees the file's heading
    /// text shifting under it (the regenerated file carries the
    /// schema's declared heading; the previous text is replaced).
    SectionHeadingDivergence {
        entity_id: EntityId,
        section_key: String,
        /// Heading the mutation is writing (the schema's declared one).
        writing_heading: String,
        /// Different heading the file carried for the same key.
        existing_heading: String,
    },
    /// A mem's resolved (already-installed) schema declares one or
    /// more sections whose heading does not derive back to its key —
    /// the condition new installs are refused for
    /// (`check_section_heading_roundtrip`). Sealed schemas keep
    /// loading by contract (refusing at boot would brick the
    /// workspace), so the violation surfaces here instead: every write
    /// against such a section forks its content into a second heading
    /// or the catch-all. Recovery: fix the schema's heading/key pairs
    /// and reinstall.
    SchemaHeadingRoundtripViolation {
        /// Mem whose pinned schema violates the rule.
        mem: String,
        /// The pinned `<name>@<version>`.
        schema_ref: String,
        /// Every offending `(type, key, heading, derived_key)` tuple.
        violations: Vec<SchemaHeadingViolation>,
    },
}

/// Wire-shape entry inside `SchemaHeadingRoundtripViolation.violations`
/// — one section whose declared heading does not derive back to its
/// declared key. Mirrors `memstead_schema::HeadingKeyViolation`, kept
/// as a local struct so the warning's JSON shape is owned here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaHeadingViolation {
    pub type_name: String,
    pub key: String,
    pub heading: String,
    pub derived_key: String,
}

impl From<&memstead_schema::HeadingKeyViolation> for SchemaHeadingViolation {
    fn from(v: &memstead_schema::HeadingKeyViolation) -> Self {
        Self {
            type_name: v.type_name.clone(),
            key: v.key.clone(),
            heading: v.heading.clone(),
            derived_key: v.derived_key.clone(),
        }
    }
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
    /// The block's declared severity. Serialized only for `block` —
    /// warn is the default the vocabulary has always had, and existing
    /// consumers keep their byte-identical `{ relationships,
    /// cardinality }` shape.
    #[serde(skip_serializing_if = "severity_is_warn")]
    pub severity: memstead_schema::ConstraintSeverity,
    /// The condition that armed a conditional block (`when_field` /
    /// `when_value` on the declaration). Serialized only when present,
    /// so unconditional blocks keep their byte-identical shape — and
    /// the reader of a refusal, warning, or health finding sees which
    /// trigger armed the obligation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_field: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_value: Option<String>,
}

fn severity_is_warn(s: &memstead_schema::ConstraintSeverity) -> bool {
    *s == memstead_schema::ConstraintSeverity::Warn
}

/// Abstract recovery action attached to a `PARSED_RELATION_INVALID`
/// warning when the source mem is writable. The shape is tool-
/// agnostic: it names *what* to do, not *which tool* to call. A
/// consumer (agent, bulk-fix orchestrator, app surface) maps `kind`
/// to the concrete call on whichever MCP / CLI path it
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
/// across the MCP and CLI surfaces; the renderer chooses the
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
    /// The backend's identity for the last successful per-source
    /// re-render the bulk-fix performed — a commit SHA on a git-branch
    /// mem, a synthetic token on a folder mem, and never a change
    /// cursor. Empty when no recovery wrote to disk
    /// (workspace already clean, only read-only warnings, or every
    /// writable attempt failed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub write_id: String,
}

/// Closed vocabulary of [`WarningHint::MountUnbacked`] reasons, serialised
/// as the lowercase `details.reason` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountUnbackedReason {
    /// Git-branch mount: the branch ref does not exist.
    MissingRef,
    /// Folder or archive mount: the path does not exist.
    MissingPath,
    /// The storage exists and holds no entity.
    Empty,
}

impl MountUnbackedReason {
    /// The wire value (`missing_ref` / `missing_path` / `empty`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingRef => "missing_ref",
            Self::MissingPath => "missing_path",
            Self::Empty => "empty",
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
