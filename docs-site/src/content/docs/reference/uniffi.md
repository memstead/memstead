---
title: "UniFFI surface"
---

# UniFFI surface

Auto-generated from the engine's UniFFI UDL. Each top-level declaration (`namespace`, `dictionary`, `interface`, `[Enum]` interface, `[Error]` interface) appears below with its full body and any preceding doc-comment block.

## `namespace memstead`

UniFFI interface for the macOS app.
Read-only surface: input records (MemInit, SearchScope),
response records (Entity, Stats, HealthSummary, List/Search/Relations/
Cluster/Path results), the error enum, and the Engine interface with
method signatures. Mutation FFI is deferred — kept as a UniFFI
`interface` (not a dictionary) so mutation methods can land later
without breaking the existing bindings.

```idl
namespace memstead {
    string version();

    // Enumerate the workspace's mems by walking the `mem-repo/.git/`
    // branch list (excluding `main` and registry refs), one `MemInit`
    // per mem. The Swift app calls this before constructing the Engine
    // to seed its UI; the Engine itself loads its mounts from
    // `.memstead/state/mounts.json` and does not consume this list.
    sequence<MemInit> discover_mems(string project_root);

    // Construct an engine rooted at an explicit `workspace_root` (a directory
    // containing `.memstead/workspace.toml`). Cwd-independent — unlike the
    // bare `Engine()` constructor, which falls back to the process cwd.
    // Backend-agnostic: folder and git-branch mounts open through this one
    // call with no caller-side branching. Throws a typed, actionable error
    // when the path has no recognised workspace layout.
    [Throws=MemsteadError]
    Engine engine_open(string workspace_root);

    // Initialise a brand-new filesystem (folder-backed) mem at `root` — the
    // engine-owned bootstrap the macOS app routes through instead of writing
    // `.memstead/config.json` itself. Writes the seed config + adapter marker +
    // one-folder-mount roster, so the result roots through `engine_open`. The
    // mem root is the workspace root (collapsed single-mem form).
    [Throws=MemsteadError]
    void init_filesystem_mem(string root, string name, string schema);

    // Render a binding's run-brief — the Markdown prompt a build agent
    // consumes — for the binding `binding_id` (the canonical `<mem>/<stem>`
    // slash form, D3) in the workspace at `workspace_root`. Byte-identical to
    // `memstead projection brief <binding>`: both call the one shared engine
    // entry point. Discovery mode today.
    [Throws=MemsteadError]
    string projection_brief(string workspace_root, string binding_id);
};
```

## `[Error] interface MemsteadError`

Error — surfaced as a throwing Swift API.
Maps memstead_base::EngineError onto a smaller Swift-facing set. Most
validation kinds collapse into `ValidationFailed { message }`; the two
cases the macOS app actually branches on (`UnknownSection`,
`UnknownMem`) get their own variants carrying the declared lists and
a suggestion so the UI can render a picker without re-parsing messages.

```idl
interface MemsteadError {
    NotFound(string message);
    ValidationFailed(string message);
    HashMismatch(string message, string current);
    SchemaError(string message);
    IoError(string message);
    Internal(string message);
    UnknownSection(string key, string entity_type, sequence<string> declared, string? suggestion);
    UnknownMem(string name, sequence<string> writable_mems);
    // A branch reset refused because the target would discard commits
    // already pushed to a remote — the one guard the human surface must
    // render distinctly (its reason names the protected SHAs).
    PushedCommitsProtected(string message, sequence<string> pushed_shas);
};
```

## `dictionary MemInit`

Input records — Engine bootstrap + query options.

```idl
dictionary MemInit {
    string name;
    string dir;
    string schema_name;
    string schema_version;
};
```

## `dictionary Query`

Flat query shape — mirrors `memstead_base::ops::Query`. All fields optional
so callers can pass a partial predicate; an empty Query (or `None`) makes
`search` behave as a metadata-only filter.

```idl
dictionary Query {
    sequence<string> any_of;
    sequence<string> not_in;
    string? phrase;
    string? field;
};
```

## `dictionary SearchScope`

```idl
dictionary SearchScope {
    Query? query;
    string? mem;
    string? entity_type;
    u32? limit;
    u32? offset;
    record<DOMString, string> filters;
    record<DOMString, string> range_filters;
    string? edge_type;
    string? related_to;
    u32? depth;
    boolean? stub;
    sequence<string>? expand_via;
    u32? expand_depth;
};
```

## `[Enum] interface MetadataValue`

Entity records. MetadataValue mirrors the engine's four variants; metadata
is a flat sequence of (key, value) pairs to preserve insertion order
across the FFI (UniFFI maps don't guarantee ordering).

```idl
interface MetadataValue {
    BoolValue(boolean value);
    IntValue(i64 value);
    FloatValue(double value);
    StringValue(string value);
};
```

## `dictionary MetadataEntry`

```idl
dictionary MetadataEntry {
    string key;
    MetadataValue value;
};
```

## `dictionary Section`

```idl
dictionary Section {
    string key;
    string content;
};
```

## `dictionary Relationship`

```idl
dictionary Relationship {
    string rel_type;
    string target;
    string? description;
};
```

## `dictionary Entity`

```idl
dictionary Entity {
    string id;
    string title;
    string entity_type;
    string mem;
    string file_path;
    sequence<MetadataEntry> metadata;
    sequence<Section> sections;
    sequence<Relationship> relationships;
    string content_hash;
    boolean stub;
};
```

## `dictionary EdgeTypeCount`

Status. edge_types is a flat sequence rather than a map for Identifiable
SwiftUI list rendering and stable sort on the Swift side.

```idl
dictionary EdgeTypeCount {
    string rel_type;
    u64 count;
};
```

## `dictionary Status`

The status payload (D11: `stats` → `status`). Every field is preserved from
the former `Stats` dictionary — the rename-preserving floor keeps the macOS
app's data source unchanged, deferring the `mem_roster` + `get_health`
rework to the editor-UI release.
`stub_count` (and therefore `real_count = entity_count - stub_count`) is a
UI staple the MCP surface also exposes (as top-level fields on
`memstead_health`'s default response); carrying it on Status avoids a second
`get_health()` round-trip just to display the entity/stub split.
`writable_mems` + `read_mems` mirror the same fields on
`memstead_health`'s default response so the macOS app can render the mem
list from one call (write mems come from the router; read mems are
the visible set minus the writable set).

```idl
dictionary Status {
    u64 entity_count;
    u64 stub_count;
    u64 edge_count;
    sequence<EdgeTypeCount> edge_types;
    u64 community_count;
    u64 mem_count;
    sequence<string> types_in_use;
    sequence<string> writable_mems;
    sequence<string> read_mems;
};
```

## `dictionary HealthIssue`

Health.

```idl
dictionary HealthIssue {
    string field;
    string message;
};
```

## `dictionary MissingField`

```idl
dictionary MissingField {
    string id;
    string title;
    f32 score;
    sequence<HealthIssue> issues;
};
```

## `dictionary StaleEntity`

```idl
dictionary StaleEntity {
    string id;
    string title;
    u64 days_since_modified;
};
```

## `dictionary HealthFinding`

One per-entity integrity finding: the conformance axis (which entities a
write would refuse under the effective schema, and why) or the
consistency axis (DANGLING_LINK, ORPHAN_STUB). `detail_json` carries the
engine's structured detail (schema context: type, field, allowed values)
rendered as JSON — the same payload the MCP surface ships.

```idl
dictionary HealthFinding {
    string id;
    string mem;
    string axis;
    string code;
    string detail_json;
};
```

## `dictionary HealthSummary`

```idl
dictionary HealthSummary {
    sequence<StaleEntity> stale_entities;
    sequence<MissingField> missing_fields;
    u64 orphan_count;
    u64 stub_count;
    // Additive widening (macos-cockpit): conformance + consistency findings
    // per mounted mem, plus the orphan entity ids behind `orphan_count` so
    // the app can list them. Pre-existing fields are unchanged.
    sequence<HealthFinding> findings;
    sequence<string> orphan_ids;
    // A mem whose findings collector errored is reported here, never
    // silently rendered clean.
    sequence<string> collector_warnings;
};
```

## `dictionary SearchHit`

List / search.

```idl
dictionary SearchHit {
    string id;
    string title;
    string mem;
    string entity_type;
    boolean stub;
    f32 score;
    u64 tokens;
    string? snippet;
    sequence<Section> sections;
};
```

## `dictionary SearchResult`

```idl
dictionary SearchResult {
    u64 total;
    u64 returned;
    u64 offset;
    sequence<SearchHit> hits;
    sequence<string> warnings;
};
```

## `dictionary ListResult`

```idl
dictionary ListResult {
    u64 total;
    u64 returned;
    u64 offset;
    u64 total_tokens;
    sequence<SearchHit> hits;
    sequence<string> warnings;
};
```

## `dictionary RelationEdge`

```idl
dictionary RelationEdge {
    string rel_type;
    string other_id;
    string other_title;
    string other_entity_type;
    RelationDirection direction;
    EdgeSource source;
};
```

## `dictionary Relations`

```idl
dictionary Relations {
    string entity_id;
    sequence<RelationEdge> outgoing;
    sequence<RelationEdge> incoming;
};
```

## `dictionary ClusterInfo`

Communities.

```idl
dictionary ClusterInfo {
    string id;
    sequence<string> entities;
};
```

## `dictionary BranchResetOutcome`

Reload diff.
Successful outcome of `branch_reset` — what moved and which commits
the reset discarded (all guaranteed unpushed by the safety probe).

```idl
dictionary BranchResetOutcome {
    string mem;
    string branch_ref;
    string previous_sha;
    string new_sha;
    sequence<string> discarded_commits;
};
```

## `dictionary StrandedCrossMemRef`

One inbound cross-mem reference a reset would strand — computed fresh
by `branch_reset_stranded_refs` at confirmation time.

```idl
dictionary StrandedCrossMemRef {
    string from_id;
    string from_mem;
    string to_id;
    string rel_type;
};
```

## `dictionary ReloadResult`

```idl
dictionary ReloadResult {
    sequence<string> added;
    sequence<string> changed;
    sequence<string> removed;
};
```

## `[Enum] interface ChangeEnvelope`

Per-mem commit-delta + agent-notes feed. Mirrors the MCP /
`memstead-cli` surface (`memstead_changes_since`, `memstead changes` —
`--include-notes` for the notes branch, `memstead_health.mems[].vcs.head`
for the per-mem head) so the macOS app has parity with what the
Claude Code plugin and operator CLI consume.

```idl
interface ChangeEnvelope {
    Added(string id, string? title, string? entity_type);
    Updated(string id, string? title, string? entity_type);
    Removed(string id, string? title, string? entity_type);
    Renamed(string from_id, string to_id, string? title, string? entity_type);
};
```

## `dictionary CommitNote`

```idl
dictionary CommitNote {
    string mem;
    string sha;
    string subject;
    string? tool_verb;
    string? entity_id;
    string? note;
    string? actor;
    string? tool;
    string? client;
    i64 timestamp;
};
```

## `dictionary ChangesReport`

```idl
dictionary ChangesReport {
    string mem;
    string since;
    string head;
    sequence<ChangeEnvelope> changes;
};
```

## `dictionary AgentNotesReport`

```idl
dictionary AgentNotesReport {
    string mem;
    string since;
    string head;
    sequence<CommitNote> notes;
    // SHA of `refs/heads/__MEMSTEAD` — unified schemas + per-mem
    // configs branch. `None` when the workspace has not been
    // migrated to the unified layout yet.
    string? memstead_ref;
};
```

## `dictionary IncomingRipple`

Two-ref structural diff (`Engine::diff` / `memstead_diff`). Read-only,
git-branch mems only — the same wire shape MCP and the CLI serve, exposed
in-process. `content_before` / `content_after` are populated only when the
caller passes `include_content: true`; `ripple` only when `include_ripple`.

```idl
dictionary IncomingRipple {
    string from_id;
    // `"ref_a"` or `"ref_b"` — which side the referrer lives on.
    string side;
    string? section;
};
```

## `[Enum] interface EntityDiff`

```idl
interface EntityDiff {
    Added(string id, string? title, string? entity_type, string? content_after, sequence<IncomingRipple> ripple);
    Modified(string id, string? title, string? entity_type, string? content_before, string? content_after, sequence<IncomingRipple> ripple);
    Deleted(string id, string? title, string? entity_type, string? content_before, sequence<IncomingRipple> ripple);
    Renamed(string from_id, string to_id, sequence<string> rename_chain, string? title, string? entity_type, string? content_before, string? content_after, sequence<IncomingRipple> ripple);
    InvalidEntity(string id, string side, string error, string? content_before, string? content_after);
};
```

## `dictionary DiffConfig`

```idl
dictionary DiffConfig {
    f32 rename_similarity;
    boolean include_content;
    boolean include_ripple;
};
```

## `dictionary Diff`

```idl
dictionary Diff {
    string ref_a;
    string ref_b;
    string resolved_a_sha;
    string resolved_b_sha;
    DiffConfig config;
    sequence<EntityDiff> entries;
};
```

## `dictionary ParseRecoveryEntry`

```idl
dictionary ParseRecoveryEntry {
    string entity_id;
    string rel_type;
    string target;
    // "removed" (recovered), "skipped" (read-only origin), or "failed".
    string outcome;
    // Skip reason or the underlying engine error code; `None` for
    // "removed" entries.
    string? reason;
};
```

## `dictionary ParseRecoveryReport`

```idl
dictionary ParseRecoveryReport {
    sequence<ParseRecoveryEntry> entries;
    // SHA of the last per-source re-render commit; `None` when the
    // workspace was already clean (nothing rewritten).
    string? commit_sha;
};
```

## `dictionary MemCreateRequest`

Mem lifecycle — create / delete / set-schema / set-version through the
engine. Mirrors the `memstead_mem_*` MCP contract onto the in-process
binding so the macOS roster manages mems without any mem-repo mutation
(git ops, raw `config.json`/`.md` writes, `.git` introspection) originating
in Swift. Export-as-`.mem` is a separate, different-shaped task — not here.

```idl
dictionary MemCreateRequest {
    string name;
    string location;
    string schema;
    string? vcs_gitdir;
    string? vcs_worktree;
    string? note;
    boolean operator_mode;
};
```

## `dictionary MemCreateOutcome`

```idl
dictionary MemCreateOutcome {
    string name;
    string location;
    string schema_ref;
    string seed_commit_sha;
    sequence<string> warnings;
};
```

## `dictionary MemDeleteOutcome`

```idl
dictionary MemDeleteOutcome {
    string name;
    boolean deleted_from_router;
    boolean files_deleted;
    sequence<string> warnings;
};
```

## `dictionary MemSchemaOutcome`

```idl
dictionary MemSchemaOutcome {
    string mem;
    string schema_pin;
    string? migration_target;
    string outcome;
    sequence<string> findings;
};
```

## `dictionary MemVersionOutcome`

```idl
dictionary MemVersionOutcome {
    string mem;
    string? old_version;
    string new_version;
    sequence<string> warnings;
};
```

## `dictionary MemRosterEntry`

```idl
dictionary MemRosterEntry {
    string mem;
    MemBackendKind backend;
    boolean writable;
    string? schema_pin;
    u64 entity_count;
    boolean drifted;
};
```

## `dictionary DanglingCrossMemEdge`

```idl
dictionary DanglingCrossMemEdge {
    string entity_path;
    string target_id;
    string target_mem;
};
```

## `dictionary MemExportOutcome`

```idl
dictionary MemExportOutcome {
    string archive_path;
    string name;
    string version;
    u64 entity_count;
    u64 size_bytes;
    sequence<DanglingCrossMemEdge> dangling_cross_mem_edges;
};
```

## `interface Engine`

Engine interface. Read/query/sync operations plus the first
disk-mutating method, `apply_parse_recovery` (parity with `memstead
recover`). Kept as a UniFFI `interface` (not a dictionary) so further
mutation methods can land without breaking the existing bindings.

```idl
interface Engine {
    [Throws=MemsteadError]
    constructor();

    Status get_status();
    HealthSummary get_health();
    ListResult list_entities(SearchScope scope);
    SearchResult search(SearchScope scope);
    Entity? get_entity(string id);

    [Throws=MemsteadError]
    Relations get_relations(string id);

    sequence<ClusterInfo> get_overview(boolean rebuild);

    // The workspace mem roster — one entry per mounted mem, with the
    // per-mount facts (backend kind, capability, schema pin, entity count,
    // drift) the home sidebar renders. Every fact comes from the engine,
    // so the roster never reconstructs backend/capability from a Swift-side
    // read of `mounts.json`. Cannot fail (per-mem drift probes that error
    // collapse to `false`).
    sequence<MemRosterEntry> mem_roster();

    [Throws=MemsteadError]
    ReloadResult reload();

    // Cached branch-tip SHA for a writable mem. `None` for fresh
    // mems with no commits yet — consumers substitute the canonical
    // empty-tree sentinel `4b825dc642cb6eb9a060e54bf8d69288fbee4904`.
    [Throws=MemsteadError]
    string? mem_head_sha(string mem);

    // Reset a git-branch mem to `target_sha` — the human surface's
    // guarded rewind (CLI parity; deliberately NOT on the MCP wire).
    // Refuses with PushedCommitsProtected when the target would discard
    // pushed commits, UnknownRef for an unresolvable target.
    // `expected_head`: optimistic-concurrency guard — the head the caller
    // observed (a review span's end, the head at preview time). A live
    // head that moved past it refuses (HashMismatch carrying the live
    // head) instead of discarding foreign commits.
    [Throws=MemsteadError]
    BranchResetOutcome branch_reset(string mem, string target_sha, string? expected_head);

    // Cross-mem references a reset would strand — a read, computed fresh
    // at confirmation-dialog time.
    [Throws=MemsteadError]
    sequence<StrandedCrossMemRef> branch_reset_stranded_refs(string mem, string target_sha);

    // Two-tree diff between `since` and the mem's current HEAD. See
    // `memstead_changes_since` for semantics. `rename_similarity` mirrors
    // the MCP knob (default 0.6 when `None`).
    [Throws=MemsteadError]
    ChangesReport changes_since(string mem, string since, f32? rename_similarity);

    // Two-ref structural diff for one mem — exposes the engine's existing
    // `Engine::diff` (the `memstead_diff` MCP surface). Read-only,
    // git-branch mems only. Optional knobs mirror the MCP defaults:
    // `include_content`/`include_ripple` default true, `rename_similarity`
    // defaults to 0.6 when `None`.
    [Throws=MemsteadError]
    Diff diff(string mem, string ref_a, string ref_b, boolean? include_content, boolean? include_ripple, f32? rename_similarity);

    // Walk per-commit agent-notes between `since` and HEAD. See the
    // engine's `Engine::agent_notes`. The response also carries the
    // workspace-level `__MEMSTEAD` ref tip (unified schemas + per-mem
    // configs).
    [Throws=MemsteadError]
    AgentNotesReport agent_notes(string mem, string since);

    // Bulk-fix accumulated parse-time relation drift across every
    // writable mem — the UniFFI counterpart to `memstead recover`.
    // First disk-mutating UniFFI method; provenance mirrors the CLI's
    // `Actor::Cli`. `note` lands on each per-source re-render commit.
    [Throws=MemsteadError]
    ParseRecoveryReport apply_parse_recovery(string? note);

    // Pipeline edits (medium / facet / projection). The macOS pipeline editor
    // routes here instead of hand-writing `.memstead/` JSON. Create/update
    // carry the primitive as a JSON string (Facet's free-form `engagement`
    // field rules out a typed record); delete/rename take identifiers.
    // Referential integrity and snapshot refresh live in the engine. See
    // `memstead_base::pipeline_edit`.
    //
    // The **binding** is the unit (D1/D14): `add_projection` / `update_projection`
    // carry a JSON **patch over the full author-editable binding record** —
    // intent / source_facets / reference_mems / destination_mem / deny_paths /
    // coverage_semantics / rules / prune / the whole `operations` block. Patch
    // semantics: an absent field is preserved from the stored record; explicit
    // `null` clears intent / rules / prune; a present `operations` block
    // replaces the block as a unit. `version` is engine-managed (ignored in the
    // payload, preserved on update), and unknown additive keys are tolerated.
    // Candidate records are validated against the medium-capability matrix
    // before anything is written; only refusals the edit *introduces* block it
    // (typed ValidationFailed, message + remedy verbatim from the engine).
    // There is no separate ingest CRUD: the flat ingest record died, and
    // operations-block edits ride this one update seam.
    //
    // Every edit accepts an optional provenance note. Git-branch
    // workspaces commit the edit's mirror to __MEMSTEAD with the note on
    // the commit body; folder workspaces accept and drop it (no commit
    // timeline exists) — the same posture as set_mem_version.
    [Throws=MemsteadError]
    void add_medium(string mem, string name, string medium_json, string? note);
    [Throws=MemsteadError]
    void update_medium(string mem, string name, string medium_json, string? note);
    [Throws=MemsteadError]
    void delete_medium(string mem, string name, string? note);
    [Throws=MemsteadError]
    void rename_medium(string mem, string old_name, string new_name, string? note);

    [Throws=MemsteadError]
    void add_facet(string mem, string name, string facet_json, string? note);
    [Throws=MemsteadError]
    void update_facet(string mem, string name, string facet_json, string? note);
    [Throws=MemsteadError]
    void delete_facet(string mem, string name, string? note);
    [Throws=MemsteadError]
    void rename_facet(string mem, string old_name, string new_name, string? note);

    [Throws=MemsteadError]
    void add_projection(string mem, string name, string projection_json, string? note);
    [Throws=MemsteadError]
    void update_projection(string mem, string name, string projection_json, string? note);
    [Throws=MemsteadError]
    void delete_projection(string mem, string name, string? note);
    [Throws=MemsteadError]
    void rename_projection(string mem, string old_name, string new_name, string? note);

    // Resolved schema for a mem — the same JSON wire shape the MCP
    // memstead_schema tool serves (single-sourced builder). Typed
    // NotFound for an unknown mem or an unresolvable pin (message names
    // the pin so the app renders the resolution error honestly).
    [Throws=MemsteadError]
    string schema_json(string mem);

    // Whether the workspace's mutation policy requires provenance notes
    // ([mutations].require_notes). The app reads this to collect a note
    // up front and refuse an empty one with the policy named — the
    // engine itself only nudges (NOTE_MISSING warning), never blocks.
    boolean workspace_requires_notes();

    // Read the pipeline store as JSON (the edit methods' read counterpart).
    // The macOS pipeline editor deserializes this to display the store. Shape
    // (D14): `{ mediums:[{mem,name,config}], facets:[...], bindings:[{mem,name,
    // config}] }` — the v1 binding shape; `config` carries the binding's
    // `operations` block. The `ingests` key is gone. Cannot fail.
    string pipeline_configs_json();

    // Mem lifecycle. The macOS roster routes create/delete/set-schema/
    // set-version here instead of mutating the mem-repo from Swift; the
    // engine owns backend instantiation, allowlist gating, policy scrub,
    // and the seed/version commits. See `memstead_engine::mem_management`
    // and `memstead_base::Engine::{set_mem_schema, set_mem_version}`.
    [Throws=MemsteadError]
    MemCreateOutcome create_mem(MemCreateRequest request);
    [Throws=MemsteadError]
    MemDeleteOutcome delete_mem(string name, string? note, boolean operator_mode);
    [Throws=MemsteadError]
    MemSchemaOutcome set_mem_schema(string mem, string schema);
    [Throws=MemsteadError]
    MemVersionOutcome set_mem_version(string mem, string version, string? note);

    // Export a mem as a portable `.mem` archive at `output_path`.
    // Backend-symmetric (folder snapshot / git-branch history); archive
    // mounts refuse (sealed), in-memory mounts refuse (unsupported). The
    // engine owns the export — Swift never walks the mem directory. See
    // `memstead_base::Engine::export_mem`.
    [Throws=MemsteadError]
    MemExportOutcome export_mem(string mem, string output_path);
};
```

