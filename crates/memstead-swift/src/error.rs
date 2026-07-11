//! FFI error surface.
//!
//! `MemsteadError` maps `memstead_base::engine::EngineError` onto a smaller
//! Swift-facing set of cases. The reviewer-friendly split keeps the API
//! focused on categories the UI can act on (NotFound triggers a "reload
//! and retry", IoError pops a filesystem warning, HashMismatch asks the
//! user to refresh, etc.) while still carrying the underlying message
//! for diagnostics. Agent-actionable validation variants
//! (`UnknownSection`, `UnknownMem`) get their own Swift cases so the
//! macOS app can branch on them without string-parsing the message.

use memstead_base::EngineError;
use memstead_base::PipelineEditError;
use memstead_base::runtime_validator::ValidationError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum MemsteadError {
    #[error("not found: {message}")]
    NotFound { message: String },
    #[error("validation failed: {message}")]
    ValidationFailed { message: String },
    #[error("hash mismatch: {message} (current: {current})")]
    HashMismatch { message: String, current: String },
    #[error("schema error: {message}")]
    SchemaError { message: String },
    #[error("io error: {message}")]
    IoError { message: String },
    #[error("internal error: {message}")]
    Internal { message: String },
    /// Mirrors `ValidationError::UnknownSection` — the app's entity editor
    /// can branch on this to show the declared-section picker rather than
    /// a generic error.
    #[error("unknown section '{key}' for type '{entity_type}'")]
    UnknownSection {
        key: String,
        entity_type: String,
        declared: Vec<String>,
        suggestion: Option<String>,
    },
    /// Mirrors `EngineError::UnknownMem` — the app can branch on this to
    /// re-open the mem picker. `writable_mems` is empty on the
    /// unified engine's variant (the legacy variant carried the roster);
    /// the picker can re-query the engine for the live list.
    #[error("unknown writable mem '{name}'")]
    UnknownMem {
        name: String,
        writable_mems: Vec<String>,
    },
    /// Mirrors `EngineError::PushedCommitsProtected` — the rewind
    /// surface renders this guard refusal distinctly (the reset would
    /// discard commits already pushed to a remote).
    #[error("{message}")]
    PushedCommitsProtected {
        message: String,
        pushed_shas: Vec<String>,
    },
}

impl From<EngineError> for MemsteadError {
    fn from(err: EngineError) -> Self {
        match err {
            // --- Typed agent-actionable variants ---------------------------
            EngineError::UnknownMem(name) => Self::UnknownMem {
                name,
                writable_mems: Vec::new(),
            },

            // Validation lift — only UnknownSection carves out a typed
            // Swift variant; the rest collapse into ValidationFailed.
            EngineError::Validation(ValidationError::UnknownSection {
                key,
                entity_type,
                declared,
                suggestion,
            }) => Self::UnknownSection {
                key,
                entity_type,
                declared,
                suggestion,
            },
            EngineError::Validation(v) => Self::ValidationFailed {
                message: v.to_string(),
            },

            // --- Lookup ----------------------------------------------------
            EngineError::NotFound { id } => Self::NotFound { message: id },
            EngineError::AlreadyExists { id } => Self::ValidationFailed {
                message: format!("already exists: {id}"),
            },
            EngineError::DuplicateMem(name) => Self::ValidationFailed {
                message: format!("duplicate mem: {name}"),
            },

            // --- Optimistic locking / structural ---------------------------
            EngineError::HashMismatch { current, .. } => Self::HashMismatch {
                message: "entity was modified concurrently".to_string(),
                current,
            },
            EngineError::RelationshipCycle {
                rel_type, from, to, ..
            } => Self::ValidationFailed {
                message: format!(
                    "relationship cycle: {rel_type} from {from} to {to} would close a cycle"
                ),
            },
            EngineError::CrossMemLinkNotAllowed { from_mem, to_mem } => Self::ValidationFailed {
                message: format!(
                    "cross-mem link from `{from_mem}` to `{to_mem}` is not allowed by the workspace `[cross_mem_links]` policy"
                ),
            },
            EngineError::CrossMemTargetNotFound {
                target_id,
                target_mem,
            } => Self::ValidationFailed {
                message: format!(
                    "cross-mem target `{target_id}` is absent in read-only mem `{target_mem}`"
                ),
            },
            EngineError::CrossMemEdgeNotDeclared {
                source_schema,
                target_schema,
                rel_type,
                from_id,
                to_id,
            } => Self::ValidationFailed {
                message: format!(
                    "cross-mem edge {rel_type} from `{from_id}` (schema {source_schema}) to `{to_id}` (schema {target_schema}) is not declared in {source_schema}'s `cross_mem_relationships:` section"
                ),
            },
            EngineError::RepairNotNeeded { id, recovery } => Self::ValidationFailed {
                message: format!(
                    "repair input refused for {id}: the entity currently passes the conformance check — {recovery}"
                ),
            },
            EngineError::RenameNoOp { id, new_title } => Self::ValidationFailed {
                message: format!("rename no-op for {id}: {new_title:?} slugifies the same"),
            },
            EngineError::RenameBlockedByCrossMemPolicy {
                from_mem,
                blocked_referrers,
            } => Self::ValidationFailed {
                message: {
                    let pairs: Vec<String> = blocked_referrers
                        .iter()
                        .map(|r| format!("{} → {} ({})", r.from_mem, r.to_mem, r.count))
                        .collect();
                    format!(
                        "rename blocked into `{from_mem}` — policy denies: {}",
                        pairs.join(", ")
                    )
                },
            },
            EngineError::RenamePartialFailure {
                committed_mems,
                failed_mem,
                failure_cause,
            } => Self::ValidationFailed {
                message: format!(
                    "rename partial-failure: mem `{failed_mem}` aborted ({failure_cause}) after {committed_mems:?} already committed"
                ),
            },
            EngineError::RelationHasBodyLinks {
                from_id,
                to_id,
                rel_type,
                body_links,
            } => Self::ValidationFailed {
                message: format!(
                    "cannot remove {rel_type} {from_id} → {to_id}: section(s) {body_links:?} still reference the target"
                ),
            },
            EngineError::WikiLinkWithoutRelation { from_id, missing } => Self::ValidationFailed {
                message: format!(
                    "post-mutation body of {from_id} has {n} unbacked wiki-link(s); declare relations via memstead_relate before retrying",
                    n = missing.len()
                ),
            },
            EngineError::ReadOnlyMount(name) => Self::ValidationFailed {
                message: format!("mem {name} is mounted read-only"),
            },
            EngineError::HasIncomingRefs { id, referrers } => Self::ValidationFailed {
                message: format!(
                    "{id} has {} incoming write-mem references; remove them first via memstead_relate --remove or memstead_update",
                    referrers.len()
                ),
            },
            EngineError::MemHasIncomingRefs { mem, referrers } => Self::ValidationFailed {
                message: format!(
                    "mem `{mem}` has {} incoming write-mem reference(s); remove them first via memstead_relate --remove or memstead_update",
                    referrers.len()
                ),
            },

            // --- Validation family composed into ValidationFailed ----------
            EngineError::UnknownType { .. }
            | EngineError::InvalidTitle(_)
            | EngineError::ConflictingSectionModes { .. }
            | EngineError::RequiredFieldUnset { .. }
            | EngineError::MissingRequiredSection { .. }
            | EngineError::SetAndUnsetConflict { .. }
            | EngineError::PatchSectionEmpty { .. }
            | EngineError::PatchOldNotFound { .. }
            | EngineError::MemNameCollision { .. }
            | EngineError::StubCannotRelate { .. }
            | EngineError::StubNotUpdatable { .. }
            | EngineError::StubNotRenamable { .. }
            | EngineError::InvalidEntityId { .. }
            | EngineError::InvalidWikiLinkTarget { .. }
            | EngineError::InvalidWikiLinkMem { .. }
            | EngineError::InvalidInput(_)
            | EngineError::UnknownRef(_)
            | EngineError::UnknownRemote(_)
            | EngineError::LocalDivergence { .. }
            | EngineError::NonFastForward { .. }
            | EngineError::LocalInvalidState { .. }
            | EngineError::SchemaViolationInFetch { .. }
            | EngineError::RenameSimilarityOutOfRange { .. }
            | EngineError::InvalidChangesCursor { .. }
            | EngineError::MissingRequiredDescription { .. }
            | EngineError::DescriptionNotPermitted { .. }
            | EngineError::RelationManualAuthoringForbidden { .. }
            | EngineError::MemConfigIncomplete { .. }
            | EngineError::InvalidAnchor(_)
            | EngineError::MarkdownExportUnsupportedBackend { .. } => Self::ValidationFailed {
                message: err.to_string(),
            },

            // Optimistic-locking shape the app already renders as
            // "Concurrent change — reload and retry": the reset's observed
            // head no longer matches the live one.
            EngineError::BranchResetHeadMoved { ref current, .. } => {
                let current = current.clone();
                Self::HashMismatch {
                    message: err.to_string(),
                    current,
                }
            }
            EngineError::PushedCommitsProtected {
                ref pushed_shas, ..
            } => {
                let shas = pushed_shas.clone();
                Self::PushedCommitsProtected {
                    message: err.to_string(),
                    pushed_shas: shas,
                }
            }

            // --- Boundary / internal ---------------------------------------
            EngineError::Parse(s) => Self::ValidationFailed {
                message: format!("parse error: {s}"),
            },
            EngineError::Mem(s) => Self::ValidationFailed {
                message: format!("mem error: {s}"),
            },
            EngineError::ParseAfterWrite(s) => Self::Internal {
                message: format!("parse after write failed: {s}"),
            },
            EngineError::Backend(e) => Self::Internal {
                message: format!("backend error: {e}"),
            },
            EngineError::SchemaNotFound { mem, pin, .. } => Self::SchemaError {
                message: format!("mem {mem}: schema pin {pin} not found"),
            },
            EngineError::SchemaResolverInit(e) => Self::SchemaError {
                message: format!("schema resolver init failed: {e}"),
            },
            EngineError::SearchUnavailable => Self::Internal {
                message: err.to_string(),
            },
            EngineError::EmptyUpdate { id } => Self::ValidationFailed {
                message: format!(
                    "no mutation content for {id} — payload carries an id but every mutation map is empty"
                ),
            },
        }
    }
}

impl From<PipelineEditError> for MemsteadError {
    fn from(err: PipelineEditError) -> Self {
        match err {
            // No workspace store to edit — an engine-configuration problem,
            // not user input.
            PipelineEditError::NoWorkspaceRoot => Self::Internal {
                message: err.to_string(),
            },
            // Caller-actionable shape problems collapse into ValidationFailed;
            // the message carries the specifics (which key, which referrers).
            // Capability carries the matrix's typed refusal messages with
            // their remedies; Dangling names the unresolvable reference —
            // both are caller-actionable, so they ride ValidationFailed
            // with the message intact (the app renders it verbatim).
            PipelineEditError::AlreadyExists { .. }
            | PipelineEditError::Referenced { .. }
            | PipelineEditError::RenameTargetExists { .. }
            | PipelineEditError::InvalidJson { .. }
            | PipelineEditError::Capability { .. }
            | PipelineEditError::Dangling { .. } => Self::ValidationFailed {
                message: err.to_string(),
            },
            PipelineEditError::NotFound { .. } => Self::NotFound {
                message: err.to_string(),
            },
            // Underlying file IO / parse failure on the store.
            PipelineEditError::Store(e) => Self::IoError {
                message: e.to_string(),
            },
            // The edit landed on disk but the provenance mirror commit
            // failed — surfaced, never silently dropped.
            PipelineEditError::Provenance(_) => Self::IoError {
                message: err.to_string(),
            },
        }
    }
}

impl From<memstead_engine::FullEngineError> for MemsteadError {
    fn from(err: memstead_engine::FullEngineError) -> Self {
        use memstead_engine::FullEngineError as P;
        match err {
            // A lean-side failure surfaced through the full orchestrator
            // (`UnknownMem`, `ReadOnlyMount`, `SchemaNotFound`, …)
            // delegates to the canonical `EngineError` mapping so typed
            // Swift variants (`UnknownMem`) survive the lift.
            P::Lean(e) => e.into(),
            // Every remaining variant is a caller-actionable lifecycle
            // refusal — workspace-policy gates, a malformed name, an
            // occupied location, or detected storage residue. They carry
            // their recovery story in the message; collapse into
            // `ValidationFailed` so the roster surfaces the typed refusal
            // rather than leaving a partially-created mem behind.
            other => Self::ValidationFailed {
                message: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_edit_referenced_maps_to_validation_failed() {
        let e: MemsteadError = PipelineEditError::Referenced {
            primitive: "medium",
            key: "v/m".into(),
            referrers: vec!["f".into()],
        }
        .into();
        match e {
            MemsteadError::ValidationFailed { message } => {
                assert!(message.contains("referenced"), "got: {message}")
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_edit_not_found_maps_to_not_found() {
        let e: MemsteadError = PipelineEditError::NotFound {
            primitive: "projection",
            key: "v/p".into(),
        }
        .into();
        assert!(matches!(e, MemsteadError::NotFound { .. }), "got {e:?}");
    }

    #[test]
    fn engine_error_not_found_maps_to_not_found() {
        let e: MemsteadError = EngineError::NotFound {
            id: "abc".to_string(),
        }
        .into();
        match e {
            MemsteadError::NotFound { message } => assert_eq!(message, "abc"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn engine_error_hash_mismatch_carries_current_hash() {
        let e: MemsteadError = EngineError::HashMismatch {
            id: "abc".into(),
            current: "deadbeef".into(),
            is_stub: false,
        }
        .into();
        match e {
            MemsteadError::HashMismatch { message, current } => {
                assert!(message.contains("modified concurrently"), "got: {message}");
                assert_eq!(current, "deadbeef");
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn engine_error_mem_maps_to_validation_failed() {
        let e: MemsteadError = EngineError::Mem("unknown mem: x".to_string()).into();
        match e {
            MemsteadError::ValidationFailed { message } => {
                assert!(message.contains("mem error"), "got: {message}")
            }
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
    }

    #[test]
    fn engine_error_unknown_section_carries_declared_list() {
        let e: MemsteadError = EngineError::Validation(ValidationError::UnknownSection {
            key: "foo".into(),
            entity_type: "spec".into(),
            declared: vec!["identity".into(), "purpose".into()],
            suggestion: Some("purpose".into()),
        })
        .into();
        match e {
            MemsteadError::UnknownSection {
                key,
                entity_type,
                declared,
                suggestion,
            } => {
                assert_eq!(key, "foo");
                assert_eq!(entity_type, "spec");
                assert_eq!(declared.len(), 2);
                assert_eq!(suggestion.as_deref(), Some("purpose"));
            }
            other => panic!("expected UnknownSection, got {other:?}"),
        }
    }

    #[test]
    fn engine_error_unknown_mem_carries_name() {
        let e: MemsteadError = EngineError::UnknownMem("ghost".to_string()).into();
        match e {
            MemsteadError::UnknownMem {
                name,
                writable_mems,
            } => {
                assert_eq!(name, "ghost");
                // The unified engine's UnknownMem variant does not carry
                // the writable roster; the picker re-queries the engine.
                assert!(writable_mems.is_empty());
            }
            other => panic!("expected UnknownMem, got {other:?}"),
        }
    }
}
