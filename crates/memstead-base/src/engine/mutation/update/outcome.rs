//! Outcome phase of an update: the wire-facing shapes an update ends
//! in without a commit — the no-op receipt, the dry-run preview — and
//! the batch receipts (a refusal naming every failing entry with its
//! typed envelope, a rehearsal or applied receipt naming every item's
//! action). The committed single-item outcome is composed at the end
//! of the commit phase, where the store-side results it reports come
//! from.

use crate::engine::outcomes::RelationDeclared;
use crate::entity::{Entity, EntityId};
use crate::ops::{
    BatchEntry, BatchError, BatchResult, ModifiedMetadata, ModifiedSections, WarningHint,
};

use super::batch::BatchItem;
use super::{Engine, EngineError, PreparedUpdate, UpdateEntityOutcome};

/// The no-op receipt: nothing landed, so the outcome reports the
/// preserved `last_modified` from the pre-stamp `next` (which still
/// carries the entity's on-disk value because the auto-stamp never
/// ran on this branch), the unchanged hash, an empty `write_id`, and
/// the applied delta — empty (F1): the request-derived keys would
/// claim a change that did not happen.
pub(super) fn noop_outcome(
    next: &Entity,
    file_path: String,
    relations_declared: Vec<RelationDeclared>,
    anchors_changed: Option<bool>,
) -> UpdateEntityOutcome {
    let modified_date = next
        .metadata
        .get("last_modified")
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    UpdateEntityOutcome {
        id: next.id.clone(),
        title: next.title.clone(),
        file_path,
        content_hash: next.content_hash.clone(),
        write_id: String::new(),
        modified_date,
        modified_sections: ModifiedSections::default(),
        modified_metadata: ModifiedMetadata::default(),
        prospective_hash: None,
        // No write happened on the no-op path, so nothing
        // could have orphaned a stub.
        orphan_stubs_removed: Vec::new(),
        warnings: vec![WarningHint::UpdateNoop {
            id: next.id.clone(),
        }],
        relations_declared,
        anchors_changed,
    }
}

/// The dry-run preview: the prospective hash of the rendered markdown
/// beside the unchanged on-disk hash (`current_hash`), so the caller
/// can use the latter as `expected_hash` on the follow-up real call
/// (the designated stale-hash recovery path). Nothing is written, so
/// no stub could have been GC'd.
pub(super) fn dry_run_outcome(
    prepared: PreparedUpdate,
    title: String,
    current_hash: String,
    modified_date: String,
) -> UpdateEntityOutcome {
    let prospective = crate::entity::parser::compute_hash(&prepared.markdown);
    UpdateEntityOutcome {
        id: prepared.id,
        title,
        file_path: prepared.file_path,
        content_hash: current_hash,
        write_id: String::new(),
        modified_date,
        modified_sections: prepared.modified_sections,
        modified_metadata: prepared.modified_metadata,
        prospective_hash: Some(prospective),
        orphan_stubs_removed: Vec::new(),
        warnings: prepared.warnings,
        relations_declared: prepared.relations_declared,
        anchors_changed: prepared.anchors_changed,
    }
}

/// Build a per-item structured error envelope for [`Engine::batch_update`].
/// Mirrors the `{code, message, details}` shape single-update failures
/// carry on the MCP wire so a mixed-success batch is structurally uniform.
/// Variants without a typed recovery payload (boundary / internal failures
/// like `ParseAfterWrite`, `Backend`) return an empty details object —
/// the code and message channels still discriminate.
pub(in crate::engine::mutation) fn batch_error_envelope(err: &EngineError) -> BatchError {
    // The per-item envelope reads the centralised
    // `EngineError::details()` helper so every typed variant ships the
    // same recovery payload the singleton MCP/CLI surfaces emit, so
    // agents' "fix from `details` rather than re-fetching" loop works
    // the same in batch mode.
    let code = err.code().to_string();
    let message = err.to_string();
    let details = err.details();
    BatchError {
        code,
        message,
        details,
    }
}

/// The refusal receipt of a batch update: every failing entry carries
/// its typed envelope up to [`Engine::BATCH_ERROR_REPORT_CAP`] detailed
/// envelopes, `errors_suppressed` counts the rest, and every valid
/// entry reads `not_applied`. Nothing was committed.
pub(super) fn batch_refusal(
    items: Vec<(EntityId, BatchItem)>,
    errors: Vec<(usize, EngineError)>,
) -> BatchResult {
    let failed = errors.len();
    let mut error_map: std::collections::HashMap<usize, EngineError> = errors.into_iter().collect();
    let mut reported = 0usize;
    let mut suppressed = 0usize;
    let results: Vec<BatchEntry> = items
        .into_iter()
        .enumerate()
        .map(|(i, (id, _))| match error_map.remove(&i) {
            Some(e) => {
                if reported < Engine::BATCH_ERROR_REPORT_CAP {
                    reported += 1;
                    BatchEntry {
                        id,
                        action: "error".to_string(),
                        error: Some(batch_error_envelope(&e)),
                    }
                } else {
                    suppressed += 1;
                    BatchEntry {
                        id,
                        action: "error".to_string(),
                        error: None,
                    }
                }
            }
            None => BatchEntry {
                id,
                action: "not_applied".to_string(),
                error: None,
            },
        })
        .collect();
    BatchResult {
        warnings: Vec::new(),
        orphan_stubs_removed: Vec::new(),
        errors_suppressed: suppressed,
        applied: false,
        results,
        succeeded: 0,
        failed,
        write_id: String::new(),
    }
}

/// The receipt of a legal batch update: `updated` for a prepared item,
/// `noop` for an applied no-op. The rehearsal passes no warnings and
/// the marker form's empty `write_id`; the applied batch passes the
/// warnings its commits raised and the last mem's commit.
pub(super) fn batch_receipt(
    items: Vec<(EntityId, BatchItem)>,
    warnings: Vec<WarningHint>,
    write_id: String,
) -> BatchResult {
    let succeeded = items.len();
    let results: Vec<BatchEntry> = items
        .into_iter()
        .map(|(id, item)| BatchEntry {
            id,
            action: match item {
                BatchItem::Prepared => "updated".to_string(),
                BatchItem::Noop => "noop".to_string(),
                BatchItem::Error => unreachable!("refusal path returned above"),
            },
            error: None,
        })
        .collect();
    BatchResult {
        warnings,
        orphan_stubs_removed: Vec::new(),
        errors_suppressed: 0,
        applied: true,
        results,
        succeeded,
        failed: 0,
        write_id,
    }
}
