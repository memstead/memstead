//! Backend-neutral provenance record.
//!
//! Two persistence shapes exist today — commit-message trailer
//! (git-branch backend) and JSONL line (folder backend's
//! `.memstead/changes.jsonl`) — and historically each backend modelled
//! its mutation log with its own type. After the workspace-store
//! rebuild both adapters construct (and are read into) this single
//! [`Provenance`] record so `memstead_changes_since` returns
//! identically-shaped values regardless of which backend serves the
//! queried mem.
//!
//! This module ships the **shape**; the read/write wiring on each
//! backend lands as that backend gains a [`crate::backend::MemBackend`]
//! implementation. The existing `crate::filesystem::changelog`
//! `ChangeEntry` / `MutationKind` pair stays as the folder backend's
//! on-disk encoder until that wiring lands; the two are kept in
//! lockstep by deliberate field correspondence (timestamp, kind,
//! entity, actor, client, note).

use std::time::SystemTime;

use crate::vcs::{Actor, ClientId};

/// Mutation kind written to provenance. The string forms produced by
/// [`Self::as_str`] are the wire shape — readers and external tools
/// (jq, grep) branch on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProvenanceKind {
    Create,
    Update,
    Delete,
    Relate,
    Rename,
    Batch,
}

impl ProvenanceKind {
    /// Stable kebab-case wire form.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProvenanceKind::Create => "create",
            ProvenanceKind::Update => "update",
            ProvenanceKind::Delete => "delete",
            ProvenanceKind::Relate => "relate",
            ProvenanceKind::Rename => "rename",
            ProvenanceKind::Batch => "batch",
        }
    }

    /// Inverse of [`Self::as_str`]. Returns `None` for any unknown
    /// string so backend readers can treat unrecognised kinds as a
    /// forward-compat extension rather than misclassify.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "create" => Some(ProvenanceKind::Create),
            "update" => Some(ProvenanceKind::Update),
            "delete" => Some(ProvenanceKind::Delete),
            "relate" => Some(ProvenanceKind::Relate),
            "rename" => Some(ProvenanceKind::Rename),
            "batch" => Some(ProvenanceKind::Batch),
            _ => None,
        }
    }
}

/// One mutation event in a mem's provenance log.
///
/// Constructed at the engine boundary (one per MCP mutating tool, one
/// per CLI mutation, one per drift-flush) and handed to the backend
/// via [`crate::backend::MemBackend::append_provenance`]. Read back
/// out via [`crate::backend::MemBackend::read_provenance`] for
/// `memstead_changes_since`.
///
/// The folder backend persists this as a JSONL line under
/// `.memstead/changes.jsonl`; the git-branch backend persists it as part
/// of the commit-message trailer block (timestamp / kind / entity ride
/// the commit metadata). The persistence form differs per backend, the
/// in-memory record does not.
#[derive(Debug, Clone)]
pub struct Provenance {
    pub timestamp: SystemTime,
    pub kind: ProvenanceKind,
    /// Mem-relative entity id (`mem:slug`), or `None` for batch
    /// mutations that touch multiple entities.
    pub entity: Option<String>,
    pub actor: Actor,
    pub client: Option<ClientId>,
    /// Agent-authored one-sentence provenance note. Whitespace-only
    /// values are normalised to `None` at construction; callers that
    /// want an empty note pass `None`.
    pub note: Option<String>,
    /// Correlation id that ties every commit produced by a single
    /// logical operation (notably a multi-mem `memstead_rename`) to one
    /// another. `Some(id)` on every commit a single logical call
    /// produced; `None` on legacy or single-call mutations that don't
    /// participate in correlation. Consumers that don't know the
    /// field continue working — it's purely additive. Single-mem
    /// mutations may carry an id too (a logical-op with one commit),
    /// or `None` — both are valid wire shapes.
    pub logical_operation_id: Option<String>,
}

impl Provenance {
    /// Build a record, normalising a whitespace-only `note` to `None`.
    /// Callers that already have a normalised `Option<String>` may set
    /// the field directly. `logical_operation_id` defaults to `None`;
    /// callers that need to tag a multi-commit logical operation use
    /// [`Self::with_logical_operation_id`].
    pub fn new(
        timestamp: SystemTime,
        kind: ProvenanceKind,
        entity: Option<String>,
        actor: Actor,
        client: Option<ClientId>,
        note: Option<String>,
    ) -> Self {
        let note = note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|s| s.to_string());
        Self {
            timestamp,
            kind,
            entity,
            actor,
            client,
            note,
            logical_operation_id: None,
        }
    }

    /// Builder: attach a correlation id so multiple commits produced
    /// by a single logical operation can be linked at read time.
    pub fn with_logical_operation_id(mut self, id: String) -> Self {
        self.logical_operation_id = Some(id);
        self
    }
}

/// Mint a fresh `logical_operation_id`. Combines a nanosecond-
/// precision timestamp with a process-monotonic counter so two ids
/// produced in the same nanosecond are still distinct, and the
/// timestamp prefix gives consumers a rough ordering hint without
/// a dedicated comparator. Mirrors the shape of
/// `make_commit_id` in the filesystem backend.
pub fn mint_logical_operation_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LOGICAL_OP_COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let counter = LOGICAL_OP_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("logop-{nanos:032x}{counter:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_wire_strings_are_stable() {
        // Locks the wire shape — readers (jq, external tools) key on
        // these exact strings.
        assert_eq!(ProvenanceKind::Create.as_str(), "create");
        assert_eq!(ProvenanceKind::Update.as_str(), "update");
        assert_eq!(ProvenanceKind::Delete.as_str(), "delete");
        assert_eq!(ProvenanceKind::Relate.as_str(), "relate");
        assert_eq!(ProvenanceKind::Rename.as_str(), "rename");
        assert_eq!(ProvenanceKind::Batch.as_str(), "batch");
    }

    #[test]
    fn new_normalises_whitespace_only_note_to_none() {
        let r = Provenance::new(
            SystemTime::UNIX_EPOCH,
            ProvenanceKind::Create,
            Some("v:e".into()),
            Actor::Cli,
            None,
            Some("   \t  ".into()),
        );
        assert!(r.note.is_none());
    }

    #[test]
    fn new_preserves_non_blank_note() {
        let r = Provenance::new(
            SystemTime::UNIX_EPOCH,
            ProvenanceKind::Create,
            Some("v:e".into()),
            Actor::Cli,
            None,
            Some("  first draft  ".into()),
        );
        assert_eq!(r.note.as_deref(), Some("first draft"));
    }
}
