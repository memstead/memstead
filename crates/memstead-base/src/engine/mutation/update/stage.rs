//! Stage phase of an update: put the entity write, the anchors
//! sidecar and the derivation baselines into the mount's pending
//! buffer so they ride one commit. Shared by the single-item commit
//! and the batch, which stages every item before committing once per
//! mem. Nothing here commits; a staged buffer is discarded by the
//! caller on a later failure.

use std::path::Path;

use super::super::{rel_type_declares_derivation, stage_anchors_sidecar, stage_derivation_sidecar};
use super::{Engine, EngineError, PreparedUpdate};

impl Engine {
    /// Stage one prepared update's write and sidecars into its mount's
    /// pending buffer. `stage_anchors` says whether the anchors sidecar
    /// is staged at all: the single-item path stages it only when the
    /// merge changes the sidecar (`anchors_changed == Some(true)`), the
    /// batch whenever the item carries anchors or unsets.
    pub(super) fn stage_prepared_update(
        &self,
        prepared: &PreparedUpdate,
        stage_anchors: bool,
    ) -> Result<(), EngineError> {
        let backend = self.mounts[prepared.mount_idx].backend.as_ref();
        backend.write_entity(Path::new(&prepared.file_path), prepared.markdown.as_bytes())?;
        // Stage the anchors sidecar into the same commit as the entity
        // write.
        if stage_anchors {
            stage_anchors_sidecar(
                backend,
                &prepared.id,
                &prepared.anchor_unsets,
                prepared.anchors.clone(),
                prepared.content_changed,
            )?;
        }
        // Derivation baselines: declared
        // relations on a derivation rel-type record the target's
        // current hash, riding the same commit as the entity write.
        if let Some(schema) = self.schemas.get(prepared.id.mem()) {
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
