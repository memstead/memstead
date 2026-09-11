//! Commit phase of a single create: stage the prepared write, land
//! the commit with the canonical `memstead: create <id>` subject,
//! append provenance, record the self-write, and apply the change to
//! the in-memory store by re-parsing the bytes that hit disk. The
//! batch path drives the same steps in [`super::batch`], staging every
//! item first and committing once per mem.

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::project_incoming;
use crate::provenance::{Provenance, ProvenanceKind};

use super::super::make_stub;
use super::{
    Actor, ClientId, CommitContext, CreateEntityOutcome, Engine, EngineError, PreparedCreate,
};

impl Engine {
    /// Stage the prepared disk write, commit it, append provenance, and
    /// apply the change to the in-memory store — the single-create tail
    /// of [`Self::create_entity`]. The batch path drives the same
    /// steps but stages every item first and commits once per mem.
    pub(super) fn commit_prepared_create(
        &mut self,
        prepared: PreparedCreate,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<CreateEntityOutcome, EngineError> {
        // Aggregate signals: the entities a create can move are the
        // new entity itself (baseline all-`none`) and the targets of
        // its inline relations. Captured before the store mutates,
        // diffed after the push below.
        let signal_snapshot = {
            let mut candidates: Vec<&EntityId> = vec![&prepared.id];
            candidates.extend(prepared.relation_targets.iter());
            crate::ops::signals::snapshot_levels(&self.store, &self.schemas, candidates)
        };

        // 8. Write + commit through the backend. The commit subject
        //    is `memstead: create <id>` so the git-branch backend's
        //    `read_provenance` recovers the kind via the verb.
        self.stage_prepared_create(&prepared)?;
        let PreparedCreate {
            mount_idx,
            id,
            title,
            mem,
            file_path,
            markdown,
            anchors: _,
            mut warnings,
            type_guidance,
            relations_declared,
            relation_targets,
            type_def,
        } = prepared;
        let backend = self.mounts[mount_idx].backend.as_ref();
        let commit_subject = format!("memstead: create {id}");
        let ctx = CommitContext {
            actor,
            client: client.cloned(),
            tool: Some("create_entity"),
            note: note.map(String::from),
            role: self.current_role,
            identity: self.current_identity.clone(),
            logical_operation_id: None,
            entity_ids: None,
        };
        let write_id = backend.commit(&commit_subject, &ctx)?;

        // 9. Append provenance. Folder writes a JSONL line; git-branch
        //    no-ops (the commit object already carries the data).
        backend.append_provenance(
            &Provenance::new(
                std::time::SystemTime::now(),
                ProvenanceKind::Create,
                Some(id.to_string()),
                actor,
                client.cloned(),
                note.map(String::from),
            )
            .with_role(self.current_role)
            .with_identity(self.current_identity.clone()),
        )?;

        // Self-write bookkeeping: jump `last_known_head` to the SHA
        // we just produced so the next read doesn't surface
        // `MEM_RELOADED` for our own commit.
        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);

        // 10. Update the in-memory store via re-parse so the store
        //     mirrors the on-disk shape (content_hash, heading_spans).
        let parse_result = parse_markdown(&markdown, &file_path, type_def.as_ref(), &mem)
            .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
        let content_hash = parse_result.entity.content_hash.clone();

        // Extract `created_date` from the parsed entity's metadata
        // before pushing into the store (after push, the entity is
        // borrowed by the store and re-fetching costs a lookup).
        // The default schema's auto-timestamp fills `created_date`
        // with today's ISO date; the field is empty for schemas
        // that don't declare it.
        let created_date = parse_result
            .entity
            .metadata
            .get("created_date")
            .map(|v| v.to_frontmatter_string())
            .unwrap_or_default();

        let fallback = engine_fallback_type();
        push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );

        // Materialise stubs for any inline-relation targets that
        // weren't already in the store. Mirrors the relate path's
        // ensure_target — full's create relies on the
        // loader stubbing unresolved targets, but the unified
        // store doesn't auto-stub on push, so the engine does it
        // explicitly. Skipped when no relations were declared
        // (the args.relations vec is empty).
        for target in &relation_targets {
            if !self.store.contains(target) {
                let kind = super::super::deferred_verified_stub_kind(self, target)?;
                self.store.upsert(target.clone(), make_stub(target, kind));
            }
        }

        self.invalidate_communities();
        // Incremental (flywheel W8/01): the new entity is the whole
        // touched set — its stub targets are never indexed.
        self.maintain_search_indexes(std::slice::from_ref(&id));

        // Stub-adoption visibility: project the incoming edges that
        // survived the upsert. Empty for a fresh create; populated
        // when a pre-existing stub at this id had referrers.
        let incoming = project_incoming(self.store.incoming(&id));
        let incoming_count = (!incoming.is_empty()).then_some(incoming.len());

        // Signal crossings — out-of-band beside the success payload,
        // never error-shaped.
        warnings.extend(stamp_warnings);
        warnings.extend(crate::ops::signals::crossing_warnings(
            &self.store,
            &self.schemas,
            &signal_snapshot,
        ));

        // `require_notes` provenance nudge — single engine-level
        // enforcement point (see `Engine::note_missing_warning`). Only
        // reached on the real-write path (commit landed); the dry-run
        // early return above never demands a note.
        if let Some(w) = self.note_missing_warning("create_entity", note) {
            warnings.push(w);
        }

        Ok(CreateEntityOutcome {
            id,
            title,
            mem,
            file_path,
            content_hash,
            write_id,
            created_date,
            warnings,
            type_guidance,
            incoming_count,
            incoming,
            relations_declared,
        })
    }
}
