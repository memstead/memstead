//! Mem-rename content sweep — the referrer-rewrite half of
//! `memstead mem rename`.
//!
//! Entity ids are derived from `(mount name, file path)`, so renaming a
//! mount re-ids every entity in it for free; what the rename must
//! rewrite by hand is every **textual** `<old>--<slug>` / `<old>:<slug>`
//! reference across the workspace — cross-mem wiki-links and
//! `## Relationships` entries in peer mems, full-id self-references in
//! the renamed mem itself, and the renamed mem's anchors-sidecar keys.
//!
//! The sweep is a raw-text pass over each entity file
//! ([`crate::entity::wikilink_rewrite::rewrite_mem_prefix`], sharing the
//! parser's code-masking discipline) rather than a parse → mutate →
//! regenerate round-trip: formatting outside the rewritten links stays
//! byte-identical, and both link surfaces (bodies and the
//! `## Relationships` section) are covered by the one pass because both
//! are wiki-links.
//!
//! One commit per affected mem, every commit tagged with a shared
//! `logical_operation_id`. Peer commits are parent-pinned
//! (`commit_with_expected_parent`) so a concurrent sibling writer
//! surfaces as [`EngineError::RenamePartialFailure`] rather than a
//! silent overwrite. Read-only mounts cannot be rewritten — their stale
//! references degrade to load-time stubs, which health surfaces.
//!
//! The sweep deliberately does NOT maintain the in-memory store — the
//! orchestrator (`memstead_engine::rename_mem`) reloads the engine
//! after the storage-identity flip, and everything between the sweep
//! and that reload happens inside one engine call.

use std::path::Path;

use crate::engine::{Engine, EngineError};
use crate::vcs::{Actor, CommitContext};

/// Outcome of [`Engine::rewrite_mem_references`].
#[derive(Debug, Clone)]
pub struct MemSweepOutcome {
    /// Mems whose entity files were rewritten and committed, in commit
    /// order (the renamed mem itself first when it had self-references
    /// or anchors, then peers sorted by name).
    pub rewritten_mems: Vec<String>,
    /// The shared `logical_operation_id` carried by every commit.
    pub logical_operation_id: String,
}

impl Engine {
    /// Rewrite every textual reference to mem `old_mem` so it carries
    /// `new_mem` instead: cross-mem wiki-links and Relationships
    /// entries in every writable peer mem, full-id self-references
    /// inside `old_mem` itself, and `old_mem`'s anchors-sidecar keys
    /// (`<old>--<slug>` → `<new>--<slug>`). One commit per affected
    /// mem; unaffected mems get no commit. Idempotent: a second run
    /// finds nothing left to rewrite and commits nothing — which is
    /// exactly what makes an interrupted `mem rename` completable by
    /// re-issuing it.
    ///
    /// Read-only mounts are skipped (no write access); their stale
    /// references surface as load-time stubs on the next boot.
    pub fn rewrite_mem_references(
        &mut self,
        old_mem: &str,
        new_mem: &str,
        note: Option<&str>,
    ) -> Result<MemSweepOutcome, EngineError> {
        // The sweep's answer scope is the whole workspace — every mem
        // that references `old_mem` must be rewritten — so its load
        // scope must match: a reference inside a deferred (lazy,
        // unloaded) mem would otherwise survive the rename unrewritten
        // (load-scope/answer-scope rule, flywheel W7/01).
        self.ensure_mems_loaded(None);
        let logical_op_id = crate::provenance::mint_logical_operation_id();

        // Plan first, commit after: collect per-mem rewrite lists
        // before any backend write so a read failure aborts cleanly.
        // The renamed mem itself sweeps first (self-references +
        // anchors move with the mem), then peers in name order.
        let mut mem_order: Vec<usize> = Vec::new();
        if let Some(own_idx) = self.mounts.iter().position(|m| {
            m.mount.mem == old_mem && m.mount.capability == crate::workspace::MountCapability::Write
        }) {
            mem_order.push(own_idx);
        }
        let mut peer_idxs: Vec<usize> = self
            .mounts
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                m.mount.mem != old_mem
                    && m.mount.capability == crate::workspace::MountCapability::Write
            })
            .map(|(i, _)| i)
            .collect();
        peer_idxs.sort_by(|a, b| self.mounts[*a].mount.mem.cmp(&self.mounts[*b].mount.mem));
        mem_order.extend(peer_idxs);

        let mut rewritten_mems: Vec<String> = Vec::new();
        for mount_idx in mem_order {
            let mem_name = self.mounts[mount_idx].mount.mem.clone();
            let backend = self.mounts[mount_idx].backend.as_ref();

            // Snapshot the head before planning so the commit is
            // parent-pinned against concurrent sibling writers.
            let head_snapshot = backend.current_head()?;

            let mut changed: Vec<(std::path::PathBuf, String)> = Vec::new();
            for rel_path in backend.list_entities()? {
                let Some(bytes) = backend.read_entity(&rel_path)? else {
                    continue;
                };
                let Ok(text) = String::from_utf8(bytes) else {
                    // Undecodable file — boot already warns about it;
                    // the sweep leaves it alone.
                    continue;
                };
                let (rewritten, count) =
                    crate::entity::wikilink_rewrite::rewrite_mem_prefix(&text, old_mem, new_mem);
                if count > 0 {
                    changed.push((rel_path, rewritten));
                }
            }

            // The renamed mem's anchors sidecar: re-key every
            // `<old>--<slug>` row to `<new>--<slug>` in the same
            // commit as its content rewrites.
            let mut anchors_changed = false;
            let mut sidecar_bytes: Option<Vec<u8>> = None;
            if mem_name == old_mem {
                let sidecar_raw = backend.read_anchors_sidecar()?;
                if let Some(raw) = sidecar_raw {
                    let sidecar = crate::anchor::AnchorSidecar::from_bytes(&raw)
                        .map_err(|e| EngineError::Mem(format!("anchors sidecar parse: {e}")))?;
                    let prefix = format!("{old_mem}--");
                    let mut next = crate::anchor::AnchorSidecar::default();
                    for (entity_id, anchors) in &sidecar.entities {
                        let new_key = match entity_id.strip_prefix(&prefix) {
                            Some(rest) => {
                                anchors_changed = true;
                                format!("{new_mem}--{rest}")
                            }
                            None => entity_id.clone(),
                        };
                        next.set(&new_key, anchors.clone());
                    }
                    if anchors_changed {
                        sidecar_bytes = Some(next.to_bytes());
                    }
                }
            }

            if changed.is_empty() && !anchors_changed {
                continue;
            }

            for (rel_path, text) in &changed {
                backend.write_entity(Path::new(rel_path), text.as_bytes())?;
            }
            if let Some(bytes) = sidecar_bytes {
                backend.write_anchors_sidecar(&bytes)?;
            }

            let subject = format!(
                "memstead: rename mem `{old_mem}` → `{new_mem}` (reference rewrite in `{mem_name}`)"
            );
            let ctx = CommitContext {
                actor: Actor::Agent,
                client: None,
                tool: Some("mem rename"),
                note: note.map(String::from),
                role: self.current_role,
                logical_operation_id: Some(logical_op_id.as_str()),
                entity_ids: None,
            };
            let commit_result =
                backend.commit_with_expected_parent(&subject, &ctx, head_snapshot.as_deref());
            let commit_sha = match commit_result {
                Ok(sha) => sha,
                Err(crate::backend::BackendError::ParentMismatch { .. }) => {
                    return Err(EngineError::RenamePartialFailure {
                        committed_mems: rewritten_mems,
                        failed_mem: mem_name,
                        failure_cause: "drift".to_string(),
                    });
                }
                Err(e) => return Err(e.into()),
            };
            self.record_self_write(mount_idx, &commit_sha);
            self.stamp_mutation_versions(mount_idx);
            rewritten_mems.push(mem_name);
        }

        Ok(MemSweepOutcome {
            rewritten_mems,
            logical_operation_id: logical_op_id,
        })
    }
}
