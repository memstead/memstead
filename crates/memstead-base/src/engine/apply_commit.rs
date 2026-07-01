//! Apply a [`CommitEnvelope`] to the in-memory store.
//!
//! Distinct from the engine's git-touching mutation paths
//! ([`crate::engine::mutation`]): `apply_external_commit` materializes
//! changes the engine did not author. The wire envelope arrives from
//! some external producer (the bridge today; future Node / Python
//! adapters) and replays the per-entity changes against the [`Store`]
//! without producing a new commit. Routes:
//!
//! - WASM clients receive envelopes over the bridge wire and call this
//!   method to keep their in-memory mirror in step with the server.
//! - Native test setups use it to seed a known graph state without
//!   running the full storage pipeline.
//! - A future replay tool reads a stream of envelopes back from disk
//!   and reconstructs the graph at a point in history.
//!
//! Per-change semantics:
//!
//! - `Added` / `Modified` → parse the new body, upsert into the store,
//!   re-emit explicit relationship edges. Parse failures abort the
//!   entire envelope (the post-state must be coherent — a partial
//!   apply would leave the store wedged between two SHAs).
//! - `Deleted` → drop the entity and cascade its edges.
//! - `Renamed` → drop the source entity, then parse + upsert the new
//!   body at the new id. We do *not* call [`Store::rename_node`]
//!   because the wire content is authoritative — the new body may
//!   carry a different relationship section than the old one did.
//!
//! Schema-validation is *soft*: a parse
//! failure refuses (the entity can't be loaded at all), but
//! schema-level warnings (dangling wiki-links, unknown rel-types in
//! the relationships section) do not refuse — the server is the
//! authority on schema, the client mirrors what arrives.
//!
//! HEAD cursor + subscriber notification: after a successful apply
//! the engine's cached `last_known_head` for the affected mount
//! advances to the envelope SHA, and one [`MemChangedEvent`] fires
//! to every subscriber. Same shape every other mem-advance event
//! flows through.

use crate::engine::{Engine, EngineError, MemChangedEvent};
use crate::entity::id::file_path_to_id;
use crate::entity::loader::parse_entries;
use crate::entity::source::SourceEntry;
use crate::entity::store_builder::push_entities_into_store;
use crate::ops::{CommitEnvelope, EntityChange};

impl Engine {
    /// Replay an externally-produced commit envelope into the
    /// in-memory store.
    ///
    /// Refuses with [`EngineError::UnknownMem`] when the envelope
    /// names a mem the engine has no mount for. Parse failures on
    /// any change variant surface as [`EngineError::Parse`] and abort
    /// the apply before any store mutation lands — the post-state is
    /// either coherent at the envelope SHA or unchanged. (The store
    /// mutation loop below uses a staged scratch list precisely to
    /// preserve this all-or-nothing property; do not refactor it
    /// into per-change in-place mutations without restoring an
    /// equivalent guarantee.)
    ///
    /// Empty `changes` is a valid envelope: the head cursor advances
    /// and a `MemChangedEvent` fires, but no store mutation
    /// happens. Lets replay drivers signal "we saw a commit, here is
    /// its SHA, no entity-level changes" — useful for empty commits
    /// (e.g. tag-only or merge commits without tree changes).
    pub fn apply_external_commit(
        &mut self,
        envelope: &CommitEnvelope,
    ) -> Result<(), EngineError> {
        let mem = envelope.mem.as_str();

        // 1. Resolve mount + schema. Unknown mem refuses before any
        //    parse work — same idempotent shape as `reload_one_mem`.
        let mount_idx = self
            .mounts
            .iter()
            .position(|m| m.mount.mem == mem)
            .ok_or_else(|| EngineError::UnknownMem(mem.to_string()))?;
        let schema = self
            .schemas
            .get(mem)
            .cloned()
            .ok_or_else(|| EngineError::UnknownMem(mem.to_string()))?;

        // 2. Walk the changes. Stage parse outputs so a parse failure
        //    on a late change rolls back the entire apply (the store
        //    remains untouched until the second loop below). Deletions
        //    and rename-source removals stage too — they're just paths.
        let mut to_upsert: Vec<SourceEntry> = Vec::new();
        let mut to_remove: Vec<String> = Vec::new();
        for change in &envelope.changes {
            match change {
                EntityChange::Added { path, content }
                | EntityChange::Modified { path, content } => {
                    to_upsert.push(SourceEntry {
                        relative_path: path.clone(),
                        source_path: std::path::PathBuf::from(path.clone()),
                        content: content.clone(),
                    });
                }
                EntityChange::Deleted { path } => {
                    to_remove.push(path.clone());
                }
                EntityChange::Renamed { from, to, content } => {
                    to_remove.push(from.clone());
                    to_upsert.push(SourceEntry {
                        relative_path: to.clone(),
                        source_path: std::path::PathBuf::from(to.clone()),
                        content: content.clone(),
                    });
                }
            }
        }

        // 3. Parse the staged adds/modifies/rename-targets against the
        //    mem schema. Parse errors abort before the store sees a
        //    mutation. `parse_entries` collects per-file errors into
        //    `LoadResult::errors`; on `apply_external_commit` semantics
        //    we treat any parse failure as a hard refuse — the client
        //    cannot half-apply a commit.
        let load_result = parse_entries(to_upsert, Vec::new(), mem, schema.as_ref());
        if let Some((path, msg)) = load_result.errors.into_iter().next() {
            return Err(EngineError::ParseAfterWrite(format!(
                "apply_external_commit: failed to parse '{}' in mem '{}': {}",
                path.display(),
                mem,
                msg
            )));
        }

        // 4. Mutate the store. Removals first so a rename's
        //    (from, to) pair where `to == from + suffix-rename`
        //    doesn't wipe the just-inserted new entity.
        for path in &to_remove {
            let id = file_path_to_id(path, mem);
            self.store.remove(&id);
        }
        // Attach file_path on each parsed entity to mirror the
        // boot/reload contract — handlers that surface
        // `Entity::file_path` to consumers (e.g. health, export) see
        // the same value the source pipeline would have produced.
        let mut parsed = load_result.entities;
        for result in parsed.iter_mut() {
            result.entity.file_path = result.entity.id.0.clone();
        }
        // Mutation-path call site → no `LoadCollector`; matches the
        // documented invariant in `push_entities_into_store` (load
        // sites emit drift warnings, mutation sites stay silent).
        // The fallback-schema parameter is underscored inside the
        // helper (no longer consulted for edge emission); the
        // engine-wide sentinel is the historical pick here.
        let fallback = crate::engine_fallback_type();
        push_entities_into_store(&mut self.store, parsed, fallback.as_ref(), None);

        // 5. Advance the cached head + invalidate memos so the next
        //    read sees the new state.
        let previous = self
            .mounts
            .get(mount_idx)
            .and_then(|m| m.last_known_head.clone())
            .unwrap_or_default();
        if let Some(state) = self.mounts.get_mut(mount_idx) {
            state.last_known_head = Some(envelope.sha.clone());
        }
        self.invalidate_communities();
        #[cfg(not(target_arch = "wasm32"))]
        self.invalidate_search_indexes();

        // 6. Emit one `MemChangedEvent` so subscribers see the
        //    transition. Same shape `record_self_write` and
        //    `branch_reset` use.
        if previous != envelope.sha {
            let event = MemChangedEvent {
                mem: mem.to_string(),
                head: envelope.sha.clone(),
                previous,
                n_commits: 1,
            };
            self.emit_mem_changed(&event);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MemBackend;
    use crate::engine::test_helpers::*;
    use crate::entity::EntityId;
    use crate::storage::FilesystemMemWriter;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    fn empty_folder_engine(tmp: &TempDir, mem: &str) -> Engine {
        let mem_dir = tmp.path().to_path_buf();
        let writer = FilesystemMemWriter::new(mem_dir.clone());
        Engine::from_mounts(vec![(
            folder_mount(mem, mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap()
    }

    fn envelope(
        mem: &str,
        sha: &str,
        parent: &str,
        changes: Vec<EntityChange>,
    ) -> CommitEnvelope {
        CommitEnvelope {
            sha: sha.to_string(),
            parent: parent.to_string(),
            mem: mem.to_string(),
            timestamp: "2026-05-19T10:00:00Z".to_string(),
            trailers: BTreeMap::new(),
            changes,
        }
    }

    fn well_formed_body(title: &str) -> String {
        format!(
            "---\ntype: spec\ncreated_date: 2026-01-01\nlast_modified: 2026-01-01\nlevel: M0\n---\n# {title}\n\n## Identity\n\n{title}\n"
        )
    }

    #[test]
    fn apply_external_commit_unknown_mem_refuses() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        let env = envelope("missing", "deadbeef", "", vec![]);
        let err = engine.apply_external_commit(&env).unwrap_err();
        assert_eq!(err.code(), "UNKNOWN_MEM");
    }

    #[test]
    fn apply_external_commit_added_lands_entity_in_store() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        let env = envelope(
            "specs",
            "abc123",
            "",
            vec![EntityChange::Added {
                path: "alpha.md".to_string(),
                content: well_formed_body("Alpha"),
            }],
        );
        engine.apply_external_commit(&env).unwrap();
        let id = EntityId::new("specs", "alpha");
        assert!(
            engine.get_entity(&id).is_some(),
            "expected 'specs--alpha' in store after Added apply"
        );
    }

    #[test]
    fn apply_external_commit_modified_replaces_existing_body() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        // First apply seeds the entity.
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha1",
                "",
                vec![EntityChange::Added {
                    path: "alpha.md".to_string(),
                    content: well_formed_body("Alpha-v0"),
                }],
            ))
            .unwrap();
        // Second apply mutates it.
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha2",
                "sha1",
                vec![EntityChange::Modified {
                    path: "alpha.md".to_string(),
                    content: well_formed_body("Alpha-v1"),
                }],
            ))
            .unwrap();
        let id = EntityId::new("specs", "alpha");
        let entity = engine.get_entity(&id).expect("alpha must still exist");
        assert_eq!(entity.title, "Alpha-v1");
    }

    #[test]
    fn apply_external_commit_deleted_removes_entity_from_store() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha1",
                "",
                vec![EntityChange::Added {
                    path: "alpha.md".to_string(),
                    content: well_formed_body("Alpha"),
                }],
            ))
            .unwrap();
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha2",
                "sha1",
                vec![EntityChange::Deleted {
                    path: "alpha.md".to_string(),
                }],
            ))
            .unwrap();
        let id = EntityId::new("specs", "alpha");
        assert!(
            engine.get_entity(&id).is_none(),
            "expected 'specs--alpha' removed after Deleted apply"
        );
    }

    #[test]
    fn apply_external_commit_renamed_moves_entity_to_new_id() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha1",
                "",
                vec![EntityChange::Added {
                    path: "alpha.md".to_string(),
                    content: well_formed_body("Alpha"),
                }],
            ))
            .unwrap();
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha2",
                "sha1",
                vec![EntityChange::Renamed {
                    from: "alpha.md".to_string(),
                    to: "alpha-renamed.md".to_string(),
                    content: well_formed_body("Alpha Renamed"),
                }],
            ))
            .unwrap();
        let old_id = EntityId::new("specs", "alpha");
        let new_id = EntityId::new("specs", "alpha-renamed");
        assert!(engine.get_entity(&old_id).is_none(), "old id must be gone");
        assert!(
            engine.get_entity(&new_id).is_some(),
            "new id must be present"
        );
    }

    #[test]
    fn apply_external_commit_mixed_changes_apply_atomically() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        // Seed the mem with two entities.
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha1",
                "",
                vec![
                    EntityChange::Added {
                        path: "alpha.md".to_string(),
                        content: well_formed_body("Alpha"),
                    },
                    EntityChange::Added {
                        path: "beta.md".to_string(),
                        content: well_formed_body("Beta"),
                    },
                ],
            ))
            .unwrap();
        // Mix: delete one, modify the other, add a third.
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha2",
                "sha1",
                vec![
                    EntityChange::Deleted {
                        path: "alpha.md".to_string(),
                    },
                    EntityChange::Modified {
                        path: "beta.md".to_string(),
                        content: well_formed_body("Beta-v2"),
                    },
                    EntityChange::Added {
                        path: "gamma.md".to_string(),
                        content: well_formed_body("Gamma"),
                    },
                ],
            ))
            .unwrap();
        let alpha = EntityId::new("specs", "alpha");
        let beta = EntityId::new("specs", "beta");
        let gamma = EntityId::new("specs", "gamma");
        assert!(engine.get_entity(&alpha).is_none());
        assert_eq!(engine.get_entity(&beta).unwrap().title, "Beta-v2");
        assert!(engine.get_entity(&gamma).is_some());
    }

    #[test]
    fn apply_external_commit_permissive_parser_accepts_minimal_body() {
        // The parse layer used by `apply_external_commit` is
        // intentionally permissive — `parse_markdown` does not refuse
        // missing frontmatter or missing titles (those `ParseError`
        // variants only fire under the strict validator). This is the
        // documented design: schema validation is soft — the server is
        // the authority, the client shows what it receives. Pin the
        // behavior so a future tightening here surfaces as an
        // intentional decision.
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        let bare_body = "# Bare\n\nNo frontmatter, no schema sections.\n";
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha1",
                "",
                vec![EntityChange::Added {
                    path: "bare.md".to_string(),
                    content: bare_body.to_string(),
                }],
            ))
            .unwrap();
        let id = EntityId::new("specs", "bare");
        let entity = engine
            .get_entity(&id)
            .expect("permissive parser must produce an entity even without frontmatter");
        assert_eq!(entity.title, "Bare");
    }

    #[test]
    fn apply_external_commit_emits_mem_changed_event_to_subscribers() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        let observed = Arc::new(Mutex::new(Vec::<MemChangedEvent>::new()));
        let observed_clone = observed.clone();
        let _handle = engine
            .subscribe_mem_changes(
                "specs",
                Arc::new(move |event| {
                    observed_clone.lock().unwrap().push(event.clone());
                }),
            )
            .unwrap();
        engine
            .apply_external_commit(&envelope(
                "specs",
                "sha-new",
                "",
                vec![EntityChange::Added {
                    path: "alpha.md".to_string(),
                    content: well_formed_body("Alpha"),
                }],
            ))
            .unwrap();
        let events = observed.lock().unwrap();
        assert_eq!(events.len(), 1, "expected one MemChangedEvent");
        assert_eq!(events[0].head, "sha-new");
        assert_eq!(events[0].mem, "specs");
        assert_eq!(events[0].n_commits, 1);
    }

    #[test]
    fn apply_external_commit_empty_changes_still_advances_head() {
        let tmp = TempDir::new().unwrap();
        let mut engine = empty_folder_engine(&tmp, "specs");
        let observed = Arc::new(Mutex::new(Vec::<MemChangedEvent>::new()));
        let observed_clone = observed.clone();
        let _handle = engine
            .subscribe_mem_changes(
                "specs",
                Arc::new(move |event| {
                    observed_clone.lock().unwrap().push(event.clone());
                }),
            )
            .unwrap();
        engine
            .apply_external_commit(&envelope("specs", "empty-commit-sha", "", vec![]))
            .unwrap();
        let events = observed.lock().unwrap();
        assert_eq!(
            events.len(),
            1,
            "empty envelope must still emit one event"
        );
        assert_eq!(events[0].head, "empty-commit-sha");
    }
}
