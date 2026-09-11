//! Stage phase of a create: put the entity write, its anchors sidecar
//! and its derivation baselines into the mount's pending buffer so
//! they ride one commit. Shared by the single-item commit and the
//! batch, which stages every item before committing once per mem.
//! Nothing here commits; a staged buffer is discarded by the caller
//! on a later failure.

use std::path::Path;

use super::super::{rel_type_declares_derivation, stage_anchors_sidecar, stage_derivation_sidecar};
use super::{Engine, EngineError, PreparedCreate};

impl Engine {
    /// Stage one prepared create's write and sidecars into its
    /// mount's pending buffer.
    pub(super) fn stage_prepared_create(
        &self,
        prepared: &PreparedCreate,
    ) -> Result<(), EngineError> {
        // 8. Write through the backend; the commit follows in the
        //    commit phase. The folder backend's commit ignores the
        //    message; the canonical form is harmless there.
        let backend = self.mounts[prepared.mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&prepared.file_path), prepared.markdown.as_bytes())?;
        // Stage the anchors sidecar into the SAME pending buffer so it
        // rides the entity's commit atomically. Only when the create
        // carried anchors — an anchorless create writes no sidecar and is
        // byte-identical to a pre-anchor create.
        if !prepared.anchors.is_empty() {
            stage_anchors_sidecar(backend, &prepared.id, &[], prepared.anchors.clone(), true)?;
        }
        // Derivation baselines: each explicitly
        // declared relation on a derivation rel-type records the
        // target's current hash ("" for an absent/stubbed target),
        // staged so baseline and entity ride one commit.
        if let Some(schema) = self.schemas.get(&prepared.mem) {
            for r in prepared
                .relations_declared
                .iter()
                .filter(|r| rel_type_declares_derivation(schema, &r.rel_type))
            {
                let hash = self
                    .store
                    .get(&r.target)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default();
                let (from, rel, to) = (
                    prepared.id.to_string(),
                    r.rel_type.clone(),
                    r.target.to_string(),
                );
                stage_derivation_sidecar(backend, |s| s.set(&from, &rel, &to, &hash))?;
            }
        }
        Ok(())
    }
}
