//! Stage phase of a rename: put one mem's part of the rename into its
//! backend's pending buffer, so it rides one commit. The renamed
//! entity's own mem takes the file move, the rewritten referrers and
//! the sidecars that are keyed by the id; a peer mem takes its
//! rewritten referrers. Nothing here commits; a staged buffer is
//! discarded when the write lock is released without a commit.

use std::path::Path;

use super::super::{
    stage_anchors_rename, stage_proposal_marks_rehash, stage_proposal_marks_rename,
};
use super::{Engine, EngineError, PeerMemPlan, PreparedRename};

impl Engine {
    /// Stage the renamed entity's own mem.
    pub(super) fn stage_rename_source(&self, prepared: &PreparedRename) -> Result<(), EngineError> {
        let (id, new_id) = (&prepared.id, &prepared.new_id);
        let backend = self.mounts[prepared.mount_idx].backend.as_ref();
        backend.write_entity(
            Path::new(&prepared.new_file_path),
            prepared.markdown.as_bytes(),
        )?;
        backend.delete_entity(Path::new(&prepared.old_file_path))?;
        for rewrite in &prepared.same_mem {
            backend.write_entity(Path::new(&rewrite.file_path), rewrite.markdown.as_bytes())?;
        }
        // Move the renamed entity's anchor row old_id → new_id in the SAME
        // commit as the file move so entity + anchors rewind together and
        // resolution finds every anchor under the new id (zero under the
        // old). A no-op when the entity had no anchors (byte-identical).
        stage_anchors_rename(backend, id, new_id)?;
        // An adopted body's origin marks follow the slug in the same commit,
        // for the same reason the anchors do: the marks are keyed by slug and
        // a rename moves the id while leaving the contributor's bytes alone.
        // Following the key here is what lets the read path be exact instead
        // of inferring a rename from matching content across slugs.
        stage_proposal_marks_rename(backend, id, new_id)?;
        // The self-link rewrite moves the renamed entity's own marked bytes,
        // under its NEW slug now that the marks have followed the key.
        stage_proposal_marks_rehash(
            backend,
            new_id,
            &prepared.before_sections,
            &prepared.after_sections,
        )?;
        for rewrite in &prepared.same_mem {
            stage_proposal_marks_rehash(backend, &rewrite.id, &rewrite.before, &rewrite.after)?;
        }
        Ok(())
    }

    /// Stage one peer mem's rewritten referrers.
    pub(super) fn stage_rename_peer(&self, plan: &PeerMemPlan) -> Result<(), EngineError> {
        let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
        for rewrite in &plan.rewrites {
            peer_backend
                .write_entity(Path::new(&rewrite.file_path), rewrite.markdown.as_bytes())?;
        }
        // Each peer mem holds its own record, so its marks are carried
        // into its own commit.
        for rewrite in &plan.rewrites {
            stage_proposal_marks_rehash(
                peer_backend,
                &rewrite.id,
                &rewrite.before,
                &rewrite.after,
            )?;
        }
        Ok(())
    }
}
