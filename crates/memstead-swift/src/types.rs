//! FFI-mirror types for the UDL surface.
//!
//! Each struct/enum here corresponds to one UDL dictionary/enum. Field names
//! and ordering must match `src/memstead.udl` exactly — UniFFI relies on the
//! positional layout when generating the scaffolding.
//!
//! These are flat wrappers over `memstead-base` types. Keeping them in their own
//! module avoids bending the engine to accommodate FFI constraints
//! (non-FFI-safe `Arc<TypeDefinition>`, `IndexMap`, `HashMap<String, usize>`,
//! nested `EntityId` newtypes).

// ---------------------------------------------------------------------------
// Input records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MemInit {
    pub name: String,
    pub dir: String,
    pub schema_name: String,
    pub schema_version: String,
}

/// FFI-mirror of `memstead_base::ops::Query`. `any_of` / `not_in` are the FFI
/// names for the core `any` / `not` fields (both are Swift keywords).
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub any_of: Vec<String>,
    pub not_in: Vec<String>,
    pub phrase: Option<String>,
    pub field: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SearchScope {
    pub query: Option<Query>,
    pub mem: Option<String>,
    pub entity_type: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub filters: std::collections::HashMap<String, String>,
    pub range_filters: std::collections::HashMap<String, String>,
    pub edge_type: Option<String>,
    pub related_to: Option<String>,
    pub depth: Option<u32>,
    pub stub: Option<bool>,
    pub expand_via: Option<Vec<String>>,
    pub expand_depth: Option<u32>,
}

// ---------------------------------------------------------------------------
// Entity records
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum MetadataValue {
    BoolValue { value: bool },
    IntValue { value: i64 },
    FloatValue { value: f64 },
    StringValue { value: String },
}

#[derive(Debug, Clone)]
pub struct MetadataEntry {
    pub key: String,
    pub value: MetadataValue,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub key: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct Relationship {
    pub rel_type: String,
    pub target: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Entity {
    pub id: String,
    pub title: String,
    pub entity_type: String,
    pub mem: String,
    pub file_path: String,
    pub metadata: Vec<MetadataEntry>,
    pub sections: Vec<Section>,
    pub relationships: Vec<Relationship>,
    pub content_hash: String,
    pub stub: bool,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct EdgeTypeCount {
    pub rel_type: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct Stats {
    pub entity_count: u64,
    pub stub_count: u64,
    pub edge_count: u64,
    pub edge_types: Vec<EdgeTypeCount>,
    pub community_count: u64,
    pub mem_count: u64,
    pub types_in_use: Vec<String>,
    pub writable_mems: Vec<String>,
    pub read_mems: Vec<String>,
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HealthIssue {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct MissingField {
    pub id: String,
    pub title: String,
    pub score: f32,
    pub issues: Vec<HealthIssue>,
}

#[derive(Debug, Clone)]
pub struct StaleEntity {
    pub id: String,
    pub title: String,
    pub days_since_modified: u64,
}

/// One per-entity integrity finding crossing the FFI: the conformance axis
/// (schema lint — what a write would refuse and why) or the consistency
/// axis (`DANGLING_LINK`, `ORPHAN_STUB`). `detail_json` is the engine's
/// structured detail rendered as JSON, carrying the schema context (type,
/// field, allowed values) the app surfaces per finding.
#[derive(Debug, Clone)]
pub struct HealthFinding {
    pub id: String,
    pub mem: String,
    pub axis: String,
    pub code: String,
    pub detail_json: String,
}

#[derive(Debug, Clone)]
pub struct HealthSummary {
    pub stale_entities: Vec<StaleEntity>,
    pub missing_fields: Vec<MissingField>,
    pub orphan_count: u64,
    pub stub_count: u64,
    /// Additive widening: integrity findings per mounted mem. Empty when
    /// every mounted mem lints clean.
    pub findings: Vec<HealthFinding>,
    /// The entity ids behind `orphan_count`, so orphans are listable, not
    /// just countable.
    pub orphan_ids: Vec<String>,
    /// Per-mem collector failures ("mem 'x': conformance findings
    /// unavailable: …") — a broken collector is reported, never rendered
    /// as a clean mem.
    pub collector_warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// List / search
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub mem: String,
    pub entity_type: String,
    pub stub: bool,
    pub score: f32,
    pub tokens: u64,
    pub snippet: Option<String>,
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub total: u64,
    pub returned: u64,
    pub offset: u64,
    pub hits: Vec<SearchHit>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ListResult {
    pub total: u64,
    pub returned: u64,
    pub offset: u64,
    pub total_tokens: u64,
    pub hits: Vec<SearchHit>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------------------
// Relations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum RelationDirection {
    Outgoing,
    Incoming,
}

#[derive(Debug, Clone)]
pub enum EdgeSource {
    Explicit,
    Hierarchy,
    BodyLink,
}

#[derive(Debug, Clone)]
pub struct RelationEdge {
    pub rel_type: String,
    pub other_id: String,
    pub other_title: String,
    pub other_entity_type: String,
    pub direction: RelationDirection,
    pub source: EdgeSource,
}

#[derive(Debug, Clone)]
pub struct Relations {
    pub entity_id: String,
    pub outgoing: Vec<RelationEdge>,
    pub incoming: Vec<RelationEdge>,
}

// ---------------------------------------------------------------------------
// Communities
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ClusterInfo {
    pub id: String,
    pub entities: Vec<String>,
}

// ---------------------------------------------------------------------------
// Reload
// ---------------------------------------------------------------------------

/// FFI mirror of `memstead_base::ops::BranchResetOutcome`.
#[derive(Debug, Clone)]
pub struct BranchResetOutcome {
    pub mem: String,
    pub branch_ref: String,
    pub previous_sha: String,
    pub new_sha: String,
    pub discarded_commits: Vec<String>,
}

/// FFI mirror of `memstead_base::ops::StrandedCrossMemRef`.
#[derive(Debug, Clone)]
pub struct StrandedCrossMemRef {
    pub from_id: String,
    pub from_mem: String,
    pub to_id: String,
    pub rel_type: String,
}

#[derive(Debug, Clone)]
pub struct ReloadResult {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

// ---------------------------------------------------------------------------
// Per-mem commit-delta + agent-notes feed (UniFFI catch-up to MCP/CLI).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ChangeEnvelope {
    Added {
        id: String,
        title: Option<String>,
        entity_type: Option<String>,
    },
    Updated {
        id: String,
        title: Option<String>,
        entity_type: Option<String>,
    },
    Removed {
        id: String,
        title: Option<String>,
        entity_type: Option<String>,
    },
    Renamed {
        from_id: String,
        to_id: String,
        title: Option<String>,
        entity_type: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct CommitNote {
    pub mem: String,
    pub sha: String,
    pub subject: String,
    pub tool_verb: Option<String>,
    pub entity_id: Option<String>,
    pub note: Option<String>,
    pub actor: Option<String>,
    pub tool: Option<String>,
    pub client: Option<String>,
    pub timestamp: i64,
}

#[derive(Debug, Clone)]
pub struct ChangesReport {
    pub mem: String,
    pub since: String,
    pub head: String,
    pub changes: Vec<ChangeEnvelope>,
}

#[derive(Debug, Clone)]
pub struct AgentNotesReport {
    pub mem: String,
    pub since: String,
    pub head: String,
    pub notes: Vec<CommitNote>,
    pub memstead_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// Parse-recovery (first disk-mutating UniFFI surface — parity with the CLI's
// `memstead recover`).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ParseRecoveryEntry {
    pub entity_id: String,
    pub rel_type: String,
    pub target: String,
    /// `"removed"` (recovered), `"skipped"` (read-only origin), or
    /// `"failed"`.
    pub outcome: String,
    /// Skip reason or the underlying engine error code; `None` for
    /// `"removed"` entries.
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ParseRecoveryReport {
    pub entries: Vec<ParseRecoveryEntry>,
    /// SHA of the last per-source re-render commit; `None` when the
    /// workspace was already clean (nothing rewritten).
    pub commit_sha: Option<String>,
}

// ---------------------------------------------------------------------------
// Mem lifecycle (create / delete / set-schema / set-version). Mirrors the
// `memstead_mem_*` MCP contract onto the in-process binding so the macOS
// roster manages mems through the engine — no mem-repo mutation (git
// ops, raw `config.json` / `.md` writes, `.git` introspection) originates in
// Swift. Export-as-`.mem` is a separate, different-shaped task and is not
// bound here.
// ---------------------------------------------------------------------------

/// Input for [`crate::Engine::create_mem`]. Mirrors `MemCreateParams`
/// minus the agent-only knobs the desktop operator never sets
/// (`recovery`, `include_schema`, `write_guidance`); those keep their
/// engine defaults. The optional VCS override is split into two scalar
/// fields rather than a nested record so the UniFFI shape stays flat;
/// set both or neither.
#[derive(Debug, Clone)]
pub struct MemCreateRequest {
    /// Full hierarchical identifier (`"sub-mem"` or `"team/sub-mem"`).
    pub name: String,
    /// Absolute path, or relative to the workspace root.
    pub location: String,
    /// Schema pin, `name@x.y.z` (e.g. `default@1.0.0`).
    pub schema: String,
    /// Gitdir of an optional VCS layout override, relative to the new
    /// mem's root. `None` uses the engine's isolated default. When set,
    /// pair with `vcs_worktree`.
    pub vcs_gitdir: Option<String>,
    /// Worktree of the optional VCS layout override. Ignored unless
    /// `vcs_gitdir` is also set; defaults to `"."` (mem root).
    pub vcs_worktree: Option<String>,
    /// Provenance note recorded in the seed commit (≤280 chars).
    pub note: Option<String>,
    /// When `true`, skip the `[[mem_management.create]]` allowlist gate
    /// (operator intent). `false` runs the gate so workspace policy
    /// refusals surface as typed errors.
    pub operator_mode: bool,
}

/// Outcome of [`crate::Engine::create_mem`].
#[derive(Debug, Clone)]
pub struct MemCreateOutcome {
    pub name: String,
    pub location: String,
    /// Canonical settled schema pin (`name@x.y.z`).
    pub schema_ref: String,
    /// Seed-commit cursor for `changes_since` polling; empty on the
    /// reattach branch (paired with a warning).
    pub seed_commit_sha: String,
    /// Non-fatal findings (e.g. `MEM_REATTACHED_AFTER_UNREGISTER`),
    /// rendered to display strings.
    pub warnings: Vec<String>,
}

/// Outcome of [`crate::Engine::delete_mem`].
#[derive(Debug, Clone)]
pub struct MemDeleteOutcome {
    pub name: String,
    pub deleted_from_router: bool,
    pub files_deleted: bool,
    /// Non-fatal cleanup findings, rendered to display strings.
    pub warnings: Vec<String>,
}

/// Outcome of [`crate::Engine::set_mem_schema`]. Flattens the engine's
/// `SetSchemaOutcome`; `findings` carries the ids of entities not yet
/// integral against the target while a migration is in progress.
#[derive(Debug, Clone)]
pub struct MemSchemaOutcome {
    pub mem: String,
    /// Settled pin after this call (`name@x.y.z`).
    pub schema_pin: String,
    /// In-flight migration target while a migration is in progress.
    pub migration_target: Option<String>,
    /// `noop` | `switched` | `migration_started` | `migration_pending`.
    pub outcome: String,
    /// Ids of entities not yet integral against the target; empty unless
    /// a migration is in progress.
    pub findings: Vec<String>,
}

/// Outcome of [`crate::Engine::set_mem_version`].
#[derive(Debug, Clone)]
pub struct MemVersionOutcome {
    pub mem: String,
    /// Previous version, or `None` when the config carried none.
    pub old_version: Option<String>,
    pub new_version: String,
    /// Concurrent-drift warnings detected at the pre-write probe,
    /// rendered to display strings.
    pub warnings: Vec<String>,
}

/// A cross-mem edge in an exported slice whose target won't travel
/// inside the single-mem archive — `install` rejects the archive for
/// each. Mirrors `memstead_base::validator::DanglingCrossMemEdge`.
#[derive(Debug, Clone)]
pub struct DanglingCrossMemEdge {
    /// Archive-relative path of the entity carrying the edge.
    pub entity_path: String,
    /// Fully-qualified target id (e.g. `other-mem--thing`).
    pub target_id: String,
    /// The target's mem (the mem that won't travel in this archive).
    pub target_mem: String,
}

/// Backend kind of a mounted mem. Mirrors the four `MountStorage`
/// variants the engine distinguishes; the roster renders each distinctly
/// and gates which mutating affordances it offers (archive = sealed,
/// in-memory = ephemeral).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemBackendKind {
    GitBranch,
    Folder,
    Archive,
    InMemory,
}

/// One row of the workspace mem roster — the per-mount facts the home
/// sidebar renders, sourced entirely from the engine so no Swift-side
/// reconstruction can disagree with engine truth. `backend` and
/// `writable` come straight from the mount; `schema_pin` is the mount's
/// settled pin; `entity_count` is the live non-stub count for the mem;
/// `drifted` is the engine's read-only drift probe (git-branch only).
#[derive(Debug, Clone)]
pub struct MemRosterEntry {
    pub mem: String,
    pub backend: MemBackendKind,
    /// `true` when the engine reports the mount writable; archive mounts
    /// (and any read-only mount) report `false`.
    pub writable: bool,
    /// Settled schema pin (`name@x.y.z`), or `None` when the mount
    /// asserts none.
    pub schema_pin: Option<String>,
    /// Live non-stub entity count for this mem.
    pub entity_count: u64,
    /// A sibling writer has advanced the mem-repo past the engine's
    /// cached head. Always `false` for non-git-branch backends; clears
    /// after the app re-reads through the engine (`reload`).
    pub drifted: bool,
}

/// Outcome of [`crate::Engine::export_mem`]. Mirrors
/// `memstead_base::ops::MemExportResult`; the archive is written to the
/// requested `output_path` and the metadata describes what landed.
#[derive(Debug, Clone)]
pub struct MemExportOutcome {
    /// Filesystem path the `.mem` archive was written to.
    pub archive_path: String,
    pub name: String,
    pub version: String,
    pub entity_count: u64,
    pub size_bytes: u64,
    /// Dangling cross-mem edges; empty for a self-contained export.
    pub dangling_cross_mem_edges: Vec<DanglingCrossMemEdge>,
}
