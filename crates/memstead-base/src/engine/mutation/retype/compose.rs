use super::*;

impl Engine {
    /// Compose phase: stamp the timestamps and render the file the
    /// retype writes.
    pub(super) fn compose_retype(
        &self,
        validated: ValidatedRetype,
    ) -> Result<ComposedRetype, EngineError> {
        let mut validated = validated;
        let today = self.now_iso();
        let target_def = validated.resolved.target_def.clone();
        crate::engine::mutation::auto_stamp_timestamps(
            &mut validated.next,
            target_def.as_ref(),
            &today,
        );
        let markdown =
            crate::engine::mutation::render_for_write(&validated.next, target_def.as_ref())?;
        Ok(ComposedRetype {
            validated,
            markdown,
        })
    }

    /// A dry run's outcome: the prospective hash, nothing written.
    pub(super) fn retype_preview(
        &self,
        composed: ComposedRetype,
    ) -> Result<RetypeEntityOutcome, EngineError> {
        let ComposedRetype {
            validated,
            markdown,
        } = composed;
        let ValidatedRetype {
            resolved,
            sections_renamed,
            edges_rechecked,
            warnings,
            ..
        } = validated;
        let ResolvedRetype {
            args,
            mem,
            entity,
            target_def,
            ..
        } = resolved;
        let parsed = parse_markdown(&markdown, &entity.file_path, target_def.as_ref(), &mem)
            .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let prospective = parsed.entity.content_hash.clone();
        Ok(RetypeEntityOutcome {
            id: args.id.clone(),
            file_path: entity.file_path.clone(),
            old_type: entity.entity_type.clone(),
            new_type: args.target_type.clone(),
            content_hash: entity.content_hash.clone(),
            prospective_hash: Some(prospective.clone()),
            write_id: String::new(),
            sections_renamed,
            edges_rechecked,
            checks_stale: true,
            staleness_note: staleness_note(&args.id, &entity.content_hash, &prospective),
            warnings,
        })
    }
}
