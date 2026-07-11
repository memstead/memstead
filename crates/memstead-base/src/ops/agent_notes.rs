//! Agent-notes wire types — pure data shapes for the
//! commit-trailer + workspace-`__MEMSTEAD`-ref payload that the
//! git-branch backend produces from its gitdir.
//!
//! These types live in `memstead-base` so the unified `Engine` can carry
//! them as Optional fields on [`crate::ops::changes::ChangesReport`]
//! (populated when callers pass `include_notes: true`, empty
//! otherwise). The gix-bound producers (`read_memstead_ref`,
//! `agent_notes_since`) stay in the git-branch backend crate — they
//! read from a gitdir and have no meaningful equivalent on the folder
//! or archive backends.
//!
//! `CommitNote` carries the parsed subject + trailer block from
//! `crate::vcs::format_commit_message`. The workspace-level pointer
//! is the SHA of `refs/heads/__MEMSTEAD` (unified schemas + per-mem
//! configs branch), exposed as `memstead_ref: Option<String>` on the
//! report — `None` when the workspace has not been migrated to the
//! unified layout yet.

use serde::Serialize;

/// One commit's worth of structured agent-note state. Fields are
/// populated best-effort: a body that doesn't match the
/// `memstead: <verb> <id>` subject shape leaves `tool_verb` / `entity_id`
/// `None`; absent trailers leave the corresponding fields `None`.
/// Callers branch on `actor` for agent-vs-external classification.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CommitNote {
    pub mem: String,
    pub sha: String,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_verb: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// Correlation id linking every commit produced by a single
    /// logical operation (notably multi-mem `memstead_rename`).
    /// Populated from the `Logical-Op:` trailer when present.
    #[serde(skip_serializing_if = "Option::is_none", rename = "logical_op")]
    pub logical_operation_id: Option<String>,
    /// Ids a multi-entity commit touched (notably `batch_update`),
    /// recovered from the `Entities:` commit trailer. Lets an
    /// `--include-notes` consumer name every entity a batch changed from
    /// the note record alone — `entity_id`/`subject` keep their
    /// (count-string) shape for backward compatibility. Empty (and
    /// serde-omitted) for single-entity commits.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub entity_ids: Vec<String>,
    /// Commit timestamp in seconds since unix epoch.
    pub timestamp: i64,
}

/// Walked output of `agent_notes_since`. `head` echoes the resolved
/// branch tip so callers record it as the next polling cursor without
/// a follow-up `memstead_health` round-trip. `memstead_ref` carries the
/// workspace-level `__MEMSTEAD` ref tip (unified schemas + per-mem
/// configs) so commit-mirroring consumers — e.g. an outer-repo cursor
/// block — anchor it alongside the per-mem head without a second
/// round-trip. `None` when the workspace has not been migrated to the
/// unified layout yet.
#[derive(Debug, Clone, Serialize)]
pub struct AgentNotesReport {
    pub mem: String,
    pub since: String,
    pub head: String,
    pub notes: Vec<CommitNote>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memstead_ref: Option<String>,
}
