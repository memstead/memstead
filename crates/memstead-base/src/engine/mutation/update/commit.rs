//! Commit phase of a single update: stage the prepared write, land
//! the commit (`memstead: update <id>`, or `memstead: anchor <id>`
//! when the sidecar is the sole delta), append provenance, record the
//! self-write, and apply the change to the in-memory store by
//! re-parsing the bytes that hit disk. The store application is
//! shared with the batch path, which drives the same steps in
//! [`super::batch`] after staging every item.

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::provenance::{Provenance, ProvenanceKind};

use super::{Actor, ClientId, Engine, EngineError, PreparedUpdate, UpdateEntityOutcome};

/// The store-side results of applying a prepared write — filled in
/// after the commit lands by [`Engine::apply_prepared_to_store`].
pub(super) struct AppliedWrite {
    pub(super) content_hash: String,
    pub(super) title: String,
    pub(super) orphan_stubs_removed: Vec<EntityId>,
}

impl Engine {
    /// Stage the prepared disk write, commit it as one commit, append
    /// provenance, and apply the change to the in-memory store — the
    /// single-update tail of [`Self::update_entity`]. The batch path
    /// drives the same steps but commits once across all items.
    pub(super) fn commit_prepared_update(
        &mut self,
        prepared: PreparedUpdate,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<UpdateEntityOutcome, EngineError> {
        // Aggregate signals: the entities an update can move are the
        // updated entity itself, the endpoints of every edge it adds
        // or removes (pre-write counterparts in both directions plus
        // the post-write markdown's targets), and — through the
        // neighbour filter — the counterparts of an entity whose
        // `neighbour_field` value the write changes (covered by the
        // same pre-write counterpart set). Captured before disk or
        // store mutate, diffed after the store applies.
        let signal_snapshot = {
            let mut candidates: Vec<EntityId> = vec![prepared.id.clone()];
            candidates.extend(
                self.store
                    .outgoing(&prepared.id)
                    .iter()
                    .map(|e| e.target.clone()),
            );
            candidates.extend(
                self.store
                    .incoming(&prepared.id)
                    .iter()
                    .map(|e| e.from.clone()),
            );
            if let Ok(parsed) = parse_markdown(
                &prepared.markdown,
                &prepared.file_path,
                prepared.type_def.as_ref(),
                &prepared.mem,
            ) {
                candidates.extend(parsed.entity.relationships.iter().map(|r| r.target.clone()));
            }
            crate::ops::signals::snapshot_levels(&self.store, &self.schemas, candidates.iter())
        };
        // Only when the update carried anchors or unsets that change
        // the sidecar.
        self.stage_prepared_update(&prepared, prepared.anchors_changed == Some(true))?;
        let backend = self.mounts[prepared.mount_idx].backend.as_ref();
        // Anchor-only commits carry the distinct `anchor` verb so their
        // otherwise-invisible sidecar change is legible in the note log;
        // every other update keeps `update`. The verb is a subject-only
        // signal — delta computation reads the tree diff, not the
        // subject, so the zero-entity-delta guarantee is untouched.
        let commit_subject = if prepared.anchor_only {
            format!("memstead: anchor {}", prepared.id)
        } else {
            format!("memstead: update {}", prepared.id)
        };
        let ctx = self.commit_context(
            Some("update_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        let write_id = backend.commit(&commit_subject, &ctx)?;
        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Update,
                Some(prepared.id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone()),
        )?;
        self.record_self_write(prepared.mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(prepared.mount_idx);

        let applied = self.apply_prepared_to_store(&prepared)?;

        self.invalidate_communities();
        // Incremental (flywheel W8/01): the updated entity is the
        // whole touched set — declared-relation stubs are never
        // indexed.
        self.maintain_search_indexes(std::slice::from_ref(&prepared.id));

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-commit path; the
        // no-op and dry-run prepare outcomes never demand a note.
        let mut warnings = prepared.warnings;
        warnings.extend(stamp_warnings);
        // Signal crossings — out-of-band beside the success payload,
        // never error-shaped.
        warnings.extend(crate::ops::signals::crossing_warnings(
            &self.store,
            &self.schemas,
            &signal_snapshot,
        ));
        if let Some(w) = self.note_missing_warning("update_entity", note) {
            warnings.push(w);
        }

        Ok(UpdateEntityOutcome {
            id: prepared.id.clone(),
            title: applied.title,
            file_path: prepared.file_path,
            content_hash: applied.content_hash,
            write_id,
            modified_date: prepared.modified_date,
            orphan_stubs_removed: applied.orphan_stubs_removed,
            modified_sections: prepared.modified_sections,
            modified_metadata: prepared.modified_metadata,
            prospective_hash: None,
            warnings,
            relations_declared: prepared.relations_declared,
            anchors_changed: prepared.anchors_changed,
        })
    }

    /// Parse the prepared markdown, push it into the in-memory store,
    /// re-map alias-target edge sources, and GC any stub the mutation
    /// orphaned. Shared post-commit store-application step for the
    /// single-update and batch paths — does NOT touch the backend or
    /// commit (the caller has already staged + committed the disk
    /// write).
    pub(super) fn apply_prepared_to_store(
        &mut self,
        prepared: &PreparedUpdate,
    ) -> Result<AppliedWrite, EngineError> {
        let parse_result = parse_markdown(
            &prepared.markdown,
            &prepared.file_path,
            prepared.type_def.as_ref(),
            &prepared.mem,
        )
        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();
        let title = parse_result.entity.title.clone();
        let fallback = engine_fallback_type();
        push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        let orphan_stubs_removed =
            super::super::gc_orphan_stubs_among(&mut self.store, &prepared.prev_body_targets);
        Ok(AppliedWrite {
            content_hash,
            title,
            orphan_stubs_removed,
        })
    }
}
