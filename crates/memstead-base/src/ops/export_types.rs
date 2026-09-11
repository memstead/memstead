//! Export and mem-setting outcome types.

use super::*;

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
    /// Entities the export declined to regenerate because their stored body
    /// ends inside an unterminated code fence: writing them would seal the
    /// sections that fence absorbed. Skipping one entity
    /// is the non-stranding half of that refusal — the rest of the export
    /// still lands, and the entity is named rather than silently passed over.
    /// Empty on the happy path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refused_entities: Vec<RefusedEntity>,
}

/// One entity `export_markdown` declined, with the condition that stopped it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct RefusedEntity {
    pub id: String,
    pub reason: String,
    pub detail: String,
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
    /// Private-pattern spans redacted in the archive's authoring
    /// provenance, counted per class (`ops::redaction`); empty when no
    /// rationale carried one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redactions: Vec<crate::ops::redaction::RedactionCount>,
    /// Ids in the exported slice whose stored body ends inside an
    /// unterminated code fence. `install` refuses the archive for each one
    /// (the repack would bury the sections that fence absorbed), so the
    /// condition is surfaced here for the same reason the dangling edges
    /// above are: the operator should see the install-time failure before
    /// sharing, not after. One predicate, two postures.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unterminated_fence_entities: Vec<String>,
}

/// Result of `Engine::set_mem_internal`. Carries `warnings` for the same
/// reason every other config setter does: without a channel the
/// `CONFIG_WRITE_INTERVENED` report has nowhere to go.
#[derive(Debug, Clone, Serialize)]
pub struct SetMemInternalOutcome {
    pub mem: String,
    pub internal: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
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

/// Result of `Engine::set_mem_title`. Same shape discipline as
/// [`SetMemDescriptionOutcome`].
#[derive(Debug, Clone, Serialize)]
pub struct SetMemTitleOutcome {
    pub mem: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningHint>,
}

/// Result of `Engine::set_mem_subject`. The block sets/clears as a
/// unit; old/new carry the whole block.
#[derive(Debug, Clone, Serialize)]
pub struct SetMemSubjectOutcome {
    pub mem: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_subject: Option<memstead_schema::MemSubject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_subject: Option<memstead_schema::MemSubject>,
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
