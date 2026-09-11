//! Outcome phase of an update: the wire-facing shapes an update ends
//! in without a commit, the no-op receipt and the dry-run preview.
//! The committed single-item outcome is composed at the end of the
//! commit phase, where the store-side results it reports come from;
//! the batch receipts are the mutation module's shared builders.

use crate::engine::outcomes::RelationDeclared;
use crate::entity::Entity;
use crate::ops::{ModifiedMetadata, ModifiedSections, WarningHint};

use super::{PreparedUpdate, UpdateEntityOutcome};

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
