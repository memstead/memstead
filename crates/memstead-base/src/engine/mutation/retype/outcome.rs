use super::*;

impl Engine {
    /// Outcome phase: the store and the indexes take the write, and the
    /// receipt reports it with every warning in the order the verb has
    /// always reported them (the commit's, the store update's, the
    /// reload's, the version stamp's, the validation's, the note's).
    pub(super) fn retype_outcome(
        &mut self,
        committed: CommittedRetype,
        mut drift_warnings: Vec<WarningHint>,
        note: Option<&str>,
    ) -> RetypeEntityOutcome {
        let CommittedRetype {
            composed,
            write_id,
            landed_warnings,
            stamp_warnings,
        } = committed;
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
            mount_idx,
            entity,
            target_def,
            ..
        } = resolved;
        let id = &args.id;
        let mut outcome_warnings = landed_warnings;

        // A failed store update reloaded the mem from storage, where the
        // write is; the receipt reads from that view.
        let content_hash = self
            .after_commit(mount_idx, "store_update", &mut outcome_warnings, |engine| {
                let parse_result =
                    parse_markdown(&markdown, &entity.file_path, target_def.as_ref(), &mem)
                        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                let content_hash = parse_result.entity.content_hash.clone();
                let fallback = engine_fallback_type();
                push_entities_into_store(
                    &mut engine.store,
                    vec![parse_result],
                    fallback.as_ref(),
                    None,
                );
                crate::entity::store_builder::remap_alias_target_edge_sources(
                    &mut engine.store,
                    &engine.schemas,
                );
                Ok(content_hash)
            })
            .unwrap_or_else(|| {
                self.store
                    .get(id)
                    .map(|e| e.content_hash.clone())
                    .unwrap_or_default()
            });
        self.invalidate_communities();
        self.maintain_search_indexes(std::slice::from_ref(id));

        outcome_warnings.append(&mut drift_warnings);
        outcome_warnings.extend(stamp_warnings);
        outcome_warnings.extend(warnings);
        if let Some(w) = self.note_missing_warning("retype_entity", note) {
            outcome_warnings.push(w);
        }

        RetypeEntityOutcome {
            id: id.clone(),
            file_path: entity.file_path.clone(),
            old_type: entity.entity_type.clone(),
            new_type: args.target_type.clone(),
            staleness_note: staleness_note(id, &entity.content_hash, &content_hash),
            content_hash,
            prospective_hash: None,
            write_id,
            sections_renamed,
            edges_rechecked,
            checks_stale: true,
            warnings: outcome_warnings,
        }
    }
}
