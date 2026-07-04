//! Wire types for `Engine::branch_reset`.
//!
//! fetch / pull / push / branch_reset form one transport surface; this op is the lone history-rewrite
//! primitive the others compose against.

use serde::{Deserialize, Serialize};

/// Successful outcome of `Engine::branch_reset`. Carries enough
/// context for callers (CLI, replay skills, audit UIs) to surface
/// what happened without a follow-up `memstead_changes_since` poll.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchResetOutcome {
    /// Mem name whose branch pointer moved.
    pub mem: String,
    /// Full ref name of the moved branch (`refs/heads/<mem>` for
    /// flat layouts, `refs/heads/<path>/<mem>` for hierarchical
    /// ones).
    pub branch_ref: String,
    /// SHA the branch pointed at before the reset.
    pub previous_sha: String,
    /// SHA the branch points at after the reset.
    pub new_sha: String,
    /// SHAs of the commits that the reset moved away from — every
    /// commit reachable from `previous_sha` but not from `new_sha`.
    /// Empty when the reset was a no-op (target == current head).
    /// Implementer guarantees: every entry was unpushed at the
    /// instant of the safety probe; nothing in the list was reachable
    /// from any `refs/remotes/*` ref.
    pub discarded_commits: Vec<String>,
}

/// One inbound cross-mem reference that a branch reset would strand: an
/// edge from an entity in another mem pointing at an entity that exists
/// at the current head but would not exist at the reset target (it was
/// created — or renamed to its current id — after the target commit).
/// Computed engine-side by `Engine::branch_reset_stranded_refs`; the
/// human surface warns with these before confirming a reset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StrandedCrossMemRef {
    /// Referencing entity (lives in a different mem than the reset).
    pub from_id: String,
    /// The referencing entity's mem.
    pub from_mem: String,
    /// The referenced entity that the reset would remove.
    pub to_id: String,
    pub rel_type: String,
}
