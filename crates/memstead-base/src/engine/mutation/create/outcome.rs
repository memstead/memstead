//! Outcome phase of a create: the wire-facing shape a create ends in
//! without a commit, the dry-run preview. The committed single-item
//! outcome is composed at the end of the commit phase, where the
//! store-side results it reports come from; the batch receipts are
//! the mutation module's shared builders.

use crate::ops::project_incoming;

use super::{CreateEntityOutcome, Engine, PreparedCreate};

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
