//! `Engine::batch_update`: the same phases as a single update, driven
//! over every item before anything is written — short ids resolved
//! up front, every touched mem reloaded once, the per-item prepare
//! (resolve, validate, compose) with report-all refusal, then stage
//! everything and commit once per mem. A refused or rehearsed batch
//! rolls the store snapshot back and discards every pending buffer.

use crate::entity::EntityId;
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};

use super::outcome::{batch_receipt, batch_refusal};
use super::{
    Actor, ClientId, Engine, EngineError, PrepareOutcome, PreparedUpdate, UpdateEntityArgs,
};

/// What each batch item is, in submission order, so the result
/// entries echo the input order. `Prepared` is a real write (its
/// `PreparedUpdate` lives in the batch's prepared list); `Noop` is an
/// applied no-op (content unchanged, no write); `Error` is a refusal
/// (its envelope lives in the batch's errors by index).
pub(super) enum BatchItem {
    Prepared,
    Noop,
    Error,
}

impl Engine {
    /// Apply a batch of [`UpdateEntityArgs`] **atomically** — all or
    /// nothing. Surfaces `BatchResult` for `memstead batch-update`
    /// consumers.
    ///
    /// The batch validates and prepares every item first (each with
    /// its own optimistic-lock check), then commits the whole set as
    /// **one** commit per mem. If any item fails — validation error,
    /// `HASH_MISMATCH`, entity-not-found, any per-item refusal —
    /// **nothing is committed**: the on-disk mem and the in-memory
    /// store are restored to exactly their pre-call state, and the
    /// result is marked `applied: false` with EVERY failing item
    /// carrying a typed `{code, message, details}` error envelope
    /// (the family's report-all contract — bounded at
    /// [`Self::BATCH_ERROR_REPORT_CAP`] detailed envelopes, with
    /// `errors_suppressed` counting the rest) and every valid item
    /// marked `"not_applied"`, so one repair cycle fixes the file.
    ///
    /// On success the returned `write_id` is the single batch commit
    /// — an honest `memstead_changes_since` cursor / revert handle. Each
    /// item's per-entry note rides into its own provenance record.
    ///
    /// Empty batches return `applied: true` with zero counts and no
    /// commit. A batch where every item is a no-op (content unchanged)
    /// likewise applies with an empty `write_id`.
    ///
    /// **Rehearsal** (`dry_run: true`): the FULL per-item validation
    /// pass runs — identical refusals, identical report-all envelope —
    /// then the batch stops before any write or commit. A legal batch
    /// returns the would-be receipt (`applied: true`, per-entry
    /// actions) with the marker form's empty `write_id`; an illegal
    /// one returns the same refusal a real call would. Nothing is
    /// staged, committed, or stamped.
    ///
    /// Atomicity is per-mem: for the common single-mem batch a
    /// commit-time backend failure rolls the whole batch back. A batch
    /// spanning multiple mems commits each mem in turn; if a later
    /// mem's commit fails, already-committed mems stay committed
    /// (true cross-mem two-phase commit is out of scope) — but the
    /// dominant failure mode, a per-item validation/hash refusal, is
    /// always fully atomic because no commit happens until every item
    /// has passed.
    pub fn batch_update(
        &mut self,
        updates: Vec<(UpdateEntityArgs, Option<String>)>,
        actor: Actor,
        client: Option<&ClientId>,
        dry_run: bool,
    ) -> Result<crate::ops::BatchResult, EngineError> {
        if updates.is_empty() {
            return Ok(crate::ops::BatchResult {
                warnings: Vec::new(),
                orphan_stubs_removed: Vec::new(),
                errors_suppressed: 0,
                applied: true,
                results: Vec::new(),
                succeeded: 0,
                failed: 0,
                write_id: String::new(),
            });
        }

        // Reload-before-operation: refresh every mem this batch
        // touches *before* preparing items, so each item's
        // `expected_hash` check runs against current truth (the batch
        // is the one multi-op-per-process path, so a sibling commit
        // between boot and this call is plausible). Notices stash on
        // the engine for the caller to drain.
        // Short ids resolve once, before the touched-mem probe reads
        // each item's mem; an item that does not resolve fails as its
        // own error below, never the whole batch.
        let mut short_hints: Vec<WarningHint> = Vec::new();
        let mut short_errors: Vec<(usize, EngineError)> = Vec::new();
        let updates: Vec<(UpdateEntityArgs, Option<String>)> = updates
            .into_iter()
            .enumerate()
            .map(|(i, (mut a, n))| {
                match self.resolve_entity_id(&a.id) {
                    Ok((rid, hint)) => {
                        a.id = rid;
                        short_hints.extend(hint);
                    }
                    Err(e) => short_errors.push((i, e)),
                }
                for r in &mut a.declare_relations {
                    match self.resolve_entity_id(&r.target) {
                        Ok((t, hint)) => {
                            r.target = t;
                            short_hints.extend(hint);
                        }
                        Err(e) => short_errors.push((i, e)),
                    }
                }
                (a, n)
            })
            .collect();
        let mut touched_mems: Vec<String> = updates
            .iter()
            .map(|(a, _)| a.id.mem().to_string())
            .collect();
        touched_mems.sort();
        touched_mems.dedup();
        for v in &touched_mems {
            self.reload_if_stale(Some(v));
        }
        // Same acyclic-guard rule as the single-item path: declared
        // relations on an ACYCLIC rel-type (or one in an
        // `acyclic_sets` set) walk the whole subgraph, so the walk
        // must see every mem — deferred ones included (see the
        // batch_relate comment). Declared signals on any involved
        // schema need the full load too (see the single-item path).
        if updates.iter().any(|(a, _)| {
            a.declare_relations.iter().any(|r| {
                self.schemas.get(a.id.mem()).is_some_and(|s| {
                    s.relationship_acyclic(&r.rel_type)
                        || s.acyclic_set_containing(&r.rel_type).is_some()
                }) || self
                    .schemas
                    .get(r.target.mem())
                    .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
            }) || self
                .schemas
                .get(a.id.mem())
                .is_some_and(|s| s.types.values().any(|td| !td.signals.is_empty()))
        }) {
            self.ensure_mems_loaded(None);
        }

        // Snapshot the in-memory store so a refused batch (or a
        // commit-time backend failure) can roll back any auto-stubs
        // and store pushes that earlier items already applied during
        // preparation. The on-disk side rolls back by discarding each
        // backend's staged-but-uncommitted pending buffer.
        let store_snapshot = self.store.clone();

        let mut items: Vec<(EntityId, BatchItem)> = Vec::with_capacity(updates.len());
        let mut prepared: Vec<PreparedUpdate> = Vec::new();
        let mut notes: Vec<Option<String>> = Vec::new();
        let mut errors: Vec<(usize, EngineError)> = Vec::new();

        // --- Phase 1: validate + prepare every item (no commits).
        // Report-all: a failing item never stops preparation — every
        // remaining item still validates so the refusal can name every
        // failing entry at once (the family's upgraded contract).
        for (i, (args, note)) in updates.into_iter().enumerate() {
            let id = args.id.clone();
            if let Some(pos) = short_errors.iter().position(|(j, _)| *j == i) {
                let (_, e) = short_errors.remove(pos);
                items.push((id, BatchItem::Error));
                errors.push((i, e));
                continue;
            }
            // Rehearsal is batch-level (the `dry_run` parameter) —
            // force the per-entry flag off so `Done` below always
            // means a genuine content no-op, never a per-entry
            // dry-run short-circuit misread as one.
            let mut args = args;
            args.dry_run = false;
            match self.prepare_update(args) {
                Ok(PrepareOutcome::Done(_)) => {
                    // No-op: applied, no write.
                    items.push((id, BatchItem::Noop));
                }
                Ok(PrepareOutcome::Prepared(p)) => {
                    prepared.push(p);
                    notes.push(note);
                    items.push((id, BatchItem::Prepared));
                }
                Err(e) => {
                    items.push((id, BatchItem::Error));
                    errors.push((i, e));
                }
            }
        }

        if !errors.is_empty() {
            // Refuse the whole batch. Roll back store + disk, then
            // report every failing entry — bounded at
            // `BATCH_ERROR_REPORT_CAP` detailed envelopes with
            // `errors_suppressed` counting the rest.
            self.store = store_snapshot;
            self.discard_all_pending();
            return Ok(batch_refusal(items, errors));
        }

        // Rehearsal: every item validated (the pass above is the same
        // one a real batch runs), nothing failed — stop before any
        // write. Roll back the prepare pass's store effects, discard
        // any pending buffers, and return the would-be receipt with
        // the marker form's empty `write_id`.
        if dry_run {
            self.store = store_snapshot;
            self.discard_all_pending();
            return Ok(batch_receipt(items, Vec::new(), String::new()));
        }

        // --- Phase 2: stage every prepared write, then commit once
        // per mem. --- Each item's anchors are staged into the same
        // per-mem pending buffer so they ride the batch commit
        // atomically.
        for p in &prepared {
            if let Err(e) =
                self.stage_prepared_update(p, !p.anchors.is_empty() || !p.anchor_unsets.is_empty())
            {
                self.store = store_snapshot;
                self.discard_all_pending();
                return Err(e);
            }
        }

        // Distinct mount indices in first-seen order — one commit each.
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
            let subject = format!("memstead: batch-update ({count} entities)");
            // Per-entry notes ride the batch commit's note record as
            // `<id>: <note>` lines (decision 3) — `append_provenance` is a
            // no-op on the git-branch backend, so this record is where
            // they survive. No notes → no note record.
            let note_lines: Vec<String> = prepared
                .iter()
                .zip(notes.iter())
                .filter(|(p, _)| p.mount_idx == m)
                .filter_map(|(p, n)| n.as_ref().map(|n| format!("{}: {n}", p.id)))
                .collect();
            let mut ctx = self.commit_context(
                Some("batch_update"),
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
                    // A commit failed. Roll back the store and any
                    // still-pending backends. Mems already committed
                    // in this loop stay committed (per-mem atomicity).
                    self.store = store_snapshot;
                    self.discard_all_pending();
                    return Err(e.into());
                }
            }
        }

        // Provenance + store application per item, now that the commits
        // landed. `record_self_write` marks the commit as engine-self
        // so drift detection ignores it.
        let mut batch_warnings: Vec<WarningHint> = short_hints;
        for (p, note) in prepared.iter().zip(notes.iter()) {
            let write_id = mount_commits
                .iter()
                .find(|(m, _)| *m == p.mount_idx)
                .map(|(_, s)| s.clone())
                .unwrap_or_default();
            self.mounts[p.mount_idx].backend.append_provenance(
                &Provenance::new(
                    std::time::SystemTime::now(),
                    ProvenanceKind::Update,
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
            self.apply_prepared_to_store(p)?;
        }

        self.invalidate_communities();
        self.invalidate_search_indexes();

        // Single-mem batches name their one commit; multi-mem names
        // the last mem committed (see the method docstring).
        let write_id = mount_commits
            .last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        Ok(batch_receipt(items, batch_warnings, write_id))
    }

    /// Best-effort discard of every backend's staged-but-uncommitted
    /// pending buffer — the disk-side half of an atomic-batch rollback.
    /// Discard errors (a poisoned pending mutex) are swallowed: we are
    /// already unwinding a refused batch and have nothing better to do.
    pub(in crate::engine::mutation) fn discard_all_pending(&self) {
        for mount in &self.mounts {
            let _ = mount.backend.discard_pending();
        }
    }
}
