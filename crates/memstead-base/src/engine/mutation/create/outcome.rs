//! Outcome phase of a create: the wire-facing shapes a create ends
//! in without a commit — the dry-run preview, and the batch receipts
//! (a refusal naming every failing entry, a rehearsal or applied
//! receipt naming every created id). The committed single-item
//! outcome is composed at the end of the commit phase, where the
//! store-side results it reports come from.

use crate::entity::EntityId;
use crate::ops::{BatchEntry, BatchResult, WarningHint, project_incoming};

use super::{CreateEntityOutcome, Engine, EngineError, PreparedCreate};

impl Engine {
    /// The dry-run outcome: the prospective hash of the rendered
    /// markdown, an empty `write_id`, and the incoming edges a stub at
    /// this id would hand over — read from the store as it stands,
    /// since nothing is written.
    pub(super) fn create_dry_run_outcome(
        &self,
        prepared: PreparedCreate,
        created_date: String,
    ) -> CreateEntityOutcome {
        let PreparedCreate {
            mount_idx: _,
            id,
            title,
            mem,
            file_path,
            markdown,
            anchors: _,
            warnings,
            type_guidance,
            relations_declared,
            relation_targets: _,
            type_def: _,
        } = prepared;
        let prospective_hash = crate::entity::parser::compute_hash(&markdown);
        // Full's dry_run computes incoming from the existing
        // store state (the refs that *would* be adopted if a
        // stub exists at this id). Read before any mutation.
        let incoming = project_incoming(self.store.incoming(&id));
        let incoming_count = (!incoming.is_empty()).then_some(incoming.len());
        CreateEntityOutcome {
            id,
            title,
            mem,
            file_path,
            content_hash: prospective_hash,
            write_id: String::new(),
            created_date,
            warnings,
            type_guidance,
            incoming_count,
            incoming,
            relations_declared,
        }
    }
}

/// The refusal receipt of a batch create: every failing entry carries
/// its typed envelope up to [`Engine::BATCH_ERROR_REPORT_CAP`] detailed
/// envelopes, `errors_suppressed` counts the rest, and every valid
/// entry reads `not_applied`. Nothing was committed.
pub(super) fn batch_refusal(
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
                        error: Some(super::super::update::batch_error_envelope(&e)),
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

/// The receipt of a legal batch create: one `created` entry per
/// prepared item. The rehearsal passes no warnings and the marker
/// form's empty `write_id`; the applied batch passes the warnings its
/// commits raised and the last mem's commit.
pub(super) fn batch_receipt(
    prepared: Vec<PreparedCreate>,
    warnings: Vec<WarningHint>,
    write_id: String,
) -> BatchResult {
    let succeeded = prepared.len();
    let results: Vec<BatchEntry> = prepared
        .into_iter()
        .map(|p| BatchEntry {
            id: p.id,
            action: "created".to_string(),
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
