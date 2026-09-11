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
pub mod coverage;
pub mod diff;
pub mod export;
pub mod health;
pub mod health_compose;
pub mod integrity;
pub mod labelling;
pub mod redaction;
#[cfg(not(target_arch = "wasm32"))]
pub mod search;
pub mod signals;
pub mod strict;
pub mod transport;

pub use agent_notes::{AgentNotesReport, CommitNote};
pub use branch_reset::{BranchResetOutcome, StrandedCrossMemRef};
pub use changes::{
    BackendChanges, ChangeEnvelope, ChangesReport, EMPTY_TREE_SHA, MemChangedNotice,
    NoticeByChange, NoticeChanges, RENAME_SIMILARITY_DEFAULT, RENAME_SIMILARITY_MAX,
    RENAME_SIMILARITY_MIN, StoreLookup, folder_changes_since,
};
pub use commit_envelope::{CommitEnvelope, EntityChange};
pub use diff::{Diff, DiffConfig, EntityDiff, IncomingRipple};
pub use export::{MemExportBytes, MemExportError};
pub use transport::{
    FetchOutcome, PullOutcome, PushAllOutcome, PushOutcome, PushedRef, RefusedRef,
    RemoteAddOutcome, RemoteRefState, RemoteRefStatus, RemoteStatusOutcome, UpdatedRef,
};

use crate::entity::EntityId;
use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};
use std::collections::HashMap;
use std::fmt;

/// Allowed `include` keys for `memstead_overview` — single source of
/// truth shared across the MCP server and the
/// CLI's `overview` command. Mirrors `HEALTH_INCLUDE_KEYS` for
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
// Type families, cut along this file's own section headers. Each child is
// re-exported so every `memstead_base::ops::X` path that resolved before
// still resolves, and no consumer import changes.
// ---------------------------------------------------------------------------

mod crud;
mod export_types;
mod health_types;
mod search_types;
mod warning_hint;

pub use crud::*;
pub use export_types::*;
pub use health_types::*;
pub use search_types::*;
pub use warning_hint::*;

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
// Status
// ---------------------------------------------------------------------------

/// Graph status — node / edge counts and schema distribution. Renamed from
/// the former `Stats` when the `stats` command became `status`; the fields are unchanged so every caller's
/// payload stays byte-compatible.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub entity_count: usize,
    pub edge_count: usize,
    /// Edge count per relationship type, in name order: a `BTreeMap` so
    /// every renderer (CLI, MCP, ui-api) emits the same bytes run after run.
    pub edge_types: std::collections::BTreeMap<String, usize>,
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

/// What `Engine::full_refresh` changed — and, just as deliberately,
/// what it SKIPPED. The refresh is additive-only: removals never take
/// effect warm, and this report is how the caller learns whether its
/// next call will succeed instead of guessing.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FullRefreshReport {
    /// Schema versions (`name@version`) newly resolvable.
    pub schemas_added: Vec<String>,
    /// In-memory schema versions absent from the re-scanned sources —
    /// the removal was skipped; they stay resolvable until restart.
    pub schema_removals_skipped: Vec<String>,
    /// Mems newly mounted (cold-loaded like any boot-time mount).
    pub mems_mounted: Vec<String>,
    /// Mounted writable mems absent from the re-scanned roster — unmounted
    /// atomically, no longer served (applied since 2026-09-02; the former
    /// `mem_removals_skipped` reported them as left live until restart).
    pub mems_unmounted: Vec<String>,
    /// Roster entries that failed to mount and sit on the quarantine
    /// roster with their reason.
    pub mems_quarantined: Vec<String>,
    /// Per-item failures: a source or mount that failed to refresh.
    /// Failed items never surface as newly available; the others
    /// proceed.
    pub failures: Vec<RefreshFailure>,
    /// Wall-clock cost of the refresh (the bounded-cost report).
    pub elapsed_ms: u64,
}

/// One failed refresh item — `item` is `schema-source:<which>`,
/// `mount:<mem>`, `mount-manifest`, or `workspace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshFailure {
    pub item: String,
    pub error: String,
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod write_id_doc_gloss_tests;
