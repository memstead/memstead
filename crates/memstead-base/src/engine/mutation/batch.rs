//! The results every batch verb ends in, built once: the refusal
//! that names every failing entry (report-all, capped), the rehearsal
//! and applied receipts, and the empty-batch result. Batch create,
//! batch update and batch relate call these with their own action
//! words; the report-all cap, the `errors_suppressed` count and the
//! `not_applied` marking therefore behave identically on every verb.

use crate::entity::EntityId;
use crate::ops::{BatchEntry, BatchError, BatchResult, WarningHint};

use super::super::Engine;
use super::super::EngineError;

/// The result of a batch with no entries: applied, nothing to report.
pub(crate) fn batch_empty() -> BatchResult {
    BatchResult {
        warnings: Vec::new(),
        orphan_stubs_removed: Vec::new(),
        errors_suppressed: 0,
        applied: true,
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
        write_id: String::new(),
    }
}

/// The refusal receipt of a batch: every failing entry carries its
/// typed envelope up to [`Engine::BATCH_ERROR_REPORT_CAP`] detailed
/// envelopes, `errors_suppressed` counts the rest, and every valid
/// entry reads `not_applied`. Nothing was committed.
pub(crate) fn batch_refusal(
    ids_in_order: Vec<EntityId>,
    errors: Vec<(usize, EngineError)>,
) -> BatchResult {
    let failed = errors.len();
    let mut error_map: std::collections::HashMap<usize, EngineError> = errors.into_iter().collect();
    let mut reported = 0usize;
    let mut suppressed = 0usize;
    let results: Vec<BatchEntry> = ids_in_order
        .into_iter()
        .enumerate()
        .map(|(i, id)| match error_map.remove(&i) {
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

/// The receipt of a legal batch: one entry per item with the action
/// word the verb chose for it. The rehearsal passes no warnings and
/// the marker form's empty `write_id`; the applied batch passes the
/// warnings its commits raised and the last mem's commit. Only relate
/// collects orphan stubs; the other verbs pass none.
pub(crate) fn batch_receipt(
    actions: Vec<(EntityId, String)>,
    warnings: Vec<WarningHint>,
    orphan_stubs_removed: Vec<EntityId>,
    write_id: String,
) -> BatchResult {
    let succeeded = actions.len();
    let results: Vec<BatchEntry> = actions
        .into_iter()
        .map(|(id, action)| BatchEntry {
            id,
            action,
            error: None,
        })
        .collect();
    BatchResult {
        warnings,
        orphan_stubs_removed,
        errors_suppressed: 0,
        applied: true,
        results,
        succeeded,
        failed: 0,
        write_id,
    }
}

/// Build a per-item structured error envelope for a batch refusal.
/// Mirrors the `{code, message, details}` shape single-item failures
/// carry on the MCP wire so a mixed-success batch is structurally uniform.
/// Variants without a typed recovery payload (boundary / internal failures
/// like `ParseAfterWrite`, `Backend`) return an empty details object —
/// the code and message channels still discriminate.
pub(crate) fn batch_error_envelope(err: &EngineError) -> BatchError {
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
