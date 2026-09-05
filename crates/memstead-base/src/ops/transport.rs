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

/// Outcome of `Engine::push_all`: every mounted git-branch mem's
/// declared branch plus the `__MEMSTEAD` ref of each mem-repo,
/// pushed fast-forward only. The run never stops at the first
/// refusal — a ref that cannot move lands in `refused` and the
/// remaining refs are still attempted, so one diverged branch never
/// holds back the publication of the others. A ref already at the
/// remote's SHA is recorded in `in_sync` and not pushed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PushAllOutcome {
    /// Remote name the run targeted.
    pub remote: String,
    /// Refs the remote now carries at a new SHA, in push order.
    pub pushed: Vec<PushedRef>,
    /// Refs whose local and remote SHA already agreed — nothing was
    /// sent for them.
    pub in_sync: Vec<String>,
    /// Refs the run could not move, each with its typed code.
    pub refused: Vec<RefusedRef>,
}

/// One ref `Engine::push_all` moved on the remote.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PushedRef {
    /// Full ref name (e.g. `refs/heads/specs`, `refs/heads/__MEMSTEAD`).
    pub ref_name: String,
    /// Mem the ref belongs to; `None` for the `__MEMSTEAD` ref.
    pub mem: Option<String>,
    /// SHA the remote held before the push. Empty when the remote
    /// did not carry the ref yet.
    pub previous_sha: String,
    /// SHA the remote acknowledged after the push.
    pub new_sha: String,
}

/// One ref `Engine::push_all` could not move.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefusedRef {
    /// Full ref name the refusal is about.
    pub ref_name: String,
    /// Mem the ref belongs to; `None` for the `__MEMSTEAD` ref.
    pub mem: Option<String>,
    /// Typed refusal code (`NON_FAST_FORWARD`, `LOCAL_INVALID_STATE`,
    /// `UNKNOWN_REF`, …) — the same vocabulary the single-mem push
    /// returns as an error.
    pub code: String,
    /// The refusal's human message.
    pub message: String,
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

/// The standing of one remote ref (or one mounted branch the remote lacks)
/// on `memstead status --remote`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteRefState {
    /// Local and remote heads agree.
    InSync,
    /// The remote head is an ancestor of the local head: unpushed local
    /// work, never staleness.
    LocalAhead,
    /// The local head is an ancestor of the remote head: the remote
    /// carries commits this clone has not seen. Staleness.
    Behind,
    /// Neither head is an ancestor of the other. Staleness.
    Forked,
    /// The remote head is not in the local object store: the remote
    /// carries commits this clone has never fetched, so it is behind or
    /// forked and only a `memstead fetch` can say which. Staleness.
    Unfetched,
    /// The remote carries a branch this workspace mounts (or the schemas
    /// ref) and the local gitdir has no such ref. Staleness.
    MissingLocal,
    /// A remote branch nothing mounts (whether or not a local ref of that
    /// name exists): a probable leftover of a retired or re-homed mem, or
    /// a branch that is not graph state at all. A notice, never staleness;
    /// deleting a remote branch is a human decision.
    UnmountedRemote,
    /// A mounted branch the remote does not carry yet: never pushed. A
    /// notice, never staleness.
    NotOnRemote,
}

impl RemoteRefState {
    /// The stable wire label.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InSync => "in_sync",
            Self::LocalAhead => "local_ahead",
            Self::Behind => "behind",
            Self::Forked => "forked",
            Self::Unfetched => "unfetched",
            Self::MissingLocal => "missing_local",
            Self::UnmountedRemote => "unmounted_remote",
            Self::NotOnRemote => "not_on_remote",
        }
    }

    /// Whether this state means the local graph lags the remote.
    pub fn is_stale(self) -> bool {
        matches!(
            self,
            Self::Behind | Self::Forked | Self::Unfetched | Self::MissingLocal
        )
    }
}

/// One ref on the remote-status report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteRefStatus {
    /// The full ref name (`refs/heads/<branch>`).
    pub ref_name: String,
    /// The mem the branch is mounted as, or `None` for the schemas ref
    /// and for an unmounted remote branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mem: Option<String>,
    /// `true` for the `__MEMSTEAD` schemas ref.
    pub schemas_ref: bool,
    pub state: RemoteRefState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_sha: Option<String>,
}

/// What `memstead status --remote` reports: every mounted git-branch mem
/// and the schemas ref compared against the named remote by a read-only
/// `ls-remote`, plus the notices of what could not be compared. Never a
/// refusal: a workspace without a remote, or one it cannot reach, reports
/// the fact as a notice and stands as not stale (fail open).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RemoteStatusOutcome {
    /// Remote name the comparison targeted.
    pub remote: String,
    /// Every ref compared, sorted by name.
    pub refs: Vec<RemoteRefStatus>,
    /// Why some or all of the comparison could not run, one line each.
    pub notices: Vec<String>,
    /// Counts by state, in wire-label order.
    pub counts: std::collections::BTreeMap<String, usize>,
    /// `true` when any ref is behind, forked or missing locally.
    pub stale: bool,
}
