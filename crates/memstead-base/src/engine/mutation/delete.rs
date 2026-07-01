//! `Engine::delete_entity` — remove an entity from a mount's backend
//! and the in-memory store.

use std::path::Path;

use crate::entity::EntityId;
use crate::ops::WarningHint;
use crate::provenance::{Provenance, ProvenanceKind};
use crate::vcs::{Actor, ClientId, CommitContext};
use crate::workspace::MountCapability;

use super::super::{
    DeleteEntityArgs, DeleteEntityOutcome, Engine, EngineError, ReferrerInfo,
};
use super::{gc_orphan_stubs, make_stub};

/// The would-be delete outcome for an entity, derived purely from the
/// in-memory graph (no mutation). `write_referrers` are the Write-Mem
/// sources that block a delete (`HAS_INCOMING_REFS`); `readonly_referrers`
/// are ReadOnly-mount sources that instead trigger the residual-stub
/// demotion. Both empty ⇒ a clean removal. Shared by [`Engine::delete_entity`]
/// (the real guard) and the CLI `delete --dry-run` preview so the preview's
/// verdict cannot drift from the real outcome.
#[derive(Debug, Clone)]
pub struct DeleteReferrers {
    pub write_referrers: Vec<ReferrerInfo>,
    pub readonly_referrers: Vec<EntityId>,
}

impl DeleteReferrers {
    /// Whether the real delete would refuse with `HAS_INCOMING_REFS`.
    pub fn would_refuse(&self) -> bool {
        !self.write_referrers.is_empty()
    }
}

impl Engine {

    /// Classify an entity's incoming referrers by the source mount's
    /// capability — the read-only core of the delete guard. Write-Mem
    /// referrers block the delete; ReadOnly referrers trigger the
    /// residual-stub demotion. Per-source dedup collapses an N-edge
    /// source into one [`ReferrerInfo`] carrying every rel-type. A
    /// referrer in an unmounted mem is treated as Write
    /// (safe-by-default: refuse rather than silently demote).
    ///
    /// Pure read — no disk, commit, lock, or store mutation. Used by
    /// [`Self::delete_entity`] and the CLI `delete --dry-run` preview so
    /// both compute the same verdict from one implementation.
    pub fn classify_delete_referrers(&self, id: &EntityId) -> DeleteReferrers {
        let mut write_referrers: Vec<ReferrerInfo> = Vec::new();
        let mut readonly_referrers: Vec<EntityId> = Vec::new();
        for edge in self.store.incoming(id) {
            let from_mem = edge.from.mem().to_string();
            let cap = self
                .mounts
                .iter()
                .find(|m| m.mount.mem == from_mem)
                .map(|m| m.mount.capability)
                .unwrap_or(MountCapability::Write);
            match cap {
                MountCapability::Write => {
                    let from_id = edge.from.to_string();
                    if let Some(existing) =
                        write_referrers.iter_mut().find(|r| r.from_id == from_id)
                    {
                        if !existing.rel_types.contains(&edge.rel_type) {
                            existing.rel_types.push(edge.rel_type.clone());
                        }
                    } else {
                        write_referrers.push(ReferrerInfo {
                            from_id,
                            rel_types: vec![edge.rel_type.clone()],
                            mem: from_mem,
                        });
                    }
                }
                MountCapability::ReadOnly => readonly_referrers.push(edge.from.clone()),
            }
        }
        DeleteReferrers {
            write_referrers,
            readonly_referrers,
        }
    }

    /// Delete an entity from its mount.
    ///
    /// Binary semantics — there is no force flag. The engine partitions
    /// incoming references by the source mem's
    /// [`MountCapability`]:
    ///
    /// - any **Write-Mem** referrers → refuse with typed
    ///   [`EngineError::HasIncomingRefs`] carrying the structured
    ///   referrer list;
    /// - only **ReadOnly-mount** referrers → delete the file +
    ///   commit, then demote the in-memory entity to a stub at the
    ///   same id so the surviving incoming edges keep a valid target.
    ///   A `RESIDUAL_STUB_FOR_READONLY_REFERRERS` warning rides on
    ///   the outcome;
    /// - no referrers → clean removal (file + store entry + cascading
    ///   edges). Orphaned stubs whose last incoming edge was the
    ///   deleted entity are GC'd.
    ///
    /// Optimistic-locking via `args.expected_hash` matches
    /// `update_entity`. Stubs (no on-disk file) skip the backend
    /// write and commit but still log provenance.
    pub fn delete_entity(
        &mut self,
        args: DeleteEntityArgs,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<DeleteEntityOutcome, EngineError> {
        let id = &args.id;
        let mem = id.mem().to_string();

        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| EngineError::UnknownMem(mem.clone()))?;
        if self.mounts[mount_idx].mount.capability != MountCapability::Write {
            return Err(EngineError::ReadOnlyMount(mem));
        }

        // Reload-before-operation: reload if a sibling advanced the
        // mem ref, so the `expected_hash` compare and the
        // referrer classification below see current truth. Notice
        // rides the outcome's `warnings`.
        let mut drift_warnings = self.reload_if_stale(Some(&mem));

        let entity = self
            .store
            .get(id)
            .ok_or_else(|| EngineError::NotFound { id: id.to_string() })?;

        if let Some(expected) = args.expected_hash.as_deref()
            && entity.content_hash != expected
        {
            // Stubs have no `content_hash` to compare against, so the
            // pre-fix `(current: )` paren misdirected callers toward
            // hash-recovery via `memstead_entity` (which returns the same
            // empty hash). Surface `details.is_stub: true` and a
            // corrective-action message — the actual fix is
            // `expected_hash: ""`. Non-stub mismatches keep the prior
            // contract.
            return Err(EngineError::HashMismatch {
                id: id.to_string(),
                current: entity.content_hash.clone(),
                is_stub: entity.stub,
            });
        }

        let file_path = entity.file_path.clone();
        let entity_is_stub = entity.stub;

        // Partition incoming refs by the source mem's mount capability
        // (Write-Mem referrers block; ReadOnly trigger the residual-stub
        // demotion). The classification is shared with the CLI
        // `delete --dry-run` preview so the two cannot disagree about the
        // outcome.
        let DeleteReferrers {
            write_referrers,
            readonly_referrers,
        } = self.classify_delete_referrers(id);
        if !write_referrers.is_empty() {
            return Err(EngineError::HasIncomingRefs {
                id: id.to_string(),
                referrers: write_referrers,
            });
        }

        let demote_to_stub = !readonly_referrers.is_empty();
        let removed_incoming: Vec<String> = readonly_referrers
            .iter()
            .map(|id| id.to_string())
            .collect();

        // Pre-delete edge count so the response carries the correct
        // value regardless of whether we drop the entity or demote it
        // to a stub. On the demote path we only retain incoming edges
        // (outgoing are severed because the source entity is gone),
        // so the cascade still removes `outgoing` worth of edges.
        let relations_removed = if demote_to_stub {
            self.store.outgoing(id).len()
        } else {
            self.store.outgoing(id).len() + self.store.incoming(id).len()
        };

        // Stubs have no on-disk file (empty file_path) and were never
        // committed as their own grain — the relate that materialised
        // them committed the source entity's markdown, the stub itself
        // is in-memory + edge-index only. Routing a stub delete through
        // `backend.delete_entity` trips `MemWriter(Path("mem-
        // relative path is empty"))`. Skip the backend write + commit
        // for stubs; provenance still records the explicit drop so
        // memstead_changes_since consumers see the event.
        let backend = self.mounts[mount_idx].backend.as_ref();
        let commit_sha = if entity_is_stub {
            String::new()
        } else {
            backend.delete_entity(Path::new(&file_path))?;
            let commit_subject = format!("memstead: delete {id}");
            let ctx = CommitContext {
                actor,
                client: client.cloned(),
                tool: Some("delete_entity"),
                note: note.map(String::from),
                logical_operation_id: None,
                entity_ids: None,
            };
            backend.commit(&commit_subject, &ctx)?
        };

        backend.append_provenance(&Provenance::new(
            std::time::SystemTime::now(),
            ProvenanceKind::Delete,
            Some(id.to_string()),
            actor,
            client.cloned(),
            note.map(String::from),
        ))?;

        if !commit_sha.is_empty() {
            self.record_self_write(mount_idx, &commit_sha);
        }

        let mut warnings: Vec<WarningHint> = Vec::new();
        // Reload-before-operation drift notice, surfaced first.
        warnings.append(&mut drift_warnings);
        let orphan_stubs_removed = if demote_to_stub {
            // Demote: drop outgoing edges (the source is gone), then
            // upsert a stub at the same id so the surviving incoming
            // edges from ReadOnly mounts retain a valid target. This
            // mirrors the state a fresh boot would produce — the
            // parser would re-emit a `LoadTime` stub at this id from
            // the surviving ReadOnly wiki-links.
            self.store.remove_edges_from(id);
            self.store.upsert(
                id.clone(),
                make_stub(
                    id,
                    crate::entity::StubKind::Residual {
                        since_commit: commit_sha.clone(),
                        readonly_referrers: readonly_referrers.clone(),
                    },
                ),
            );
            warnings.push(WarningHint::ResidualStubForReadOnlyReferrers {
                id: id.clone(),
                referrers: readonly_referrers,
            });
            // Don't GC orphan stubs on the demote path — the entity we
            // just demoted is itself a stub now and would be flagged
            // as orphan if we count its surviving incoming edges
            // wrong. Its incoming edges from ReadOnly are real and
            // keep it alive; siblings are unaffected.
            Vec::new()
        } else {
            self.store.remove(id);
            // Sweep stubs whose last referrer was the deleted entity.
            // Common case: the deleted entity had a relate edge to a
            // stub target and was the only one holding it in-graph.
            gc_orphan_stubs(&mut self.store)
        };

        self.invalidate_communities();
        self.invalidate_search_indexes();

        // `require_notes` provenance nudge — single engine-level
        // enforcement point. Gated on a landed commit: a stub delete
        // skips the backend write (empty `commit_sha`, nothing to
        // attribute), so it doesn't demand a note.
        if !commit_sha.is_empty()
            && let Some(w) = self.note_missing_warning("delete_entity", note)
        {
            warnings.push(w);
        }

        Ok(DeleteEntityOutcome {
            id: id.clone(),
            file_path,
            removed_incoming,
            relations_removed,
            commit_sha,
            orphan_stubs_removed,
            warnings,
        })
    }

    /// Positional + CommitContext wrapper around
    /// [`Self::delete_entity`]. Bundles `id` + `expected_hash` into
    /// a [`DeleteEntityArgs`].
    pub fn delete_entity_with_ctx(
        &mut self,
        id: &EntityId,
        expected_hash: &str,
        ctx: &CommitContext<'_>,
    ) -> Result<DeleteEntityOutcome, EngineError> {
        let args = DeleteEntityArgs {
            id: id.clone(),
            expected_hash: Some(expected_hash.to_string()),
        };
        self.delete_entity(args, ctx.actor, ctx.client.as_ref(), ctx.note.as_deref())
    }
}

#[cfg(test)]
mod tests {
    

    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::engine::{DeleteEntityArgs, Engine, EngineError, RelateEntityArgs};
    use crate::ops::WarningHint;
    use crate::storage::FilesystemMemWriter;

    #[test]
    fn delete_entity_removes_file_and_store_entry() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Doomed");
        let (actor, client) = cli_actor();
        let outcome = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some(seeded.content_hash.clone()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert_eq!(outcome.id, seeded.id);
        assert_eq!(outcome.removed_incoming, Vec::<String>::new());
        // Store no longer carries the entity.
        assert!(engine.get_entity(&seeded.id).is_none());
        // On-disk file gone.
        assert!(!tmp.path().join(&seeded.file_path).exists());
        // Provenance log records the delete.
        let log = std::fs::read_to_string(tmp.path().join(".memstead/changes.jsonl")).unwrap();
        assert!(log.contains("\"kind\":\"delete\""));
    }

    #[test]
    fn delete_entity_rejects_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, seeded) = engine_with_seed(&tmp, "Locked");
        let (actor, client) = cli_actor();
        let err = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: seeded.id.clone(),
                    expected_hash: Some("nope".to_string()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::HashMismatch {
                id,
                current,
                is_stub,
            } => {
                assert_eq!(id, seeded.id.to_string());
                assert_eq!(current, seeded.content_hash);
                assert!(!is_stub, "real entity must not flag as stub");
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    /// Item 04 — a stub delete with a non-empty `expected_hash` used to
    /// trip the hash-mismatch path with `(current: )` empty paren,
    /// misdirecting the agent toward `memstead_entity`-based hash recovery.
    /// The recovery is `expected_hash: ""` — stubs have no content
    /// hash. The typed envelope now surfaces `is_stub: true` and the
    /// message names the corrective action directly.
    #[test]
    fn delete_entity_on_stub_with_non_empty_hash_surfaces_is_stub_flag() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, _seed) = engine_with_seed(&tmp, "Anchor");
        let (actor, client) = cli_actor();
        // Materialise a stub via the forward-reference relate path: the
        // stub keeps its incoming USES edge from the source, but
        // `delete_entity` checks `expected_hash` BEFORE it partitions
        // incoming refs, so the bogus-hash mismatch below fires
        // regardless of the referrer. (The former body-wiki-link-drop
        // trick no longer leaves an orphan to delete — the update path
        // now runs the orphan-stub GC sweep alongside relate-remove and
        // delete.)
        let stub_id = crate::EntityId::new("specs", "stub-target");
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source With Link"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(
            engine.store().contains(&stub_id),
            "forward-reference relate must materialise stub"
        );

        // Stub delete with a non-empty (bogus) expected_hash —
        // pre-fix the message read `current is ` with an empty trailing
        // value; now `is_stub: true` and the message names the fix.
        let err = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: stub_id.clone(),
                    expected_hash: Some("definitely-non-empty".to_string()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::HashMismatch {
                id,
                current,
                is_stub,
            } => {
                assert_eq!(id, stub_id.to_string());
                assert!(current.is_empty(), "stub has no content hash");
                assert!(is_stub, "is_stub must be true on a stub mismatch");
                let msg = format!(
                    "{}",
                    EngineError::HashMismatch {
                        id,
                        current,
                        is_stub
                    }
                );
                assert!(
                    msg.contains("stub") && msg.contains("expected_hash: \"\""),
                    "stub message must name the corrective action; got: {msg}",
                );
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }

    }

    /// `memstead_delete id=<stub> expected_hash=""` end-to-end. The pre-fix
    /// path tripped `BackendError::MemWriter(Path("mem-relative
    /// path is empty"))` because stubs carry an empty `file_path` and
    /// the backend's `delete_entity` rejected the empty path. The fix
    /// routes stub deletes around the backend write — stubs are
    /// in-memory + edge-index only, never committed as their own grain.
    #[test]
    fn delete_entity_on_stub_with_empty_hash_succeeds_via_in_memory_route() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, _anchor) = engine_with_seed(&tmp, "Anchor");
        let (actor, client) = cli_actor();

        // Inject an orphan stub (zero incoming edges) directly into the
        // store. Organic mutation paths can no longer leave one — the
        // relate-remove, delete, and update-via-alias-resync sweeps all
        // GC orphans the moment a stub's last referrer drops — so a
        // white-box insertion is the only way to set up the "delete a
        // pre-existing orphan stub" case this test exercises (e.g. a
        // legacy in-memory artifact).
        let stub_id = crate::EntityId::new("specs", "ghost-target");
        engine.store.upsert(
            stub_id.clone(),
            super::make_stub(&stub_id, crate::entity::StubKind::ForwardReference),
        );
        assert!(
            engine.store().contains(&stub_id),
            "injected orphan stub must be in store"
        );

        let outcome = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: stub_id.clone(),
                    expected_hash: Some(String::new()),
                },
                actor,
                Some(&client),
                None,
            )
            .expect("stub delete with expected_hash=\"\" must succeed");

        assert_eq!(outcome.id, stub_id);
        // Backend write was skipped — no commit grain for a stub.
        assert!(
            outcome.commit_sha.is_empty(),
            "stub deletes skip the backend write; commit_sha is empty"
        );
        // In-memory store no longer carries the stub.
        assert!(
            !engine.store().contains(&stub_id),
            "stub must be gone from the in-memory store"
        );
        // Subsequent lookups behave like a normal not-found.
        assert!(engine.get_entity(&stub_id).is_none());
        // Provenance log records the delete (the changes feed must see
        // explicit stub drops just like real deletes).
        let log = std::fs::read_to_string(tmp.path().join(".memstead/changes.jsonl")).unwrap();
        assert!(log.contains("\"kind\":\"delete\""));
    }

    #[test]
    fn delete_entity_refuses_on_write_mem_referrers_with_typed_payload() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, target) = engine_with_seed(&tmp, "Target");
        let (actor, client) = cli_actor();
        // Create a second entity that points at `target`.
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Delete refuses — the engine has no force flag; the agent
        // removes the offending references first.
        let err = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: target.id.clone(),
                    expected_hash: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap_err();
        match err {
            EngineError::HasIncomingRefs { id, referrers } => {
                assert_eq!(id, target.id.to_string());
                assert_eq!(referrers.len(), 1);
                let r = &referrers[0];
                assert_eq!(r.from_id, source.id.to_string());
                assert_eq!(r.rel_types, vec!["USES".to_string()]);
                assert_eq!(r.mem, "specs");
            }
            other => panic!("expected HasIncomingRefs, got {other:?}"),
        }

        // The entity is still in the store and the file still on disk —
        // no partial state from a refused delete.
        assert!(engine.get_entity(&target.id).is_some());
        assert!(tmp.path().join(&target.file_path).exists());
    }

    #[test]
    fn delete_entity_returns_commit_sha_and_relations_removed() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, target) = engine_with_seed(&tmp, "Target");
        let (actor, client) = cli_actor();

        // Build a small graph: source --USES--> target. Delete `source`
        // (no incoming refs on it) and observe relations_removed
        // counts the one outgoing edge.
        let source = engine
            .create_entity(
                empty_create_args("specs", "Source"),
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        let related = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: target.id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        let outcome = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(related.content_hash.clone()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // Real write — folder backend produces a synthetic CommitId.
        assert!(
            !outcome.commit_sha.is_empty(),
            "commit_sha must be populated on a real delete"
        );
        // Zero incoming + one outgoing edge removed.
        assert_eq!(outcome.relations_removed, 1);
        // No stubs in this graph; orphan_stubs_removed is empty.
        assert!(outcome.orphan_stubs_removed.is_empty());
        // No residual-stub warning — pure clean removal.
        assert!(outcome.warnings.is_empty());
    }

    #[test]
    fn delete_entity_garbage_collects_orphaned_stubs() {
        let tmp = TempDir::new().unwrap();
        let (mut engine, source) = engine_with_seed(&tmp, "Source");
        let (actor, client) = cli_actor();
        let stub_id = crate::EntityId::new("specs", "ghost-stub");

        // Relate source → stub_id; engine creates the stub since the
        // target was absent.
        let related = engine
            .relate_entity(
                RelateEntityArgs {
                    source: source.id.clone(),
                    expected_hash: Some(source.content_hash.clone()),
                    rel_type: "USES".to_string(),
                    target: stub_id.clone(),
                    remove: false,
                    description: None,
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();
        assert!(engine.store().contains(&stub_id), "stub must be in store");

        // Delete `source` — its outgoing edge to `stub_id` was the
        // stub's only incoming edge, so the GC sweep drops the stub.
        let outcome = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: source.id.clone(),
                    expected_hash: Some(related.content_hash.clone()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        assert_eq!(outcome.orphan_stubs_removed, vec![stub_id.clone()]);
        assert!(
            !engine.store().contains(&stub_id),
            "GC must drop the orphaned stub"
        );
    }

    /// ReadOnly-only referrer path: the entity has no Write-Mem
    /// referrers but is referenced from a ReadOnly archive. Delete
    /// removes the file and demotes the entity in-memory to a stub at
    /// the same id, preserving the incoming edges from the archive
    /// and surfacing a `RESIDUAL_STUB_FOR_READONLY_REFERRERS` warning.
    /// The post-mutation in-memory state matches what a fresh boot
    /// would produce: the parser would re-emit a stub at this id from
    /// the surviving wiki-link in the archive's markdown.
    #[test]
    fn delete_entity_demotes_to_stub_when_only_readonly_referrers_remain() {
        use crate::engine::test_helpers::{archive_mount, build_archive};
        use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

        let tmp = TempDir::new().unwrap();
        let writable_dir = tmp.path().join("writable");
        std::fs::create_dir_all(&writable_dir).unwrap();
        let writer = FilesystemMemWriter::new(writable_dir.clone());

        // Build an archive that declares an explicit cross-mem
        // relation into the writable mem. Under the alias model
        // edges originate from `## Relationships` only — the body
        // wiki-link aliases the declared relation.
        let archive_md = "---\ntype: spec\ncreated_date: 2026-01-15\nlast_modified: 2026-04-12\nlevel: M0\n---\n# Archived Source\n\n## Identity\n\nLinks to [[specs:target]].\n\n## Purpose\n\nFixture for residual-stub demotion.\n\n## Relationships\n\n- **REFERENCES**: [[specs:target]]\n";
        let archive_path = build_archive(
            tmp.path(),
            "archive",
            &[("archived-source.md", archive_md.as_bytes())],
        );

        let folder_mount = Mount {
            mem: "specs".to_string(),
            schema: Some(crate::engine::test_helpers::pin("default")),
            storage: MountStorage::Folder { path: writable_dir.clone() },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let archive_reader = crate::storage::ArchiveBackend::new(archive_path.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount,
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("archive", archive_path.clone()),
                Box::new(archive_reader) as Box<dyn MemBackend>,
            ),
        ])
        .unwrap();

        let (actor, client) = cli_actor();
        let target = engine
            .create_entity(empty_create_args("specs", "Target"), actor, Some(&client), None)
            .unwrap();

        // Sanity: the archive's wiki-link surfaces as an incoming
        // edge on `specs--target`.
        let archived_source_id = crate::EntityId::new("archive", "archived-source");
        assert!(
            engine.store().contains(&archived_source_id),
            "archive entity must load into the store"
        );
        let incoming_pre: Vec<_> = engine
            .store()
            .incoming(&target.id)
            .iter()
            .map(|e| e.from.clone())
            .collect();
        assert!(
            incoming_pre.contains(&archived_source_id),
            "archive wiki-link must produce an incoming edge on target; got {incoming_pre:?}"
        );

        // Delete: only-ReadOnly referrer → file removed, entity
        // demoted to a stub, warning surfaces, incoming edge survives.
        let outcome = engine
            .delete_entity(
                DeleteEntityArgs {
                    id: target.id.clone(),
                    expected_hash: Some(target.content_hash.clone()),
                },
                actor,
                Some(&client),
                None,
            )
            .unwrap();

        // File is gone from the writable mem.
        assert!(!writable_dir.join(&target.file_path).exists());
        // Entity in the store is now a stub at the same id.
        let demoted = engine.get_entity(&target.id).expect("residual stub must remain in store");
        assert!(demoted.stub, "demoted entity must be flagged as stub");
        assert!(demoted.entity_type.is_empty());
        // Typed provenance: the residual stub records its origin so
        // an agent reading via `memstead_entity` later sees the diagnostic
        // context the mutation-time warning carried.
        match &demoted.stub_kind {
            Some(crate::entity::StubKind::Residual {
                since_commit: _,
                readonly_referrers,
            }) => {
                assert_eq!(
                    readonly_referrers,
                    &vec![archived_source_id.clone()],
                    "Residual.readonly_referrers must snapshot the surviving referrers at mutation time"
                );
            }
            other => panic!(
                "demoted stub must be tagged Residual; got {other:?}"
            ),
        }
        // Incoming edge from archive survives.
        let incoming_post: Vec<_> = engine
            .store()
            .incoming(&target.id)
            .iter()
            .map(|e| e.from.clone())
            .collect();
        assert!(
            incoming_post.contains(&archived_source_id),
            "archive incoming edge must survive demotion"
        );
        // Warning carries the surviving referrer.
        let referrers = outcome
            .warnings
            .iter()
            .find_map(|w| match w {
                WarningHint::ResidualStubForReadOnlyReferrers { referrers, .. } => {
                    Some(referrers.clone())
                }
                _ => None,
            })
            .expect("ResidualStubForReadOnlyReferrers warning must surface");
        assert_eq!(referrers, vec![archived_source_id]);
        assert_eq!(outcome.removed_incoming.len(), 1);
    }

    // ---- Engine::relate_entity --------------------------------------
}
