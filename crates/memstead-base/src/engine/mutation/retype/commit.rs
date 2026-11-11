use super::*;

impl Engine {
    /// Commit phase: one commit with the retype's provenance, recorded as
    /// this engine's own write.
    pub(super) fn commit_retype(
        &mut self,
        composed: ComposedRetype,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<CommittedRetype, EngineError> {
        let id = composed.validated.resolved.args.id.clone();
        let mount_idx = composed.validated.resolved.mount_idx;
        // `memstead: <verb> <id>` — the subject grammar the history reader
        // attributes touches by; the type change rides the outcome and the
        // provenance verb, not the subject.
        let commit_subject = format!("memstead: retype {id}");
        let ctx = self.commit_context(
            Some("retype_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        let record = self.provenance_record(ProvenanceKind::Retype, &id, actor, client, note);
        let landed = self.commit_staged(mount_idx, &commit_subject, &ctx, &[record])?;
        let write_id = landed.write_id;
        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);
        Ok(CommittedRetype {
            composed,
            write_id,
            landed_warnings: landed.warnings,
            stamp_warnings,
        })
    }
}
