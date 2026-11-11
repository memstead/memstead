use super::*;

impl Engine {
    /// Stage phase: the rendered file, and the origin marks of an adopted
    /// body, in the mem's working state.
    pub(super) fn stage_retype(&self, composed: &ComposedRetype) -> Result<(), EngineError> {
        let resolved = &composed.validated.resolved;
        let backend = self.mounts[resolved.mount_idx].backend.as_ref();
        backend.write_entity(
            Path::new(&resolved.entity.file_path),
            composed.markdown.as_bytes(),
        )?;
        // An adopted body's origin marks follow a re-keyed section in the
        // same commit. A `section_map` moves content to another key without
        // changing it, and a mark's hash covers its key, so an unfollowed
        // mark would stop matching and serve a contributor's untouched
        // sentences as the workspace's own prose.
        crate::engine::mutation::stage_proposal_marks_retype(
            backend,
            &resolved.args.id,
            &resolved.args.section_map,
            &composed.validated.next.sections,
        )?;
        Ok(())
    }
}
