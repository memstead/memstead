//! Commit phase of a rename: stage and commit each peer mem, pinned to
//! the head the engine saw, then the renamed entity's own mem; record
//! the engine's own writes, apply the change to the in-memory store by
//! re-parsing the bytes that hit disk, and shape the outcome. Every
//! commit of one rename carries the same logical operation id. A
//! commit that fails after another landed ends in the partial-failure
//! envelope.

use crate::engine_fallback_type;
use crate::entity::parser::parse_markdown;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::WarningHint;
use crate::provenance::ProvenanceKind;
use crate::vcs::{Actor, ClientId};

use super::super::make_stub;
use super::{Engine, EngineError, PreparedRename, RenameEntityOutcome};
use crate::engine::error::{PartialItem, PartialMem};

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

        // Mint a single `logical_operation_id` up front so every
        // commit produced by this rename — every peer mem + the source
        // mem — carries the same correlation id in its provenance
        // entry. Single-mem renames also tag with an id (it just
        // maps to one commit); consumers branch on whether the id
        // recurs to identify a multi-commit logical operation.
        let logical_op_id = crate::provenance::mint_logical_operation_id();
        // One record shape for every commit of this rename; each lands
        // with its mem's commit.
        let record = self
            .provenance_record(ProvenanceKind::Rename, &new_id, actor, client, note)
            .with_logical_operation_id(logical_op_id.clone())
            .with_renamed_from(id.to_string());
        // Every entry whose links the rename rewrites is written by this
        // identity too: one record per rewritten entry beside the rename's
        // own (the folder ledger's lines), and the same ids in the commit's
        // `Entities:` trailer (the git-branch notes). Independence counts
        // every identity that wrote an entry, so a renamer cannot confirm
        // the entries its rename rewrote.
        let records_of = |rewrites: &[super::ReferrerRewrite]| {
            let mut records = vec![record.clone()];
            for r in rewrites {
                records.push(
                    self.provenance_record(ProvenanceKind::Update, &r.id, actor, client, note)
                        .with_logical_operation_id(logical_op_id.clone()),
                );
            }
            records
        };
        let source_records = records_of(same_mem);
        let peer_records: std::collections::BTreeMap<String, Vec<_>> = peer_plans
            .values()
            .map(|plan| (plan.mem.clone(), records_of(&plan.rewrites)))
            .collect();
        let rewrite_ids = |rewrites: &[super::ReferrerRewrite]| {
            (!rewrites.is_empty()).then(|| {
                rewrites
                    .iter()
                    .map(|r| r.id.to_string())
                    .collect::<Vec<_>>()
            })
        };

        // Every peer's head, taken before the first write and pinned
        // through `commit_with_expected_parent`. Each peer is locked
        // and was reloaded under its lock, so no writer that takes the
        // lock can move it; the pin is for one that does not (a git
        // operation made by hand). The folder backend has no parent to
        // pin and ignores it.
        let mut peer_snapshots: std::collections::BTreeMap<String, Option<String>> =
            std::collections::BTreeMap::new();
        for plan in peer_plans.values() {
            let peer_backend = self.mounts[plan.mount_idx].backend.as_ref();
            let snapshot = peer_backend.current_head()?;
            peer_snapshots.insert(plan.mem.clone(), snapshot);
        }

        // ----- Apply: the peer mems first -----
        // The commit of the renamed entity's own mem is the one that
        // uses up what the caller sent: after it the old id is gone and
        // the hash is stale. It lands last, so a rename that fails part
        // of the way leaves the entity where the caller named it and
        // the same call completes the rename.
        let mut committed: Vec<(usize, String)> = Vec::new();
        let mut follow_up_warnings: Vec<WarningHint> = Vec::new();
        let mut peer_stamp_warnings: Vec<WarningHint> = Vec::new();
        for plan in peer_plans.values() {
            let landed = self.stage_rename_peer(plan).and_then(|()| {
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
                peer_ctx.entity_ids = rewrite_ids(&plan.rewrites);
                let expected = peer_snapshots.get(&plan.mem).cloned().unwrap_or(None);
                self.commit_or_complete(
                    plan.mount_idx,
                    self.commit_staged_pinned(
                        plan.mount_idx,
                        &peer_commit_subject,
                        &peer_ctx,
                        &peer_records[&plan.mem],
                        expected.as_deref(),
                    ),
                )
            });
            let landed = match landed {
                Ok(landed) => landed,
                Err(cause) => {
                    return Err(self.rename_failed_at(&prepared, &plan.mem, committed, cause));
                }
            };
            follow_up_warnings.extend(landed.warnings);
            self.record_self_write(plan.mount_idx, &landed.write_id);
            // Per peer mount, not only the source mem: a rename touches every
            // pinned peer, and each one's config write can meet its own
            // intervening writer.
            peer_stamp_warnings.extend(self.stamp_mutation_versions(plan.mount_idx));
            committed.push((plan.mount_idx, landed.write_id));
        }

        // ----- Apply: the renamed entity's own mem, last -----
        let landed = self
            .stage_rename_source(&prepared)
            // The check ledger follows the id too. It is workspace state, not
            // mem content, so it cannot ride the commit; it is carried before
            // the commit so a ledger that refuses stops the rename the way a
            // refused check stops a record, and a commit that then fails
            // leaves at worst a carried line under an id that never came to
            // be — never a renamed entity that lost its checks.
            .and_then(|()| self.carry_checks_across_rename(id, &new_id).map(|_| ()))
            .and_then(|()| {
                let commit_subject = format!("memstead: rename {} → {new_id}", id);
                let mut ctx = self.commit_context(
                    Some("rename_entity"),
                    actor,
                    client.cloned(),
                    note.map(String::from),
                );
                ctx.logical_operation_id = Some(logical_op_id.as_str());
                ctx.entity_ids = rewrite_ids(same_mem);
                self.commit_or_complete(
                    mount_idx,
                    self.commit_staged(mount_idx, &commit_subject, &ctx, &source_records),
                )
            });
        let landed = match landed {
            Ok(landed) => landed,
            Err(cause) => return Err(self.rename_failed_at(&prepared, &mem, committed, cause)),
        };
        let write_id = landed.write_id;
        follow_up_warnings.extend(landed.warnings);
        let mut committed_mounts: Vec<usize> = committed.iter().map(|(idx, _)| *idx).collect();
        committed_mounts.push(mount_idx);

        self.record_self_write(mount_idx, &write_id);
        let stamp_warnings = self.stamp_mutation_versions(mount_idx);

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
            // Incremental: the touched set is the OLD id
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

    /// The report for a rename that failed at `failed_mem`. With
    /// nothing in storage the rename failed as a whole and `cause` is
    /// the report: storage is as it was, and the same call sent again
    /// is a first call. A rename with no peer mem reports `cause` too.
    /// With a commit in storage (a mem that committed, or the failed
    /// mem when its commit is decided and not applied) the report is
    /// the partial-failure envelope, built from a view that is level
    /// with storage: those mems are reloaded first, and each entry
    /// carries the hash its entity has now. What the failed mem
    /// had staged is discarded when the write lock is released, before
    /// any other operation can read it.
    fn rename_failed_at(
        &mut self,
        prepared: &PreparedRename,
        failed_mem: &str,
        committed: Vec<(usize, String)>,
        cause: EngineError,
    ) -> EngineError {
        let failed_mount = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == failed_mem)
            .expect("the failed mem is one the rename writes");
        let in_storage =
            super::super::landing::with_a_decided_commit(committed, failed_mount, &cause);
        if in_storage.is_empty() || prepared.peers.is_empty() {
            return cause;
        }
        self.level_committed_mounts(&in_storage);
        let landed_in = |mount_idx: usize| in_storage.iter().any(|(idx, _)| *idx == mount_idx);
        let item = |id: &crate::entity::EntityId, landed: bool| PartialItem {
            id: id.to_string(),
            landed,
            current_hash: self.hash_now(id),
        };
        let error_for = |mem: &str| (mem == failed_mem).then(|| cause.to_string());
        let mut mems: Vec<PartialMem> = prepared
            .peers
            .values()
            .map(|plan| {
                let landed = landed_in(plan.mount_idx);
                PartialMem {
                    mem: plan.mem.clone(),
                    landed,
                    error: error_for(&plan.mem),
                    items: plan.rewrites.iter().map(|r| item(&r.id, landed)).collect(),
                }
            })
            .collect();
        // The renamed entity's own mem commits last. Its commit is in
        // storage only when it was decided and not applied; the entity
        // is then listed under its new id, otherwise under the id it
        // still has.
        let own_landed = landed_in(prepared.mount_idx);
        let renamed = if own_landed {
            &prepared.new_id
        } else {
            &prepared.id
        };
        mems.push(PartialMem {
            mem: prepared.mem.clone(),
            landed: own_landed,
            error: error_for(&prepared.mem),
            items: std::iter::once(item(renamed, own_landed))
                .chain(prepared.same_mem.iter().map(|r| item(&r.id, own_landed)))
                .collect(),
        });
        EngineError::RenamePartialFailure {
            committed_mems: in_storage
                .iter()
                .map(|(idx, _)| self.mounts[*idx].mount.mem.clone())
                .collect(),
            failed_mem: failed_mem.to_string(),
            failure_cause: super::super::landing::failure_cause_of(&cause).to_string(),
            mems,
        }
    }
}
