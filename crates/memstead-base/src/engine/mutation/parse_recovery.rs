//! `Engine::apply_parse_recovery` — bulk-fix path that consumes the
//! `ParsedRelationRecovery` payload on every writable-origin
//! `PARSED_RELATION_INVALID` warning and applies the recovery action
//! in a single operator-initiated call.
//!
//! The recovery action `remove_explicit_relation` means: drop the
//! parse-time-dropped row from the source entity's markdown. The
//! drop is already reflected in the in-memory `entity.relationships`
//! (the parse-time validator strips the bad row at boot / reload /
//! attach), so re-rendering and re-writing the source entity is
//! enough — the renderer emits `## Relationships` from
//! `entity.relationships`, so the stale row disappears.
//!
//! Multiple drops from the same source entity collapse to one
//! re-render: `entity.relationships` already excludes every dropped
//! row, so a single re-write fixes them all. The report still lists
//! one entry per warning so consumers see exactly which drops were
//! recovered.

use indexmap::IndexMap;

use crate::entity::EntityId;
use crate::ops::{ParseRecoveryEntry, ParseRecoveryReport, WarningHint};
use crate::vcs::{Actor, ClientId};

use super::super::{Engine, EngineError, UpdateEntityArgs};

impl Engine {
    /// Walk `load_warnings`, dispatch the `remove_explicit_relation`
    /// recovery for every writable-origin `PARSED_RELATION_INVALID`,
    /// and report each entry on the response. Read-only-origin
    /// warnings cannot be acted on (the engine has no write access
    /// to their source markdown) and surface as
    /// `outcome: "skipped"` with `reason: "readonly_mount"`.
    ///
    /// Failure model: per-entry failures land on the response as
    /// `outcome: "failed"` with the underlying engine error code in
    /// `reason`. The bulk-fix continues past per-entry failures so a
    /// single bad source doesn't strand the rest of the batch. Only
    /// engine-level errors (reload failure, broken workspace state)
    /// abort the call and propagate via `Err`.
    ///
    /// Idempotency: after the per-source re-renders land, the method
    /// runs `reload_each_writable_mem` so subsequent calls to
    /// `health` / `load_warnings` reflect the post-recovery state.
    /// Re-running on an already-clean workspace returns an empty
    /// `entries` list with no commits.
    pub fn apply_parse_recovery(
        &mut self,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<ParseRecoveryReport, EngineError> {
        // Snapshot every `PARSED_RELATION_INVALID` warning. We
        // iterate the snapshot, not `self.load_warnings`, so the
        // mid-loop `update_entity` calls (which do not touch
        // `load_warnings`) can't introduce ordering surprises.
        struct Drop {
            entity_id: EntityId,
            rel_type: String,
            target: EntityId,
            origin: String,
        }
        let drops: Vec<Drop> = self
            .load_warnings()
            .iter()
            .filter_map(|w| match w {
                WarningHint::ParsedRelationInvalid {
                    entity_id,
                    rel_type,
                    target,
                    origin,
                    ..
                } => Some(Drop {
                    entity_id: entity_id.clone(),
                    rel_type: rel_type.clone(),
                    target: target.clone(),
                    origin: origin.clone(),
                }),
                _ => None,
            })
            .collect();

        // Group writable drops by source-entity id so each source is
        // re-rendered at most once. Iteration over the IndexMap
        // preserves the order the warnings appeared in.
        let mut writable_by_source: IndexMap<EntityId, Vec<usize>> = IndexMap::new();
        let mut readonly_indices: Vec<usize> = Vec::new();
        for (idx, drop) in drops.iter().enumerate() {
            if drop.origin == "writable" {
                writable_by_source
                    .entry(drop.entity_id.clone())
                    .or_default()
                    .push(idx);
            } else {
                readonly_indices.push(idx);
            }
        }

        let mut entries: Vec<ParseRecoveryEntry> = Vec::with_capacity(drops.len());
        // Per-drop result slot — populated as the per-source attempts
        // land. Keyed by the snapshot index so the final `entries`
        // vec preserves the warning order.
        let mut result_per_drop: Vec<Option<(String, Option<String>)>> = vec![None; drops.len()];

        for idx in &readonly_indices {
            result_per_drop[*idx] = Some((
                ParseRecoveryEntry::OUTCOME_SKIPPED.to_string(),
                Some(ParseRecoveryEntry::REASON_READONLY_MOUNT.to_string()),
            ));
        }

        let mut last_commit_sha = String::new();
        for (source_id, drop_indices) in writable_by_source {
            let outcome = self.rewrite_for_parse_recovery(&source_id, actor, client, note);
            match outcome {
                Ok(commit_sha) => {
                    if !commit_sha.is_empty() {
                        last_commit_sha = commit_sha;
                    }
                    for idx in drop_indices {
                        result_per_drop[idx] =
                            Some((ParseRecoveryEntry::OUTCOME_REMOVED.to_string(), None));
                    }
                }
                Err(err) => {
                    let code = err.code().to_string();
                    for idx in drop_indices {
                        result_per_drop[idx] = Some((
                            ParseRecoveryEntry::OUTCOME_FAILED.to_string(),
                            Some(code.clone()),
                        ));
                    }
                }
            }
        }

        // Materialise entries in the original warning order.
        for (idx, drop) in drops.into_iter().enumerate() {
            let (outcome, reason) = result_per_drop[idx]
                .take()
                .expect("every drop should have been classified");
            entries.push(ParseRecoveryEntry {
                entity_id: drop.entity_id,
                rel_type: drop.rel_type,
                target: drop.target,
                outcome,
                reason,
            });
        }

        // Reload writable mems so `load_warnings` reflects the
        // post-recovery state — drops that re-rendered cleanly drop
        // out; drops that failed (or read-only ones that were always
        // out of scope) survive.
        if !entries.is_empty() {
            self.reload_each_writable_mem()?;
        }

        Ok(ParseRecoveryReport {
            entries,
            commit_sha: last_commit_sha,
        })
    }

    /// Re-render the source entity and write it back to disk. Calls
    /// `update_entity` seeding the first section's current body as a
    /// rewrite anchor — section content stays identical, but the
    /// full re-render flushes the parse-time relation drops out of
    /// the auto-managed `## Relationships` section. Returns the
    /// resulting `commit_sha` on success.
    ///
    /// Section-anchor seeding (instead of an empty payload) keeps
    /// this internal rewrite path on the public `update_entity`
    /// surface — the agent-boundary `EMPTY_UPDATE` guard refuses
    /// empty payloads, and re-using the same guard avoids splitting the engine's
    /// validation surface for one internal caller. Picking the
    /// first section is safe because real entities always carry at
    /// least one schema-required section (a stub would have tripped
    /// the `StubNotUpdatable` guard before reaching here).
    fn rewrite_for_parse_recovery(
        &mut self,
        source_id: &EntityId,
        actor: Actor,
        client: Option<&ClientId>,
        note: Option<&str>,
    ) -> Result<String, EngineError> {
        let entity = self
            .store()
            .get(source_id)
            .ok_or_else(|| EngineError::NotFound {
                id: source_id.to_string(),
            })?;
        let expected_hash = entity.content_hash.clone();
        let mut sections: IndexMap<String, String> = IndexMap::new();
        if let Some((key, body)) = entity.sections.iter().next() {
            sections.insert(key.clone(), body.clone());
        }
        let args = UpdateEntityArgs {
            anchors: Vec::new(),
            id: source_id.clone(),
            expected_hash: Some(expected_hash),
            sections,
            append_sections: IndexMap::new(),
            patch_sections: IndexMap::new(),
            metadata: IndexMap::new(),
            metadata_unset: Vec::new(),
            dry_run: false,
            declare_relations: Vec::new(),
            relations_unset: Vec::new(),
        };
        let outcome = self.update_entity(args, actor, client, note)?;
        Ok(outcome.commit_sha)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use crate::backend::MemBackend;
    use crate::engine::Engine;
    use crate::engine::test_helpers::{
        archive_mount, build_archive, cli_actor, folder_mount, write_schema_files_with_default_type,
    };
    use crate::ops::{ParseRecoveryEntry, WarningHint};
    use crate::storage::{ArchiveBackend, FilesystemMemWriter};
    use crate::workspace::{Mount, MountCapability, MountLifecycle, MountStorage};

    use memstead_schema::SchemaRef;

    /// Two writable parse-time drops on the same source collapse to
    /// one re-render. Both entries land on the response as
    /// `removed`; the on-disk markdown no longer carries the bad
    /// rows; `load_warnings` is empty after the call.
    #[test]
    fn apply_parse_recovery_clears_writable_drops_in_one_call() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let target = "---\ntype: spec\n---\n# Target\n\n## Identity\n\nTarget body.\n";
        let source = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nSource body.\n\n## Relationships\n\n- **MADE_UP_TYPE_A**: [[specs--target]]\n- **MADE_UP_TYPE_B**: [[specs--target]]\n";
        std::fs::write(mem_dir.join("target.md"), target).unwrap();
        std::fs::write(mem_dir.join("source.md"), source).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir.clone()),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let pre: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }))
            .collect();
        assert_eq!(pre.len(), 2, "expected two parse-time drops, got {pre:?}");

        let (actor, client) = cli_actor();
        let report = engine
            .apply_parse_recovery(actor, Some(&client), Some("recovery"))
            .expect("recovery succeeds");

        assert_eq!(report.entries.len(), 2);
        for entry in &report.entries {
            assert_eq!(
                entry.outcome,
                ParseRecoveryEntry::OUTCOME_REMOVED,
                "expected both writable drops removed, got {entry:?}",
            );
            assert!(entry.reason.is_none());
        }
        assert!(!report.commit_sha.is_empty(), "recovery must commit");

        let post: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }))
            .collect();
        assert!(post.is_empty(), "drops must be cleared, got {post:?}");

        let cleaned = std::fs::read_to_string(mem_dir.join("source.md")).unwrap();
        assert!(
            !cleaned.contains("MADE_UP_TYPE_A"),
            "cleaned source: {cleaned}"
        );
        assert!(
            !cleaned.contains("MADE_UP_TYPE_B"),
            "cleaned source: {cleaned}"
        );
    }

    /// Read-only-origin drops are reported as `skipped` with
    /// `reason: "readonly_mount"`. The engine cannot rewrite an
    /// archive, so the warning survives the call.
    #[test]
    fn apply_parse_recovery_skips_readonly_origin_drops() {
        let tmp = TempDir::new().unwrap();
        let target = "---\ntype: spec\n---\n# Target\n\n## Identity\n\nTarget.\n";
        let source = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nSource.\n\n## Relationships\n\n- **MADE_UP**: [[external--target]]\n";
        let archive_path = build_archive(
            tmp.path(),
            "ext",
            &[
                ("target.md", target.as_bytes()),
                ("source.md", source.as_bytes()),
            ],
        );

        let mut engine = Engine::from_mounts(vec![(
            archive_mount("external", archive_path.clone()),
            Box::new(ArchiveBackend::new(archive_path)),
        )])
        .unwrap();

        let (actor, client) = cli_actor();
        let report = engine
            .apply_parse_recovery(actor, Some(&client), None)
            .expect("recovery succeeds");

        assert_eq!(report.entries.len(), 1);
        let entry = &report.entries[0];
        assert_eq!(entry.outcome, ParseRecoveryEntry::OUTCOME_SKIPPED);
        assert_eq!(
            entry.reason.as_deref(),
            Some(ParseRecoveryEntry::REASON_READONLY_MOUNT),
        );
        assert!(
            report.commit_sha.is_empty(),
            "readonly path commits nothing"
        );

        let post: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }))
            .collect();
        assert_eq!(post.len(), 1, "readonly drop must persist, got {post:?}");
    }

    /// Re-running on an already-clean workspace returns an empty
    /// entries list and produces no commits.
    #[test]
    fn apply_parse_recovery_is_idempotent_after_clean_state() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().to_path_buf();
        let target = "---\ntype: spec\n---\n# Target\n\n## Identity\n\nTarget.\n";
        let source = "---\ntype: spec\n---\n# Source\n\n## Identity\n\nSource.\n\n## Relationships\n\n- **MADE_UP_TYPE**: [[specs--target]]\n";
        std::fs::write(mem_dir.join("target.md"), target).unwrap();
        std::fs::write(mem_dir.join("source.md"), source).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let mut engine = Engine::from_mounts(vec![(
            folder_mount("specs", mem_dir),
            Box::new(writer) as Box<dyn MemBackend>,
        )])
        .unwrap();

        let (actor, client) = cli_actor();
        let first = engine
            .apply_parse_recovery(actor, Some(&client), None)
            .expect("first recovery succeeds");
        assert_eq!(first.entries.len(), 1);
        assert_eq!(
            first.entries[0].outcome,
            ParseRecoveryEntry::OUTCOME_REMOVED
        );
        assert!(!first.commit_sha.is_empty());

        let second = engine
            .apply_parse_recovery(actor, Some(&client), None)
            .expect("second recovery succeeds");
        assert!(
            second.entries.is_empty(),
            "second call must be no-op, got {:?}",
            second.entries
        );
        assert!(second.commit_sha.is_empty());
    }

    /// Mixed writable + readonly drops land in a single report.
    #[test]
    fn apply_parse_recovery_reports_per_warning_across_origins() {
        let tmp = TempDir::new().unwrap();

        let writable_dir = tmp.path().join("writable");
        std::fs::create_dir_all(&writable_dir).unwrap();
        let w_target = "---\ntype: spec\n---\n# WT\n\n## Identity\n\nwt\n";
        let w_source = "---\ntype: spec\n---\n# WS\n\n## Identity\n\nws\n\n## Relationships\n\n- **MADE_UP_A**: [[specs--target]]\n- **MADE_UP_B**: [[specs--target]]\n";
        std::fs::write(writable_dir.join("target.md"), w_target).unwrap();
        std::fs::write(writable_dir.join("source.md"), w_source).unwrap();

        let r_target = "---\ntype: spec\n---\n# RT\n\n## Identity\n\nrt\n";
        let r_source = "---\ntype: spec\n---\n# RS\n\n## Identity\n\nrs\n\n## Relationships\n\n- **MADE_UP_RO**: [[external--target]]\n";
        let archive_path = build_archive(
            tmp.path(),
            "ext",
            &[
                ("target.md", r_target.as_bytes()),
                ("source.md", r_source.as_bytes()),
            ],
        );

        let writer = FilesystemMemWriter::new(writable_dir.clone());
        let mut engine = Engine::from_mounts(vec![
            (
                folder_mount("specs", writable_dir),
                Box::new(writer) as Box<dyn MemBackend>,
            ),
            (
                archive_mount("external", archive_path.clone()),
                Box::new(ArchiveBackend::new(archive_path)),
            ),
        ])
        .unwrap();

        let (actor, client) = cli_actor();
        let report = engine
            .apply_parse_recovery(actor, Some(&client), None)
            .expect("recovery succeeds");

        assert_eq!(report.entries.len(), 3);
        let removed: Vec<_> = report
            .entries
            .iter()
            .filter(|e| e.outcome == ParseRecoveryEntry::OUTCOME_REMOVED)
            .collect();
        let skipped: Vec<_> = report
            .entries
            .iter()
            .filter(|e| e.outcome == ParseRecoveryEntry::OUTCOME_SKIPPED)
            .collect();
        assert_eq!(removed.len(), 2);
        assert_eq!(skipped.len(), 1);
        assert_eq!(
            skipped[0].reason.as_deref(),
            Some(ParseRecoveryEntry::REASON_READONLY_MOUNT),
        );
        assert!(!report.commit_sha.is_empty());
    }

    /// A drop whose source still has an unresolved body wiki-link to
    /// the dropped target lands as `failed` with
    /// `WIKILINK_WITHOUT_RELATION`. The strict validator refuses to
    /// leave a body wiki-link unbacked by any relation; the
    /// operator's recovery is to also remove the body reference.
    #[test]
    fn apply_parse_recovery_reports_failed_for_unbacked_body_link() {
        let tmp = TempDir::new().unwrap();
        let schemas_dir = tmp.path().join("schemas");
        std::fs::create_dir_all(&schemas_dir).unwrap();
        let manifest = r#"name: link-test
version: 0.1.0
description: schema for wikilink-blocker test
when_to_use: tests
types:
  - doc
relationships:
  mode: strict
  definitions:
    - name: MENTIONS
      description: doc references doc
      default_weight: 1.0
    - name: _default
      description: fallback
      default_weight: 1.0
community:
  resolution: 1.0
  seed: 42
"#;
        write_schema_files_with_default_type(&schemas_dir, "link-test", manifest, &["doc"]);

        let mem_dir = tmp.path().join("mem");
        std::fs::create_dir_all(&mem_dir).unwrap();
        let target = "---\ntype: doc\n---\n# Target\n\n## Body\n\nbody\n";
        let source = "---\ntype: doc\n---\n# Source\n\n## Body\n\nrefer to [[specs--target]] here\n\n## Relationships\n\n- **BADTYPE**: [[specs--target]]\n";
        std::fs::write(mem_dir.join("target.md"), target).unwrap();
        std::fs::write(mem_dir.join("source.md"), source).unwrap();

        let writer = FilesystemMemWriter::new(mem_dir.clone());
        let pin = SchemaRef::new("link-test", semver::Version::new(0, 1, 0));
        let mount = Mount {
            mem: "specs".to_string(),
            schema: Some(pin),
            storage: MountStorage::Folder {
                path: mem_dir.clone(),
            },
            capability: MountCapability::Write,
            lifecycle: MountLifecycle::Eager,
            cross_linkable: true,
            migration_target: None,
        };
        let mut engine = Engine::from_mounts_with_schemas_dir(
            vec![(mount, Box::new(writer) as Box<dyn MemBackend>)],
            Some(&schemas_dir),
        )
        .unwrap();

        let (actor, client) = cli_actor();
        let report = engine
            .apply_parse_recovery(actor, Some(&client), None)
            .expect("recovery returns Ok even when entries fail");

        assert_eq!(report.entries.len(), 1);
        let entry = &report.entries[0];
        assert_eq!(entry.outcome, ParseRecoveryEntry::OUTCOME_FAILED);
        assert_eq!(
            entry.reason.as_deref(),
            Some("WIKILINK_WITHOUT_RELATION"),
            "expected the strict validator's typed code, got {:?}",
            entry.reason,
        );
        let unchanged = std::fs::read_to_string(mem_dir.join("source.md")).unwrap();
        assert!(
            unchanged.contains("BADTYPE"),
            "source must be unchanged on failure"
        );

        let post: Vec<_> = engine
            .load_warnings()
            .iter()
            .filter(|w| matches!(w, WarningHint::ParsedRelationInvalid { .. }))
            .collect();
        assert_eq!(post.len(), 1, "failed drop must persist, got {post:?}");
    }
}
