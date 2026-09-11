//! `Engine::batch_create`: the same phases as a single create, driven
//! over every entry before anything is written — an identity pass
//! that refuses duplicates against the store and within the batch,
//! skeleton staging so intra-batch references validate as real
//! targets, the per-entry prepare (resolve, validate, compose), then
//! stage everything and commit once per mem. A refused or rehearsed
//! batch rolls the store snapshot back and discards every pending
//! buffer.

use std::collections::HashMap;

use crate::engine_fallback_type;
use crate::entity::EntityId;
use crate::entity::id::validate_and_derive_slug;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};

use super::super::{batch_empty, batch_receipt, batch_refusal, make_stub};
use super::{
    Actor, ClientId, CreateEntityArgs, CreatePrepareOutcome, Engine, EngineError, PreparedCreate,
};

impl Engine {
    /// Atomic batch create — the create-side sibling of
    /// [`Self::batch_update`], with one upgrade and one addition:
    ///
    /// - **Report-all refusal.** Every failing entry is identified with
    ///   its index and typed `{code, message, details}` envelope (the
    ///   family's upgraded contract) — bounded at
    ///   [`Self::BATCH_ERROR_REPORT_CAP`] detailed envelopes, with
    ///   `errors_suppressed` counting the rest. A refused batch writes
    ///   NOTHING: no entity, no edge, no head movement.
    /// - **Intra-batch references resolve as REAL targets.** Every
    ///   entity in the batch is staged (a skeleton store entry carrying
    ///   its declared type) before per-entry validation runs, so an
    ///   edge to a sibling created in the same batch gets full
    ///   target-type shape validation, no transient stub, and no stub
    ///   warning — the batch validates as one graph state, cycles
    ///   included where the schema permits them. Duplicates within the
    ///   batch are refused in the identity pass.
    ///
    /// One workspace load (the caller's), one commit per touched mem
    /// (subject `memstead: batch-create (N entities)`), per-entry
    /// provenance notes exactly like `batch_update`.
    ///
    /// **Rehearsal** (`dry_run: true`): the FULL validation pass runs —
    /// identity, skeleton staging (so intra-batch references resolve as
    /// real targets, cycles included), per-entry prepare, report-all
    /// refusals — then the batch stops before any write. A legal batch
    /// returns the would-be receipt (`applied: true`, per-entry
    /// `"created"` with the prospective ids) with the marker form's
    /// empty `write_id`; an illegal one returns the same refusal a
    /// real call would. Nothing is written, committed, or stubbed.
    pub fn batch_create(
        &mut self,
        creates: Vec<(CreateEntityArgs, Option<String>)>,
        actor: Actor,
        client: Option<&ClientId>,
        dry_run: bool,
    ) -> Result<crate::ops::BatchResult, EngineError> {
        use std::collections::HashSet;

        if creates.is_empty() {
            return Ok(batch_empty());
        }

        // Reload every touched mem once, up front.
        let mut touched_mems: Vec<String> = creates.iter().map(|(a, _)| a.mem.clone()).collect();
        touched_mems.sort();
        touched_mems.dedup();
        for m in &touched_mems {
            self.reload_if_stale(Some(m));
        }
        // Same acyclic-guard rule as the single-item path: declared
        // relations on an ACYCLIC rel-type (or one in an
        // `acyclic_sets` set) walk the whole subgraph, so the walk
        // must see every mem — deferred ones included (see the
        // batch_relate comment). Declared signals on any involved
        // schema need the full load too (see the single-item path).
        if creates.iter().any(|(a, _)| {
            a.relations.iter().any(|r| {
                self.schemas.get(&a.mem).is_some_and(|s| {
                    s.relationship_acyclic(&r.rel_type)
                        || s.acyclic_set_containing(&r.rel_type).is_some()
                }) || self
                    .schemas
                    .get(r.target.mem())
                    .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
            }) || self
                .schemas
                .get(&a.mem)
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        }) {
            self.ensure_mems_loaded(None);
        }

        let store_snapshot = self.store.clone();

        // --- Identity pass: derive every entry's id, refusing
        // duplicates against the pre-batch store AND within the batch.
        // Collect EVERY failure (report-all), never just the first.
        struct IdentityRow {
            id: Option<EntityId>,
            error: Option<EngineError>,
        }
        let mut rows: Vec<IdentityRow> = Vec::with_capacity(creates.len());
        // id → title of the batch entry that claimed it, so a
        // within-batch duplicate can name the occupying title.
        let mut batch_ids: HashMap<EntityId, String> = HashMap::new();
        for (args, _) in &creates {
            let identity = (|| -> Result<EntityId, EngineError> {
                let title = args.title.trim();
                // Divergence warnings ride the per-entry prepare pass
                // below, which re-derives; this pass only needs the id.
                let slug = validate_and_derive_slug(title)?.slug;
                let id = EntityId::new(&args.mem, &slug);
                crate::entity::id::enforce_id_length(id.as_ref())?;
                if let Some(existing) = self.store.get(&id)
                    && !existing.stub
                {
                    return Err(EngineError::AlreadyExists {
                        id: id.to_string(),
                        existing_title: existing.title.clone(),
                        existing_is_stub: false,
                    });
                }
                if let Some(prior_title) = batch_ids.get(&id) {
                    // Duplicate WITHIN the batch — same typed code as
                    // the store collision; the index in the report
                    // localises it.
                    return Err(EngineError::AlreadyExists {
                        id: id.to_string(),
                        existing_title: prior_title.clone(),
                        existing_is_stub: false,
                    });
                }
                Ok(id)
            })();
            match identity {
                Ok(id) => {
                    batch_ids.insert(id.clone(), args.title.trim().to_string());
                    rows.push(IdentityRow {
                        id: Some(id),
                        error: None,
                    });
                }
                Err(e) => rows.push(IdentityRow {
                    id: None,
                    error: Some(e),
                }),
            }
        }

        // --- Skeleton staging: make every batch id a REAL, typed store
        // entry so sibling references validate against present targets.
        // A pre-existing stub at a batch id is replaced (its incoming
        // edges survive the upsert — the same adoption the single-item
        // create performs).
        for ((args, _), row) in creates.iter().zip(rows.iter()) {
            if let Some(id) = &row.id {
                let mut skeleton = make_stub(id, crate::entity::StubKind::ForwardReference);
                skeleton.stub = false;
                skeleton.stub_kind = None;
                skeleton.entity_type = args.entity_type.clone();
                skeleton.title = args.title.trim().to_string();
                self.store.upsert(id.clone(), skeleton);
            }
        }

        // --- Full prepare pass, report-all. Skeletons make intra-batch
        // targets real; each entry's own skeleton is exempted from the
        // duplicate check via `batch_skeleton_ids`.
        let mut prepared: Vec<PreparedCreate> = Vec::new();
        let mut notes: Vec<Option<String>> = Vec::new();
        let mut errors: Vec<(usize, EngineError)> = Vec::new();
        let mut ids_in_order: Vec<EntityId> = Vec::new();
        let skeleton_ids: HashSet<EntityId> = batch_ids.keys().cloned().collect();
        for (i, ((args, note), row)) in creates.into_iter().zip(rows).enumerate() {
            let fallback_id = row
                .id
                .clone()
                .unwrap_or_else(|| EntityId::new(&args.mem, "invalid-entry"));
            ids_in_order.push(fallback_id);
            if let Some(e) = row.error {
                errors.push((i, e));
                continue;
            }
            // Rehearsal is batch-level (the `dry_run` parameter) —
            // per-entry dry-run stays forced off so the prepare pass
            // below never short-circuits into a per-entry preview.
            let mut args = args;
            args.dry_run = false;
            match self.prepare_create(args, Some(&skeleton_ids), Vec::new()) {
                Ok(CreatePrepareOutcome::Prepared(p)) => {
                    // Stage this item's declared edges onto its skeleton
                    // so later items validate against the batch's own
                    // graph state — an intra-batch cycle on an acyclic
                    // rel-type refuses exactly like a stored one
                    // (`validate_edge_acyclicity` walks the store). The
                    // snapshot rollback discards these on refusal; the
                    // apply pass replaces them with the parsed truth.
                    for r in &p.relations_declared {
                        self.store.add_edge(
                            p.id.clone(),
                            crate::store::Edge {
                                rel_type: r.rel_type.clone(),
                                target: r.target.clone(),
                                source: crate::store::EdgeSource::Explicit,
                            },
                        );
                    }
                    ids_in_order[i] = p.id.clone();
                    prepared.push(p);
                    notes.push(note);
                }
                Ok(CreatePrepareOutcome::Done(_)) => unreachable!("dry_run forced off"),
                Err(e) => errors.push((i, e)),
            }
        }

        if !errors.is_empty() {
            // Refuse the whole batch; nothing was committed and the
            // store snapshot rolls back the skeletons.
            self.store = store_snapshot;
            self.discard_all_pending();
            return Ok(batch_refusal(ids_in_order, errors));
        }

        // Rehearsal: every entry validated against the batch's own
        // graph state (skeletons made intra-batch targets real) and
        // nothing failed — stop before any write. Roll back the
        // skeleton staging and return the would-be receipt with the
        // marker form's empty `write_id`.
        if dry_run {
            self.store = store_snapshot;
            self.discard_all_pending();
            return Ok(batch_receipt(
                created_actions(prepared),
                Vec::new(),
                Vec::new(),
                String::new(),
            ));
        }

        // --- Stage every write + anchors, then commit once per mem.
        for p in &prepared {
            if let Err(e) = self.stage_prepared_create(p) {
                self.store = store_snapshot;
                self.discard_all_pending();
                return Err(e);
            }
        }
        let mut distinct_mounts: Vec<usize> = Vec::new();
        for p in &prepared {
            if !distinct_mounts.contains(&p.mount_idx) {
                distinct_mounts.push(p.mount_idx);
            }
        }
        let mut mount_commits: Vec<(usize, String)> = Vec::with_capacity(distinct_mounts.len());
        for &m in &distinct_mounts {
            let entity_ids: Vec<String> = prepared
                .iter()
                .filter(|p| p.mount_idx == m)
                .map(|p| p.id.to_string())
                .collect();
            let count = entity_ids.len();
            let subject = format!("memstead: batch-create ({count} entities)");
            // Per-entry notes ride the ONE batch commit's note record as
            // `<id>: <note>` lines (decision 3, an earlier plan):
            // `append_provenance` below is a documented no-op on the
            // git-branch backend, so without this the notes survived
            // nowhere exactly where most writes happen. A batch with no
            // notes carries no note record at all.
            let note_lines: Vec<String> = prepared
                .iter()
                .zip(notes.iter())
                .filter(|(p, _)| p.mount_idx == m)
                .filter_map(|(p, n)| n.as_ref().map(|n| format!("{}: {n}", p.id)))
                .collect();
            let mut ctx = self.commit_context(
                Some("batch_create"),
                actor,
                client.cloned(),
                if note_lines.is_empty() {
                    None
                } else {
                    Some(note_lines.join("\n"))
                },
            );
            ctx.entity_ids = Some(entity_ids);
            match self.mounts[m].backend.commit(&subject, &ctx) {
                Ok(sha) => mount_commits.push((m, sha)),
                Err(e) => {
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    return Err(e.into());
                }
            }
        }

        // Provenance + store application (parse the generated bytes so
        // the store mirrors disk, replacing the skeletons).
        let fallback = engine_fallback_type();
        let mut batch_warnings: Vec<WarningHint> = Vec::new();
        for (p, note) in prepared.iter().zip(notes.iter()) {
            let write_id = mount_commits
                .iter()
                .find(|(m, _)| *m == p.mount_idx)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            self.mounts[p.mount_idx].backend.append_provenance(
                &Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Create,
                    Some(p.id.to_string()),
                    actor,
                    client.cloned(),
                    note.clone(),
                )
                .with_role(self.current_role)
                .with_identity(self.current_identity.clone()),
            )?;
            self.record_self_write(p.mount_idx, &write_id);
            batch_warnings.extend(self.stamp_mutation_versions(p.mount_idx));
            let parse_result =
                parse_markdown(&p.markdown, &p.file_path, p.type_def.as_ref(), &p.mem)
                    .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
            push_entities_into_store(&mut self.store, vec![parse_result], fallback.as_ref(), None);
        }
        crate::entity::store_builder::remap_alias_target_edge_sources(
            &mut self.store,
            &self.schemas,
        );
        // Forward-reference stubs for OUT-OF-BATCH targets only —
        // in-batch targets are real entities now.
        let mut out_of_batch_stubs: Vec<(EntityId, crate::entity::StubKind)> = Vec::new();
        for p in &prepared {
            for target in &p.relation_targets {
                if !self.store.contains(target) {
                    let kind = super::super::deferred_verified_stub_kind(self, target)?;
                    out_of_batch_stubs.push((target.clone(), kind));
                }
            }
        }
        for (target, kind) in out_of_batch_stubs {
            self.store.upsert(target.clone(), make_stub(&target, kind));
        }
        self.invalidate_communities();
        self.invalidate_search_indexes();

        let write_id = mount_commits
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        Ok(batch_receipt(
            created_actions(prepared),
            batch_warnings,
            Vec::new(),
            write_id,
        ))
    }
}

/// The create verb's one action word: every prepared item was `created`.
fn created_actions(prepared: Vec<PreparedCreate>) -> Vec<(EntityId, String)> {
    prepared
        .into_iter()
        .map(|p| (p.id, "created".to_string()))
        .collect()
}
