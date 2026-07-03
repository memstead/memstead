//! Wire types for the three transport ops `Engine::fetch`,
//! `Engine::pull`, `Engine::push`.
//!
//! `branch_reset` lives next to these in the transport surface;
//! this file pins the success-path payloads each transport op
//! returns.

use serde::{Deserialize, Serialize};

/// Outcome of `Engine::fetch`. Updates remote-tracking refs without
/// moving the local branch pointer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FetchOutcome {
    /// Remote name the fetch targeted (verbatim from input).
    pub remote: String,
    /// Refspecs that were fetched. Empty in the response means "the
    /// remote's configured defaults"; otherwise echoes the caller's
    /// list.
    pub refspecs: Vec<String>,
    /// Per-ref tip the fetch landed on, keyed by remote-tracking ref
    /// (e.g. `refs/remotes/origin/specs` → new SHA). Only refs that
    /// actually moved appear here; unchanged refs are omitted.
    pub updated_refs: Vec<UpdatedRef>,
}

/// One ref's transition recorded by a successful fetch / pull.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdatedRef {
    /// Ref name (e.g. `refs/remotes/origin/specs`).
    pub ref_name: String,
    /// SHA the ref pointed at before this op, when known. Empty
    /// string when the ref did not exist locally before the op.
    pub previous_sha: String,
    /// SHA the ref points at after this op.
    pub new_sha: String,
}

/// Outcome of `Engine::pull`. Fast-forwards the local branch when
/// possible; refuses with `LOCAL_DIVERGENCE` on a diverged local
/// branch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PullOutcome {
    /// Mem whose branch was advanced.
    pub mem: String,
    /// Remote-tracking ref the fast-forward consumed (e.g.
    /// `refs/remotes/origin/specs`).
    pub source_ref: String,
    /// Local branch ref that was moved (e.g. `refs/heads/specs`).
    pub branch_ref: String,
    /// SHA the local branch pointed at before the pull. Empty for a
    /// fresh branch that did not yet exist locally.
    pub previous_sha: String,
    /// SHA the local branch points at after the pull.
    pub new_sha: String,
    /// Updated remote-tracking refs the underlying fetch produced.
    pub updated_refs: Vec<UpdatedRef>,
}

/// Outcome of `Engine::push`. The remote's view of the mem's
/// branch has moved to `new_sha` after the operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushOutcome {
    /// Mem whose branch was pushed.
    pub mem: String,
    /// Remote name the push targeted.
    pub remote: String,
    /// Local branch ref that was pushed (e.g. `refs/heads/specs`).
    pub branch_ref: String,
    /// SHA the remote acknowledged after the push.
    pub new_sha: String,
    /// `true` when the push was a force update (the caller passed
    /// `force: true` and the underlying ref move was not a
    /// fast-forward). Consumers warn on this in their UI.
    pub forced: bool,
}

/// Outcome of `Engine::remote_add`. Configures a named remote on the
/// workspace's mem-repo so `fetch` / `pull` / `push` have somewhere to
/// go — upsert semantics (re-pointing an existing remote is not an
/// error; `updated` says which happened).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteAddOutcome {
    /// Remote name (verbatim from input).
    pub remote: String,
    /// URL the remote now points at.
    pub url: String,
    /// `true` when the remote already existed and its URL was
    /// re-pointed; `false` when it was newly added.
    pub updated: bool,
}
