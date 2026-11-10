//! Commit phase of a rename: stage and commit the renamed entity's
//! own mem, then each peer mem pinned to the head the engine saw,
//! record the engine's own writes, apply the change to the in-memory
//! store by re-parsing the bytes that hit disk, and shape the outcome.
//! Every commit of one rename carries the same logical operation id.

use crate::engine_fallback_type;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::WarningHint;
use crate::provenance::ProvenanceKind;
use crate::vcs::{Actor, ClientId};

use super::super::make_stub;
use super::{Engine, EngineError, PreparedRename, RenameEntityOutcome};

impl Engine {
    /// Land a prepared rename. The caller holds the write lock.
    pub(super) fn commit_prepared_rename(
        &mut self,
        prepared: PreparedRename,
        mut drift_warnings: Vec<WarningHint>,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<RenameEntityOutcome, EngineError> {
        let PreparedRename {
            id,
            new_id,
            mem,
            mount_idx,
            new_title,
            derivation,
            type_def,
            markdown,
            old_file_path,
            new_file_path,
            same_mem,
            peers: peer_plans,
            readonly_referrers,
            ..
        } = &prepared;
        let (mount_idx, new_id, mem) = (*mount_idx, new_id.clone(), mem.clone());
        let (old_file_path, new_file_path) = (old_file_path.clone(), new_file_path.clone());
        let new_slug = derivation.slug.clone();
        let readonly_referrers = readonly_referrers.clone();

        // ----- Apply: renaming entity's own mem first -----
        // Mint a single `logical_operation_id` up front so every
        // commit produced by this rename — source mem + every peer
        // mem — carries the same correlation id in its provenance
        // entry. Single-mem renames also tag with an id (it just
        // maps to one commit); consumers branch on whether the id
        // recurs to identify a multi-commit logical operation.
        let logical_op_id = crate::provenance::mint_logical_operation_id();

        self.stage_rename_source(&prepared)?;
        // The check ledger follows the id too. It is workspace state, not
        // mem content, so it cannot ride the commit; it is carried before
        // the commit so a ledger that refuses stops the rename the way a
        // refused check stops a record, and a commit that then fails
        // leaves at worst a carried line under an id that never came to
        // be — never a renamed entity that lost its checks.
        self.carry_checks_across_rename(id, &new_id)?;
        let commit_subject = format!("memstead: rename {} → {new_id}", id);
        let mut ctx = self.commit_context(
            Some("rename_entity"),
            actor,
            client.cloned(),
            note.map(String::from),
        );
        ctx.logical_operation_id = Some(logical_op_id.as_str());
        // One record shape for every commit of this rename; each lands
        // with its mem's commit.
        let record = self
            .provenance_record(ProvenanceKind::Rename, &new_id, actor, client, note)
            .with_logical_operation_id(logical_op_id.clone());
        let landed = self.commit_staged(
            mount_idx,
            &commit_subject,
            &ctx,
            std::slice::from_ref(&record),
        )?;
        let write_id = landed.write_id;
        let mut follow_up_warnings = landed.warnings;
        let mut committed_mounts: Vec<usize> = vec![mount_idx];

        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);
        let mut peer_stamp_warnings: Vec<WarningHint> = Vec::new();

        // ----- Apply: cross-mem peer mems (parent-pinned) -----
        // Snapshot every peer mem's current head before any peer
        // writes begin. The snapshots are pinned through
        // `commit_with_expected_parent`, so a sibling writer that
        // advances a peer mem's head between snapshot and commit
        // aborts the commit with `BackendError::ParentMismatch`. The
        // engine layer maps this to `RENAME_PARTIAL_FAILURE` (the
        // source mem has already committed by this point — its
        // state is durable; only the failed peer's writes are lost).
        // Folder and archive backends inherit the trait's default
        // `commit_with_expected_parent` (which ignores the parent
        // and delegates to `commit`); the git-branch backend
        // overrides to check the per-mem branch tip.
        let mut peer_snapshots: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for plan in peer_plans.values() {
            let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
            let snapshot = peer_backend.current_head()?;
            peer_snapshots.insert(plan.mem.clone(), snapshot);
        }

        // Track which mems have already committed in this logical
        // operation. On a peer-commit failure, the engine surfaces
        // the partial-state envelope so the agent can decide whether
        // to retry, reconcile, or accept.
        let mut committed_mems: Vec<String> = vec![mem.clone()];
        for plan in peer_plans.values() {
            self.stage_rename_peer(plan)?;
            let peer_commit_subject = format!(
                "memstead: rename {} → {new_id} (cross-mem rewrite in `{}`)",
                id, plan.mem
            );
            let mut peer_ctx = self.commit_context(
                Some("rename_entity"),
                actor,
                client.cloned(),
                note.map(String::from),
            );
            peer_ctx.logical_operation_id = Some(logical_op_id.as_str());
            let expected = peer_snapshots.get(&plan.mem).cloned().unwrap_or(None);
            let peer_commit_result = self.commit_staged_pinned(
                plan.mount_idx,
                &peer_commit_subject,
                &peer_ctx,
                std::slice::from_ref(&record),
                expected.as_deref(),
            );
            let peer_write_id = match peer_commit_result {
                Ok(landed) => {
                    follow_up_warnings.extend(landed.warnings);
                    landed.write_id
                }
                Err(EngineError::Backend(crate::backend::BackendError::ParentMismatch {
                    ..
                })) => {
                    return Err(EngineError::RenamePartialFailure {
                        committed_mems: std::mem::take(&mut committed_mems),
                        failed_mem: plan.mem.clone(),
                        failure_cause: "drift".to_string(),
                    });
                }
                Err(e) => return Err(e),
            };
            committed_mounts.push(plan.mount_idx);
            self.record_self_write(plan.mount_idx, &peer_write_id);
            // Per peer mount, not only the source mem: a rename touches every
            // pinned peer, and each one's config write can meet its own
            // intervening writer.
            peer_stamp_warnings.extend(self.stamp_mutation_versions(plan.mount_idx));
            committed_mems.push(plan.mem.clone());
        }

        // ----- Re-parse and push -----
        // A failed re-parse reloads every committed mem from storage,
        // where the rename is; the store edits below are then already
        // done by the reload and the receipt reads from that view.
        let parsed = self.after_commits(
            &committed_mounts,
            "store_update",
            &mut follow_up_warnings,
            |_engine| {
                let parse_result =
                    parse_markdown(markdown, &new_file_path, type_def.as_ref(), &mem)
                        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                let mut parse_results = vec![parse_result];
                for rewrite in same_mem {
                    let pr = parse_markdown(
                        &rewrite.markdown,
                        &rewrite.file_path,
                        rewrite.type_def.as_ref(),
                        &mem,
                    )
                    .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                    parse_results.push(pr);
                }
                for plan in peer_plans.values() {
                    for rewrite in &plan.rewrites {
                        let pr = parse_markdown(
                            &rewrite.markdown,
                            &rewrite.file_path,
                            rewrite.type_def.as_ref(),
                            &plan.mem,
                        )
                        .map_err(|e| EngineError::ParseAfterWrite(e.to_string()))?;
                        parse_results.push(pr);
                    }
                }
                Ok(parse_results)
            },
        );

        // Residual-stub demotion for ReadOnly cross-mem referrers.
        // The engine can't rewrite ReadOnly-mount markdown, so the
        // wiki-links there still point at the OLD slug after the
        // rename. To keep `incoming(<new_id>)` aligned with what a
        // fresh boot would produce (and to surface the dangling
        // reference to the agent), we demote the OLD-id store entry
        // to a stub instead of removing it outright. Its surviving
        // `in_edges` from the ReadOnly mount remain valid — they
        // point at the now-stub at the old id.
        //
        // When no ReadOnly referrers exist, the old entry is
        // removed cleanly (the existing Write-path behaviour).
        let mut outcome_warnings: Vec<WarningHint> = Vec::new();
        outcome_warnings.extend(follow_up_warnings);
        outcome_warnings.extend(stamp_warnings);
        outcome_warnings.extend(peer_stamp_warnings);
        // Reload-before-operation drift notice, surfaced first.
        outcome_warnings.append(&mut drift_warnings);
        // Title↔slug divergence of the NEW title — same visibility
        // contract as create's.
        if !derivation.dropped_chars.is_empty() {
            outcome_warnings.push(WarningHint::TitleCharsDroppedFromSlug {
                title: new_title.trim().to_string(),
                dropped_chars: derivation.dropped_chars.clone(),
                slug: new_slug.clone(),
            });
        }
        if let Some(parse_results) = parsed {
            if readonly_referrers.is_empty() {
                self.store.remove(id);
            } else {
                // Sever outgoing edges from the old id (the entity is
                // gone — its body and relations live at the new id now)
                // and replace the node with a stub at the same id. The
                // in_edges from the ReadOnly mount survive untouched.
                self.store.remove_edges_from(id);
                self.store.upsert(
                    id.clone(),
                    make_stub(
                        id,
                        crate::entity::StubKind::Residual {
                            since_commit: write_id.clone(),
                            readonly_referrers: readonly_referrers.clone(),
                        },
                    ),
                );
            }

            let fallback = engine_fallback_type();
            // Incremental (flywheel W8/01): the touched set is the OLD id
            // (its document must leave the index) plus every re-parsed
            // entity — the renamed entity at its new id and each rewritten
            // referrer.
            let mut touched: Vec<crate::EntityId> = parse_results
                .iter()
                .map(|pr| pr.entity.id.clone())
                .collect();
            touched.push(id.clone());
            push_entities_into_store(&mut self.store, parse_results, fallback.as_ref(), None);
            crate::entity::store_builder::remap_alias_target_edge_sources(
                &mut self.store,
                &self.schemas,
            );

            self.invalidate_communities();
            self.maintain_search_indexes(&touched);
        }
        if !readonly_referrers.is_empty() {
            outcome_warnings.push(WarningHint::ResidualStubForReadOnlyReferrers {
                id: id.clone(),
                referrers: readonly_referrers,
            });
        }
        let content_hash = self
            .store
            .get(&new_id)
            .map(|e| e.content_hash.clone())
            .unwrap_or_default();

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Only reached on the real-rename path; the
        // slug-noop short-circuit returns early above with an empty
        // `write_id` and never demands a note.
        if let Some(w) = self.note_missing_warning("rename_entity", note) {
            outcome_warnings.push(w);
        }

        Ok(RenameEntityOutcome {
            old_id: id.clone(),
            new_id,
            old_path: old_file_path,
            new_path: new_file_path,
            content_hash,
            write_id,
            warnings: outcome_warnings,
        })
    }
}
